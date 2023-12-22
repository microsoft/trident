use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Error};

use log::error;
use tempfile::TempDir;

use crate::{exe::RunAndCheck, files, lsof, systemd};

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
        Command::new("mount")
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
        Command::new("umount")
            .arg(self.target_path)
            .run_and_check()
            .context("Overlay unmount command failed")?;
        self.dir
            .close()
            .context("Failed to clean up overlay temporary working directory")?;
        Ok(())
    }
}

/// Mounts an overlayfs on top of the provided path. The overlay is removed when
/// exit() is called. The overlay temporary files are stored in a temporary
/// directory. Uses SystemD mount unit to mount overlays, so does work from
/// isolated environments like containers if needed to be visible from other
/// processes running on the host.
pub struct SystemDFilesystemOverlay {
    dir: Option<TempDir>,
    unit_name: PathBuf,
    target_path: PathBuf,
}

impl SystemDFilesystemOverlay {
    const SYSTEMD_UNIT_ROOT_PATH: &'static str = "/etc/systemd/system";

    /// Creates the new overlay and mounts it on top of the provided path.
    /// Overlay files are stored in a temporary directory that is deleted upon unmount.
    pub fn mount_temporary(target_path: &Path, options: &[&str]) -> Result<Self, Error> {
        let dir = tempfile::tempdir().context("Failed to create temporary directory")?;
        let overlay_work_path = dir.path().join("work");
        let overlay_upper_path = dir.path().join("upper");
        files::create_dirs(&overlay_work_path).context("Failed to create overlay work dir")?;
        files::create_dirs(&overlay_upper_path).context("Failed to create overlay upper dir")?;

        let mut this = Self::mount(
            target_path,
            &overlay_upper_path,
            &overlay_work_path,
            target_path,
            options,
        )?;
        this.dir = Some(dir);
        Ok(this)
    }

    /// Creates the new overlay and mounts it on top of the provided path.
    /// Overlay files are stored in the supplied directories.
    pub fn mount(
        target_path: &Path,
        upper_path: &Path,
        work_path: &Path,
        lower_path: &Path,
        options: &[&str],
    ) -> Result<Self, Error> {
        // Create .mount systemd unit for mounting the overlay, so it works from
        // a container as well
        let systemd_unit_root_path = Path::new(Self::SYSTEMD_UNIT_ROOT_PATH);
        let overlay_mount_unit_name =
            systemd::escape_mount_unit_name(&target_path, systemd::MOUNT_UNIT_SUFFIX)
                .context("Failed to escape overlay mount unit name")?;
        let overlay_mount_unit_path = systemd_unit_root_path.join(&overlay_mount_unit_name);
        fs::write(
            &overlay_mount_unit_path,
            format!(
                indoc::indoc! {r#"
                    [Unit]
                    Description=Trident Overlay Mount
                    DefaultDependencies=no
                    Conflicts=shutdown.target

                    [Mount]
                    What=overlay
                    Where={target_path}
                    Type=overlay
                    Options=lowerdir={lowerdir},upperdir={upperdir},workdir={workdir}{extra_options}
                "#},
                extra_options = if options.is_empty() {
                    "".to_owned()
                } else {
                    ",".to_owned() + &options.join(",")
                },
                target_path = target_path
                    .to_str()
                    .context(format!("Failed to decode '{}'", target_path.display()))?,
                upperdir = upper_path
                    .to_str()
                    .context(format!("Failed to decode '{}'", upper_path.display()))?,
                workdir = work_path
                    .to_str()
                    .context(format!("Failed to decode '{}'", work_path.display()))?,
                lowerdir = lower_path
                    .to_str()
                    .context(format!("Failed to decode '{}'", lower_path.display()))?,
            ),
        )?;

        let this = Self {
            dir: None,
            unit_name: overlay_mount_unit_name.clone(),
            target_path: target_path.to_owned(),
        };

        if let Err(e1) = this.apply().context("Failed to mount overlay") {
            error!("Failed to apply overlay: {}", e1);
            match Command::new("systemctl")
                .arg("status")
                .arg(overlay_mount_unit_name)
                .output_and_check()
            {
                Ok(output) => {
                    error!("Report from systemctl status: {}", output);
                }
                Err(e2) => {
                    error!("Failed to read systemctl status for mount unit: {}", e2);
                    error!("If running from a container, ensure to have access to the host's PID namespace.")
                }
            }
            if let Err(e3) = fs::remove_file(overlay_mount_unit_path) {
                error!("Failed to remove mount unit: {}", e3);
            }
            anyhow::bail!(e1);
        }

        Ok(this)
    }

    fn apply(&self) -> Result<(), Error> {
        Command::new("systemctl")
            .arg("daemon-reload")
            .run_and_check()
            .context(
                "SystemD failed to reload configuration files including the overlay mount unit",
            )?;

        Command::new("systemctl")
            .arg("start")
            .arg(&self.unit_name)
            .run_and_check()
            .context("Failed to mount the overlay")?;

        Ok(())
    }

    /// Unmounts the overlay and removes the temporary files.
    pub fn unmount(self) -> Result<(), Error> {
        let res = Command::new("systemctl")
            .arg("stop")
            .arg(&self.unit_name)
            .run_and_check()
            .context("Failed to unmount the overlay");
        if res.is_err() {
            let opened_process_files = lsof::run(&self.target_path);
            // best effort, ignore failures here (such as missing external dependency)
            if let Ok(opened_process_files) = opened_process_files {
                error!("Open files: {:?}", opened_process_files);
            }

            res?
        }
        fs::remove_file(Path::new(Self::SYSTEMD_UNIT_ROOT_PATH).join(self.unit_name))
            .context("Failed to clean up the overlay mount unit")?;
        if let Some(dir) = self.dir {
            dir.close()
                .context("Failed to cleanup the overlay temporary working directory")?;
        }
        Ok(())
    }
}

#[cfg(all(test, feature = "functional-tests"))]
mod functional_tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn test() {
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

