use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};

use duct::cmd;
use log::error;
use tempfile::TempDir;

use crate::{files, systemd};

use super::exe::OutputChecker;

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
        cmd!(
            "mount",
            "-t",
            "overlay",
            "overlay",
            "-o",
            format!(
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
            ),
            target_path
        )
        .run()
        .context("Failed to execute overlay mount command")?
        .check()
        .context("Overlay mount command failed")?;

        Ok(Self {
            dir,
            target_path: target_path.to_owned(),
        })
    }

    /// Unmounts the overlay and removes the temporary files.
    pub fn unmount(self) -> Result<(), Error> {
        cmd!("umount", self.target_path)
            .run()
            .context("Failed to run overlay unmount command")?
            .check()
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
pub struct EphemeralOverlayWithSystemD {
    dir: TempDir,
    unit_name: PathBuf,
}

impl EphemeralOverlayWithSystemD {
    const SYSTEMD_UNIT_ROOT_PATH: &'static str = "/etc/systemd/system";

    /// Creates the new overlay and mounts it on top of the provided path.
    pub fn mount(target_path: &Path) -> Result<Self, Error> {
        let dir = tempfile::tempdir().context("Failed to create temporary directory")?;
        let overlay_work_path = dir.path().join("work");
        let overlay_upper_path = dir.path().join("upper");
        files::create_dirs(&overlay_work_path).context("Failed to create overlay work dir")?;
        files::create_dirs(&overlay_upper_path).context("Failed to create overlay upper dir")?;

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
                    Options=lowerdir={target_path},upperdir={upperdir},workdir={workdir}
                "#},
                target_path = target_path
                    .to_str()
                    .context(format!("Failed to decode '{}'", target_path.display()))?,
                upperdir = overlay_upper_path.to_str().context(format!(
                    "Failed to decode '{}'",
                    overlay_upper_path.display()
                ))?,
                workdir = overlay_work_path.to_str().context(format!(
                    "Failed to decode '{}'",
                    overlay_work_path.display()
                ))?,
            ),
        )?;

        let this = Self {
            dir,
            unit_name: overlay_mount_unit_name.clone(),
        };

        if let Err(e1) = this.apply().context("Failed to mount overlay") {
            error!("Failed to apply overlay: {}", e1);
            match cmd!("systemctl", "status", overlay_mount_unit_name).read() {
                Ok(output) => {
                    error!("Report from systemctl status: {}", output);
                }
                Err(e2) => {
                    error!("Failed to read systemctl status for mount unit: {}", e2);
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
        cmd!("systemctl", "daemon-reload")
            .run()
            .context(
                "Failed to execute systemctl daemon-reload after creating the overlay mount unit",
            )?
            .check()
            .context(
                "SystemD failed to reload configuration files including the overlay mount unit",
            )?;

        cmd!("systemctl", "start", &self.unit_name)
            .run()
            .context("Failed to execute systemctl start for the overlay mount unit")?
            .check()
            .context("Failed to mount the overlay")?;

        Ok(())
    }

    /// Unmounts the overlay and removes the temporary files.
    pub fn unmount(self) -> Result<(), Error> {
        cmd!("systemctl", "stop", &self.unit_name)
            .run()
            .context("Failed to execute systemctl stop for the overlay mount unit")?
            .check()
            .context("Failed to unmount the overlay")?;
        fs::remove_file(Path::new(Self::SYSTEMD_UNIT_ROOT_PATH).join(self.unit_name))
            .context("Failed to clean up the overlay mount unit")?;
        self.dir
            .close()
            .context("Failed to cleanup the overlay temporary working directory")?;
        Ok(())
    }
}

#[cfg(all(test, feature = "integration-test"))]
mod test {
    use super::*;

    #[test]
    fn test_ephemeral_overlay() {
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
}
