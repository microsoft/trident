use std::{fs, path::Path, process::Command};

use anyhow::{Context, Error};
use log::error;
use osutils::{exe::RunAndCheck, systemd};
use trident_api::{config::HostConfiguration, status::HostStatus};

use crate::modules::storage::tabfile::{TabFile, TabFileSettings};

pub(crate) fn unmount_updated_volumes(mount_path: &Path) -> Result<(), Error> {
    let mount_unit_name = systemd::escape_mount_unit_name(&mount_path, systemd::MOUNT_UNIT_SUFFIX)?;
    Command::new("systemctl")
        .arg("stop")
        .arg(mount_unit_name)
        .run_and_check()
        .context("Failed to safely unmount target root partition.")
}

pub(crate) fn mount_updated_volumes(
    host_config: &HostConfiguration,
    host_status: &HostStatus,
    root_mount_path: &Path,
    read_only: bool,
) -> Result<(), Error> {
    let update_fs_target = if read_only {
        Path::new("update-fs-ro.target")
    } else {
        Path::new("update-fs.target")
    };
    let update_fstab_root = tempfile::tempdir().context("Failed to create temporary directory")?;
    let update_fstab_path = update_fstab_root.path().join("fstab");
    let systemd_unit_root_path = Path::new(crate::SYSTEMD_UNIT_ROOT_PATH);

    let mut tab_file_settings = TabFileSettings {
        path_prefix: Some(root_mount_path),
        required_by: Some(update_fs_target),
        ..Default::default()
    };
    if read_only {
        tab_file_settings.read_only = true;
    } else {
        tab_file_settings.make_fs = true;
        tab_file_settings.grow_fs = true;
    }

    TabFile::from_mount_points(
        host_status,
        &host_config.storage.mount_points,
        &tab_file_settings,
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

    Command::new("/usr/lib/systemd/system-generators/systemd-fstab-generator")
        .arg(systemd_unit_root_path)
        .arg(systemd_unit_root_path)
        .arg(systemd_unit_root_path)
        .env("SYSTEMD_FSTAB", update_fstab_path)
        .env("SYSTEMD_LOG_TARGET", "console")
        .env("SYSTEMD_LOG_LEVEL", "debug")
        .run_and_check()
        .context("Failed to generate systemd units for the updated fstab")?;

    Command::new("systemctl")
        .arg("daemon-reload")
        .run_and_check()
        .context("Failed to reload systemd daemon")?;

    let mount_result = Command::new("systemctl")
        .arg("start")
        .arg(update_fs_target)
        .run_and_check()
        .context(
            "Failed to mount target filesystems".to_owned()
                + (if read_only { " (read-only)" } else { "" }),
        );

    if let Err(e) = mount_result {
        error!("{e:?}");
        let dep_output = Command::new("systemctl")
            .arg("list-dependencies")
            .arg(update_fs_target)
            .output_and_check()
            .context("Failed to list dependencies of the mount target")?;
        error!("Dependencies of the mount target:\n{dep_output}");
        unmount_updated_volumes(root_mount_path)?;
        return Err(e);
    }

    Ok(())
}
