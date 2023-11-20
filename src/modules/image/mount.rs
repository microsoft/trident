use std::{fs, path::Path, process::Command};

use anyhow::{Context, Error};
use log::error;
use osutils::systemd;
use trident_api::{config::HostConfiguration, status::HostStatus};

use crate::modules::storage::tabfile::TabFile;

pub(crate) fn unmount_target_volumes(mount_path: &Path) -> Result<(), Error> {
    let mount_unit_name = systemd::escape_mount_unit_name(&mount_path, systemd::MOUNT_UNIT_SUFFIX)?;
    crate::run_command(Command::new("systemctl").arg("stop").arg(mount_unit_name))
        .context("Failed to safely unmount target root partition.")?;
    Ok(())
}

pub(crate) fn mount_updated_volumes(
    host_config: &HostConfiguration,
    host_status: &HostStatus,
    root_mount_path: &Path,
) -> Result<(), Error> {
    let update_fs_target = Path::new("update-fs.target");
    let update_fstab_root = tempfile::tempdir().context("Failed to create temporary directory")?;
    let update_fstab_path = update_fstab_root.path().join("fstab");
    let systemd_unit_root_path = Path::new(crate::SYSTEMD_UNIT_ROOT_PATH);

    TabFile::from_mount_points(
        host_status,
        &host_config.storage.mount_points,
        Some(root_mount_path),
        Some(update_fs_target),
    )
    .context("Failed to generate bootstrap fstab")?
    .write(update_fstab_path.as_path())
    .context("Failed to write bootstrap fstab")?;

    // Create custom target for the filesystems mounted for the update reconciliation.
    fs::write(
        systemd_unit_root_path.join(update_fs_target),
        indoc::indoc! {r#"
                [Unit]
                Description=Update File Systems
                DefaultDependencies=no
                Conflicts=shutdown.target
            "#}
        .as_bytes(),
    )
    .context(format!(
        "Failed to write {}",
        update_fs_target.to_string_lossy()
    ))?;

    crate::run_command(
        Command::new("/usr/lib/systemd/system-generators/systemd-fstab-generator")
            .arg(systemd_unit_root_path)
            .arg(systemd_unit_root_path)
            .arg(systemd_unit_root_path)
            .env("SYSTEMD_FSTAB", update_fstab_path)
            .env("SYSTEMD_LOG_TARGET", "console")
            .env("SYSTEMD_LOG_LEVEL", "debug"),
    )
    .context("Failed to generate systemd units for the updated fstab")?;

    crate::run_command(Command::new("systemctl").arg("daemon-reload"))
        .context("Failed to reload systemd daemon")?;

    let mount_result =
        crate::run_command(Command::new("systemctl").arg("start").arg(update_fs_target))
            .context("Failed to mount target filesystems");

    if let Err(e) = mount_result {
        error!("{e:?}");
        unmount_target_volumes(root_mount_path)?;
        return Err(e);
    }

    Ok(())
}
