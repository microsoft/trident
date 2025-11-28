use std::path::{Path, PathBuf};

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