        // fail if target is missing
        let does_not_exist = Path::new("/does-not-exist");
        if does_not_exist.exists() {
            std::fs::remove_dir(does_not_exist).unwrap();
        }

        assert_eq!(
            EphemeralOverlay::mount(Path::new("/does-not-exist"))
                .err()
                .unwrap()
                .root_cause()
                .to_string(),
            "Process output:\nstderr:\nmount: /does-not-exist: mount point does not exist.\n\n"
        );
    }

    #[test]
    pub fn test_systemd() {
        let dir = tempfile::tempdir().unwrap();
        let overlay = SystemDFilesystemOverlay::mount_temporary(dir.path(), &[]).unwrap();
        // create a file on top of the overlay
        let test_file = dir.path().join("test");
        std::fs::write(&test_file, "test").unwrap();
        // check that the file exists in the overlay
        assert!(test_file.exists());

        overlay.unmount().unwrap();
        // check that the file does not exist in the target
        assert!(!test_file.exists());

        // fail to write file for read-only overlay
        let overlay = SystemDFilesystemOverlay::mount_temporary(dir.path(), &["ro"]).unwrap();
        // create a file on top of the overlay
        let test_file = dir.path().join("test");
        assert_eq!(
            std::fs::write(test_file, "test").unwrap_err().to_string(),
            "Read-only file system (os error 30)"
        );
        overlay.unmount().unwrap();

        // fail to mount on top of a symlink
        let symlink_path = Path::new("/tmp2");
        if symlink_path.exists() {
            std::fs::remove_file(symlink_path).unwrap();
        }
        symlink(Path::new("/tmp"), symlink_path).unwrap();
        assert_eq!(
            SystemDFilesystemOverlay::mount_temporary(symlink_path, &[])
                .err()
                .unwrap()
                .root_cause()
                .to_string(),
            "Process output:\nstderr:\nJob failed. See \"journalctl -xe\" for details.\n\n"
        );

        // test persistent mount
        let dir_base = tempfile::tempdir()
            .context("Failed to create temporary directory")
            .unwrap();
        let overlay_work_path = dir_base.path().join("work");
        let overlay_upper_path = dir_base.path().join("upper");
        files::create_dirs(&overlay_work_path)
            .context("Failed to create overlay work dir")
            .unwrap();
        files::create_dirs(&overlay_upper_path)
            .context("Failed to create overlay upper dir")
            .unwrap();

        let overlay = SystemDFilesystemOverlay::mount(
            dir.path(),
            &overlay_upper_path,
            &overlay_work_path,
            Path::new("/etc"),
            &[],
        )
        .unwrap();
        // create a file on top of the overlay
        let test_file = dir.path().join("test");
        std::fs::write(&test_file, "test").unwrap();
        // check that the file exists in the overlay
        assert!(test_file.exists());

        overlay.unmount().unwrap();
        // check that the file does not exist in the target
        assert!(!test_file.exists());

        // check it exists in the upper directory
        let test_file = overlay_upper_path.join("test");
        assert!(test_file.exists());
    }
}
