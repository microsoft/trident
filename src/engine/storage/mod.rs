use std::path::PathBuf;

use anyhow::{Context, Error};
use log::{debug, info, trace, warn};

use osutils::{e2fsck, hashing_reader::compute_file_hash};
use trident_api::{
    config::{FileSystemType, HostConfiguration},
    constants::internal_params::PRE_REBOOT_CHECKS,
    error::{ReportError, ServicingError, TridentError, TridentResultExt},
    status::HostStatus,
};

mod encryption;
mod filesystem;
pub mod image;
pub mod partitioning;
pub mod raid;
pub mod rebuild;
pub mod verity;

use super::EngineContext;

const ENCRYPTION_SUBSYSTEM_NAME: &str = "encryption";

#[tracing::instrument(skip_all)]
pub(super) fn create_block_devices(ctx: &mut EngineContext) -> Result<(), TridentError> {
    trace!("Mount points: {:?}", ctx.spec.storage.internal_mount_points);

    debug!("Initializing block devices");

    // Close verity devices and encrypted volumes before stopping RAID
    // arrays, as both can sit on top of RAID arrays.
    close_pre_existing_devices(ctx).message("Closing pre-existing block devices failed")?;

    partitioning::create_partitions(ctx).structured(ServicingError::CreatePartitions)?;
    raid::create_sw_raid(ctx, &ctx.spec).structured(ServicingError::CreateRaid)?;
    encryption::provision(ctx, &ctx.spec).message(format!(
        "Step 'Provision' failed for subsystem '{ENCRYPTION_SUBSYSTEM_NAME}'"
    ))?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(super) fn close_pre_existing_devices(ctx: &EngineContext) -> Result<(), TridentError> {
    debug!("Closing pre-existing block devices");

    // Close verity devices and encrypted volumes before stopping RAID
    // arrays, as both can sit on top of RAID arrays.
    verity::stop_pre_existing_verity_devices(&ctx.spec)
        .structured(ServicingError::CleanupVerity)?;
    encryption::close_pre_existing_encrypted_volumes(&ctx.spec)
        .structured(ServicingError::CleanupEncryption)?;
    raid::stop_pre_existing_raid_arrays(&ctx.spec).structured(ServicingError::CleanupRaid)?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(super) fn initialize_block_devices(ctx: &EngineContext) -> Result<(), TridentError> {
    trace!("Mount points: {:?}", ctx.spec.storage.internal_mount_points);

    image::deploy_images(ctx).structured(ServicingError::DeployImages)?;

    filesystem::create_filesystems(ctx).structured(ServicingError::CreateFilesystems)?;

    // Assumes that images are already in place (data and hash), so that it can
    // assemble the verity devices.
    verity::setup_verity_devices(ctx).structured(ServicingError::CreateVerity)?;

    Ok(())
}

pub(super) fn check_block_devices(host_status: &HostStatus) {
    if !host_status.spec.internal_params.get_flag(PRE_REBOOT_CHECKS) {
        return;
    }

    for (id, path) in &host_status.partition_paths {
        let Ok(canonical) = path.canonicalize() else {
            warn!(
                "Block device '{id}' (path '{}'): No longer exists",
                path.display()
            );
            continue;
        };

        let Ok((length, sha384)) = compute_file_hash(&canonical) else {
            warn!(
                "Block device '{id}' (path '{}' -> '{}'): Failed to compute hash",
                path.display(),
                canonical.display()
            );
            continue;
        };

        let fs_type = host_status
            .spec
            .storage
            .internal_mount_points
            .iter()
            .find(|fs| &fs.target_id == id)
            .map(|fs| fs.filesystem);

        let fsck_status = match fs_type {
            Some(FileSystemType::Ext4) => {
                if let Err(e) = e2fsck::check(&canonical) {
                    format!(", e2fsck failed: {e:?}")
                } else {
                    ", e2fsck OK".to_string()
                }
            }
            _ => "".to_string(),
        };

        info!(
            "Block device '{id}' (path '{}' -> '{}'): Size = {length} bytes, sha384 = {sha384}{fsck_status}",
            path.display(),
            canonical.display(),
        );
    }
}

/// Get the canonicalized paths of all disks in a Host Configuration
fn get_hostconfig_disk_paths(host_config: &HostConfiguration) -> Result<Vec<PathBuf>, Error> {
    host_config
        .storage
        .disks
        .iter()
        .map(|disk| {
            disk.device
                .canonicalize()
                .with_context(|| format!("failed to get canonicalized path for disk: {}", disk.id))
        })
        .collect()
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use osutils::testutils::repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH};
    use pytest_gen::functional_test;
    use trident_api::config::{Disk, Storage};

    #[functional_test]
    fn test_get_hostconfig_disk_paths() {
        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk1".to_owned(),
                        device: "/dev/sda".into(),
                        ..Default::default()
                    },
                    Disk {
                        id: "disk2".to_owned(),
                        device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-3".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let disks = get_hostconfig_disk_paths(&host_config).unwrap();
        assert_eq!(
            disks,
            vec![
                PathBuf::from(OS_DISK_DEVICE_PATH),
                PathBuf::from(TEST_DISK_DEVICE_PATH)
            ]
        );

        // fail on missing disk
        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk1".to_owned(),
                    device: "/dev/sdc".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            get_hostconfig_disk_paths(&host_config)
                .unwrap_err()
                .to_string(),
            "failed to get canonicalized path for disk: disk1"
        );
    }
}
