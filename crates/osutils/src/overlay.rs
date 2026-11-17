use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use tempfile::TempDir;

use crate::{dependencies::Dependency, files};

/// Mounts an overlayfs on top of the provided path. The overlay is removed when
/// exit() is called. The overlay temporary files are stored in a temporary
/// directory. Uses `mount` directly to mount overlays, so does not work from
/// isolated environments like containers if needed to be visible from other
/// processes running on the host.
pub struct EphemeralOverlay {
    dir: TempDir,
    target_path: PathBuf,
}

impl EphemeralOverlay {
    /// Creates the new overlay and mounts it on top of the provided path.
    pub fn mount(target_path: &Path) -> Result<Self, Error> {
        let dir = tempfile::tempdir().context("Failed to create temporary directory")?;
        let overlay_work_path = dir.path().join("work");
        let overlay_upper_path = dir.path().join("upper");
        files::create_dirs(&overlay_work_path).context("Failed to create overlay work dir")?;
        files::create_dirs(&overlay_upper_path).context("Failed to create overlay upper dir")?;
        Dependency::Mount
            .cmd()
            .arg("-t")
            .arg("overlay")
            .arg("overlay")
            .arg("-o")
            .arg(format!(
                "lowerdir={},upperdir={},workdir={}",
                target_path
                    .to_str()
                    .context(format!("Failed to decode '{}'", target_path.display()))?,
                overlay_upper_path.to_str().context(format!(
                    "Failed to decode '{}'",
                    overlay_upper_path.display()
                ))?,
                overlay_work_path.to_str().context(format!(
                    "Failed to decode '{}'",
                    overlay_work_path.display()
                ))?,
            ))
            .arg(target_path)
            .run_and_check()
            .context("Overlay mount command failed")?;

        Ok(Self {
            dir,
            target_path: target_path.to_owned(),
        })
    }

    /// Unmounts the overlay and removes the temporary files.
    pub fn unmount(self) -> Result<(), Error> {
        Dependency::Umount
            .cmd()
            .arg(self.target_path)
            .run_and_check()
            .context("Overlay unmount command failed")?;
        self.dir
            .close()
            .context("Failed to clean up overlay temporary working directory")?;
        Ok(())
    }
}

/// An overlay mounted from explicitly chosen layers.
///
/// Unlike [`EphemeralOverlay`], whose upper layer is a temporary directory
/// discarded on unmount, this mounts a caller-provided upper layer, so writes
/// made through the overlay persist in it. That makes the merged view -- and
/// crucially overlayfs copy-up, which duplicates a lower-layer file into the
/// upper layer on first write -- available to code that would otherwise see
/// only the upper layer's contents.
pub struct LayeredOverlay {
    target_path: PathBuf,
    work_dir: PathBuf,
}

impl LayeredOverlay {
    /// Mounts an overlay over `target_path` with the given layers.
    ///
    /// `work_dir` must be on the same filesystem as `upper_dir`, as overlayfs
    /// requires. It is created if missing, as are the other directories.
    /// `options` are appended to the layer options, for callers that need to
    /// match the mount options another party uses for the same overlay.
    pub fn mount(
        target_path: impl AsRef<Path>,
        lower_dir: impl AsRef<Path>,
        upper_dir: impl AsRef<Path>,
        work_dir: impl AsRef<Path>,
        options: Option<&str>,
    ) -> Result<Self, Error> {
        let target_path = target_path.as_ref();
        let (lower_dir, upper_dir, work_dir) =
            (lower_dir.as_ref(), upper_dir.as_ref(), work_dir.as_ref());

        for dir in [upper_dir, work_dir] {
            files::create_dirs(dir)
                .with_context(|| format!("Failed to create overlay dir '{}'", dir.display()))?;
        }

        let path_str = |p: &Path| -> Result<String, Error> {
            Ok(p.to_str()
                .with_context(|| format!("Failed to decode '{}'", p.display()))?
                .to_owned())
        };

        let mut opts = format!(
            "lowerdir={},upperdir={},workdir={}",
            path_str(lower_dir)?,
            path_str(upper_dir)?,
            path_str(work_dir)?,
        );
        if let Some(options) = options {
            opts.push(',');
            opts.push_str(options);
        }

        Dependency::Mount
            .cmd()
            .arg("-t")
            .arg("overlay")
            .arg("overlay")
            .arg("-o")
            .arg(opts)
            .arg(target_path)
            .run_and_check()
            .with_context(|| format!("Failed to mount overlay on '{}'", target_path.display()))?;

        Ok(Self {
            target_path: target_path.to_owned(),
            work_dir: work_dir.to_owned(),
        })
    }

    /// Unmounts the overlay, leaving both layers in place.
    ///
    /// The work directory is removed, since it is overlayfs bookkeeping rather
    /// than content and is meaningless once the overlay is gone.
    pub fn unmount(self) -> Result<(), Error> {
        Dependency::Umount
            .cmd()
            .arg(&self.target_path)
            .run_and_check()
            .with_context(|| {
                format!(
                    "Failed to unmount overlay on '{}'",
                    self.target_path.display()
                )
            })?;

        fs::remove_dir_all(&self.work_dir).with_context(|| {
            format!(
                "Failed to remove overlay work directory '{}'",
                self.work_dir.display()
            )
        })
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_ephemeral_overlay_mount_unmount() {
        let dir = tempfile::tempdir().unwrap();
        let overlay = EphemeralOverlay::mount(dir.path()).unwrap();
        // create a file on top of the overlay
        let test_file = dir.path().join("test");
        std::fs::write(&test_file, "test").unwrap();
        // check that the file exists in the overlay
        assert!(test_file.exists());

        overlay.unmount().unwrap();
        // check that the file does not exist in the target
        assert!(!test_file.exists());
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_ephemeral_overlay_mount_fails_on_missing_target() {
        // fail if target is missing
        let does_not_exist = Path::new("/does-not-exist");
        if does_not_exist.exists() {
            std::fs::remove_dir(does_not_exist).unwrap();
        }

        let error_string = EphemeralOverlay::mount(does_not_exist)
            .err()
            .unwrap()
            .root_cause()
            .to_string();
        assert!(
            error_string.contains("stderr:\nmount: /does-not-exist: mount point does not exist.\n"),
            "Unexpected error message: {error_string}",
        );
    }
}
