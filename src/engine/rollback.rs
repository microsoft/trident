use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Error};
use log::{debug, info, trace};

use osutils::{block_devices, dependencies::Dependency, veritysetup};

use trident_api::{
    config::{AbUpdate, InternalVerityDevice},
    constants::ROOT_MOUNT_POINT_PATH,
    error::{ReportError, ServicingError, TridentError, TridentResultExt},
    status::{AbVolumeSelection, ServicingState, ServicingType},
    BlockDeviceId,
};

use crate::{
    engine::{self, bootentries, storage::verity, EngineContext},
    DataStore,
};

/// Validates that the firmware did not perform a rollback, i.e. correctly booted from the updated
/// runtime OS image.
///
/// If the firmware did not boot from the expected root device, this function will return an error.
/// In either case, the function will update the Host Status.
#[tracing::instrument(skip_all)]
pub fn validate_boot(datastore: &mut DataStore) -> Result<(), TridentError> {
    info!("Validating whether host correctly booted from updated runtime OS image");

    // Create an EngineContext based on the Host Status
    let ctx = EngineContext {
        spec: datastore.host_status().spec.clone(),
        spec_old: datastore.host_status().spec_old.clone(),
        servicing_type: datastore.host_status().servicing_type,
        ab_active_volume: datastore.host_status().ab_active_volume,
        block_device_paths: datastore.host_status().block_device_paths.clone(),
        disks_by_uuid: datastore.host_status().disks_by_uuid.clone(),
        install_index: datastore.host_status().install_index,
        os_image: None, // Not used for boot validation logic
    };

    // Get the block device path of the current root
    let root_device_path =
        get_current_root_device_path(&ctx).message("Failed to get root block device path")?;

    // Get expected root device path
    let expected_root_device_path =
        get_expected_root_device_path(&ctx).message("Failed to get expected root device path")?;

    if compare_root_device_paths(root_device_path.clone(), expected_root_device_path.clone())
        .message("Host failed to boot from expected root device")?
    {
        info!("Host correctly booted from updated runtime OS image");

        // If it's QEMU, after confirming that we have booted into the
        // correct image, we need to update the `BootOrder` to boot from
        // the correct image next time.
        if osutils::virt::is_qemu() {
            bootentries::set_bootentries_after_reboot_for_qemu()
                .message("Failed to set boot entries after reboot")?;
        }
    } else if datastore.host_status().servicing_type == ServicingType::CleanInstall {
        // If Trident was executing a clean install, need to re-set the Host Status.
        datastore.with_host_status(|host_status| {
            host_status.spec = Default::default();
            host_status.servicing_type = ServicingType::NoActiveServicing;
            host_status.servicing_state = ServicingState::NotProvisioned;
        })?;

        return Err(TridentError::new(ServicingError::CleanInstallRebootCheck {
            root_device_path: root_device_path.to_string_lossy().to_string(),
            expected_device_path: expected_root_device_path.to_string_lossy().to_string(),
        }));
    } else {
        // If Trident was executing an A/B update, need to re-set the Host Status.
        datastore.with_host_status(|host_status| {
            host_status.spec = host_status.spec_old.clone();
            host_status.spec_old = Default::default();
            host_status.servicing_type = ServicingType::NoActiveServicing;
            host_status.servicing_state = ServicingState::Provisioned;
        })?;

        return Err(TridentError::new(ServicingError::AbUpdateRebootCheck {
            root_device_path: root_device_path.to_string_lossy().to_string(),
            expected_device_path: expected_root_device_path.to_string_lossy().to_string(),
        }));
    }

    match datastore.host_status().servicing_type {
        ServicingType::CleanInstall => {
            info!("Clean install of runtime OS succeeded");
            tracing::info!(metric_name = "clean_install_success", value = true);
        }
        ServicingType::AbUpdate => {
            info!("A/B update succeeded");
            tracing::info!(metric_name = "ab_update_success", value = true);
        }
        // Because the boot validation logic is currently called only on clean install and A/B
        // update, this should be unreachable.
        // TODO: When/If `UpdateAndReboot` is used, this should be updated.
        _ => unreachable!(),
    }

    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::Provisioned
    );

    datastore.with_host_status(|host_status| {
        host_status.servicing_state = ServicingState::Provisioned;
        host_status.servicing_type = ServicingType::NoActiveServicing;
        host_status.spec_old = Default::default();
        host_status.ab_active_volume = match host_status.ab_active_volume {
            None | Some(AbVolumeSelection::VolumeB) => Some(AbVolumeSelection::VolumeA),
            Some(AbVolumeSelection::VolumeA) => Some(AbVolumeSelection::VolumeB),
        };
    })?;

    Ok(())
}

/// Returns the current root device path, i.e. the device path that the host booted from.
fn get_current_root_device_path(ctx: &EngineContext) -> Result<PathBuf, TridentError> {
    // If the root is verity, fetch the block device path of the root data device path from the
    // 'veritysetup' output; otherwise, fetch the root device path from the host.
    let current_root_device_path = if ctx.spec.storage.root_is_verity() {
        // Get the block device ID of root
        let root_device_id = ctx
            .spec
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
            .map(|m| &m.target_id)
            .structured(ServicingError::GetRootMountPointInfo {
                root_path: ROOT_MOUNT_POINT_PATH.to_string(),
            })?;
        debug!("Root device ID: {}", root_device_id);

        get_root_verity_data_device_path(ctx, root_device_id)
            .structured(ServicingError::GetRootVerityDataDevPath)?
    } else {
        // Fetch the root device path that the host booted from
        block_devices::get_root_device_path()?
    };

    debug!(
        "Current root device path: '{}'",
        current_root_device_path.display()
    );

    Ok(current_root_device_path)
}

/// Returns the path of the root device that the host was expected to boot from.
fn get_expected_root_device_path(ctx: &EngineContext) -> Result<PathBuf, TridentError> {
    // Get the block device ID of root
    let root_device_id = ctx
        .spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
        .map(|m| &m.target_id)
        .structured(ServicingError::GetRootMountPointInfo {
            root_path: ROOT_MOUNT_POINT_PATH.to_string(),
        })?;

    let expected_root_device_path = if ctx.spec.storage.root_is_verity() {
        // If root is on verity, fetch the block device path of the verity data device. Because
        // get_block_device_path(), which is called eventually, already has the logic for
        // determining the update volume, i.e. volume we expect to have booted from, getting the
        // block device path of the verity data device is sufficient.
        let root_verity_device_config = get_root_verity_device_config(ctx, root_device_id)
            .structured(ServicingError::GetRootVerityDeviceConfig)?;

        let (verity_data_path, _) =
            verity::get_verity_device_paths(ctx, &root_verity_device_config)
                .structured(ServicingError::GetRootVerityDataDevPath)?;

        verity_data_path
    } else {
        // Fetch the expected root device path
        engine::get_block_device_path(ctx, root_device_id).structured(
            ServicingError::GetBlockDevicePath {
                device_id: root_device_id.to_string(),
            },
        )?
    };

    debug!(
        "Expected root device path: '{}'",
        expected_root_device_path.display()
    );

    Ok(expected_root_device_path)
}

/// Compares the expected root device path with the current root device path that the host booted
/// from. Returns true if they match; false otherwise.
fn compare_root_device_paths(
    root_dev_path: PathBuf,
    expected_root_dev_path: PathBuf,
) -> Result<bool, TridentError> {
    // Canonicalize both paths
    let root_dev_path_canonicalized =
        root_dev_path
            .canonicalize()
            .structured(ServicingError::CanonicalizePath {
                path: root_dev_path.display().to_string(),
            })?;

    let expected_root_path_canonicalized =
        expected_root_dev_path
            .canonicalize()
            .structured(ServicingError::CanonicalizePath {
                path: expected_root_dev_path.display().to_string(),
            })?;

    info!(
        "Expected host to boot from block device with path '{}'",
        expected_root_path_canonicalized.display()
    );

    // If current root device path is NOT the same as the expected root device path, return false.
    if root_dev_path_canonicalized != expected_root_path_canonicalized {
        info!(
            "But host booted from an unexpected device with path '{}'",
            root_dev_path.display()
        );

        return Ok(false);
    }

    info!(
        "Host booted from the expected root device '{}'",
        root_dev_path.display()
    );

    Ok(true)
}

/// Validates that the A/B active volume in Host Status is set correctly.
///
/// This function is called before starting any update servicing, to confirm that the firmware did
/// not perform a rollback, since the A/B active volume was set in Host Status.
pub(crate) fn validate_active_volume(
    ctx: &EngineContext,
    root_device_path: PathBuf,
) -> Result<(), Error> {
    let ab_update = &ctx
        .spec
        .storage
        .ab_update
        .as_ref()
        .context("No A/B update found")?;

    let root_device_id = ctx
        .spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
        .map(|m| &m.target_id)
        .context("No mount point for root volume found")?;
    debug!("Root device ID: {:?}", root_device_id);

    let (volume_pair_paths, root_data_device_path) = if ctx.spec.storage.root_is_verity() {
        debug!("Root is a verity device");

        let volume_pair = get_verity_data_volume_pair_paths(ctx, ab_update, root_device_id)
            .context("Failed to find root verity data volume pair")?;

        let root_data_device_path = get_root_verity_data_device_path(ctx, root_device_id)
            .context("Failed to find root verity data device path")?;

        (volume_pair, root_data_device_path)
    } else {
        debug!("Root is not on verity");

        let volume_pair = get_plain_volume_pair_paths(ctx, ab_update, root_device_id)
            .context("Failed to find root volume pair")?;

        (volume_pair, root_device_path)
    };

    debug!(
        "Root volume A path: {} (device ID: {})",
        volume_pair_paths.volume_a_path.display(),
        volume_pair_paths.volume_a_id
    );
    debug!(
        "Root volume B path: {} (device ID: {})",
        volume_pair_paths.volume_b_path.display(),
        volume_pair_paths.volume_b_id
    );

    let volume_a_path_canonical =
        volume_pair_paths
            .volume_a_path
            .canonicalize()
            .context(format!(
                "Failed to canonicalize path '{}' for device with ID '{}'",
                volume_pair_paths.volume_a_path.display(),
                volume_pair_paths.volume_a_id,
            ))?;
    let volume_b_path_canonical =
        volume_pair_paths
            .volume_b_path
            .canonicalize()
            .context(format!(
                "Failed to canonicalize path '{}' for device with ID '{}'",
                volume_pair_paths.volume_b_path.display(),
                volume_pair_paths.volume_b_id,
            ))?;

    debug!(
        "Root volume A path (canonical): {}",
        volume_a_path_canonical.display()
    );
    debug!(
        "Root volume B path (canonical): {}",
        volume_b_path_canonical.display()
    );

    trace!(
        "Available devices: {}",
        Dependency::Blkid.cmd().output_and_check().unwrap()
    );

    // Validate that the active volume in Host Status matches actual root device path
    if volume_a_path_canonical == root_data_device_path {
        debug!(
            "Current root device path '{}' matches root volume A path '{}'",
            root_data_device_path.display(),
            volume_a_path_canonical.display()
        );

        if ctx.ab_active_volume != Some(AbVolumeSelection::VolumeA) {
            bail!(
                "Volume A is active, but active volume in Host Status is set to {}",
                ctx.ab_active_volume
                    .map_or("None".to_string(), |v| v.to_string())
            );
        }
    } else if volume_b_path_canonical == root_data_device_path {
        debug!(
            "Current root device path '{}' matches root volume B path '{}'",
            root_data_device_path.display(),
            volume_b_path_canonical.display()
        );

        if ctx.ab_active_volume != Some(AbVolumeSelection::VolumeB) {
            bail!(
                "Volume B is active, but active volume in Host Status is set to {}",
                ctx.ab_active_volume
                    .map_or("None".to_string(), |v| v.to_string())
            );
        }
    } else {
        bail!("Failed to match current root device path '{}' to either volume A path '{}' or volume B path '{}'",
            root_data_device_path.display(),
            volume_a_path_canonical.display(),
            volume_b_path_canonical.display()
        )
    }

    Ok(())
}

#[derive(Debug, PartialEq)]
struct VolumePairPaths {
    volume_a_path: PathBuf,
    volume_b_path: PathBuf,
    volume_a_id: BlockDeviceId,
    volume_b_id: BlockDeviceId,
}

/// Returns the paths of the A/B volume pair for the root device.
fn get_plain_volume_pair_paths(
    ctx: &EngineContext,
    ab_update: &AbUpdate,
    root_device_id: &BlockDeviceId,
) -> Result<VolumePairPaths, Error> {
    let root_device_pair = ab_update
        .volume_pairs
        .iter()
        .find(|p| &p.id == root_device_id)
        .context("No volume pair for root volume found")?;
    debug!("Root device pair: {:?}", root_device_pair);

    let volume_a_path =
        engine::get_block_device_path(ctx, &root_device_pair.volume_a_id).context(format!(
            "Failed to get block device path for volume A with ID '{}'",
            root_device_pair.volume_a_id
        ))?;
    let volume_b_path =
        engine::get_block_device_path(ctx, &root_device_pair.volume_b_id).context(format!(
            "Failed to get block device path for volume B with ID '{}'",
            root_device_pair.volume_b_id
        ))?;

    Ok(VolumePairPaths {
        volume_a_path: volume_a_path.clone(),
        volume_b_path: volume_b_path.clone(),
        volume_a_id: root_device_pair.volume_a_id.clone(),
        volume_b_id: root_device_pair.volume_b_id.clone(),
    })
}

/// Returns the paths of the A/B volume pair for the root verity data device.
fn get_verity_data_volume_pair_paths(
    ctx: &EngineContext,
    ab_update: &AbUpdate,
    root_device_id: &BlockDeviceId,
) -> Result<VolumePairPaths, Error> {
    let root_verity_device_config = get_root_verity_device_config(ctx, root_device_id)?;

    let root_data_device_pair = ab_update
        .volume_pairs
        .iter()
        .find(|vp| vp.id == root_verity_device_config.data_target_id)
        .context("No volume pair for root data device found")?;
    debug!("Root data device pair: {:?}", root_data_device_pair);

    let volume_a_path = engine::get_block_device_path(ctx, &root_data_device_pair.volume_a_id)
        .context(format!(
            "Failed to get block device for data volume A with ID '{}'",
            &root_data_device_pair.volume_a_id
        ))?;
    let volume_b_path = engine::get_block_device_path(ctx, &root_data_device_pair.volume_b_id)
        .context(format!(
            "Failed to get block device for data volume B with ID '{}'",
            &root_data_device_pair.volume_b_id
        ))?;

    Ok(VolumePairPaths {
        volume_a_path,
        volume_b_path,
        volume_a_id: root_data_device_pair.volume_a_id.clone(),
        volume_b_id: root_data_device_pair.volume_b_id.clone(),
    })
}

/// Gets the path of the root verity data device for the given root device ID.
fn get_root_verity_data_device_path(
    ctx: &EngineContext,
    root_device_id: &BlockDeviceId,
) -> Result<PathBuf, Error> {
    let root_verity_device_config =
        get_root_verity_device_config(ctx, root_device_id).context(format!(
            "Failed to get configuration for root verity device '{}'",
            root_device_id
        ))?;

    // Run 'veritysetup' to get the data device path
    let root_verity_status =
        veritysetup::status(&root_verity_device_config.device_name).context(format!(
            "Failed to get verity status for device '{}'",
            root_verity_device_config.device_name
        ))?;
    trace!("Root verity status: {:?}", root_verity_status);

    Ok(root_verity_status.data_device_path)
}

/// Gets the configuration for the root verity device for the given root device ID.
pub fn get_root_verity_device_config(
    ctx: &EngineContext,
    root_device_id: &BlockDeviceId,
) -> Result<InternalVerityDevice, Error> {
    // Get the root data device path from the 'veritysetup' output
    let root_verity_device_config = ctx
        .spec
        .storage
        .internal_verity
        .iter()
        .find(|vd| &vd.id == root_device_id)
        .cloned()
        .context(format!(
            "Failed to find configuration for root verity device '{}'",
            root_device_id
        ))?;
    trace!("Root verity device config: {:?}", root_verity_device_config);

    Ok(root_verity_device_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use maplit::btreemap;

    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystemType, Image, ImageFormat, ImageSha256,
            InternalMountPoint, InternalVerityDevice, MountOptions, MountPoint, Partition,
            PartitionType, VerityFileSystem,
        },
        constants::MOUNT_OPTION_READ_ONLY,
        error::ErrorKind,
        status::AbVolumeSelection,
    };

    #[test]
    fn test_get_expected_root_device_path() {
        let mut ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        // Add a disk and partitions
        ctx.spec.storage.disks.push(Disk {
            id: "os".to_owned(),
            device: PathBuf::from("/dev/disk/by-bus/foobar"),
            partitions: vec![
                Partition {
                    id: "esp".to_owned(),
                    size: 100.into(),
                    partition_type: PartitionType::Esp,
                },
                Partition {
                    id: "root-a".to_owned(),
                    size: 900.into(),
                    partition_type: PartitionType::Root,
                },
                Partition {
                    id: "root-b".to_owned(),
                    size: 9000.into(),
                    partition_type: PartitionType::Root,
                },
            ],
            ..Default::default()
        });

        // Add the required A/B update configuration
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        // Test case #0: If no mount points defined, should return an error.
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::GetRootMountPointInfo {
                root_path: ROOT_MOUNT_POINT_PATH.to_string()
            })
        );

        // Test case #1: If no root ID in block devices, should return an error.
        ctx.spec.storage.internal_mount_points = vec![InternalMountPoint {
            path: PathBuf::from("/"),
            target_id: "root".to_string(),
            filesystem: FileSystemType::Ext4,
            options: vec![],
        }];
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::GetBlockDevicePath {
                device_id: "root".to_string()
            })
        );

        // Test case #3: When block devices are defined, should return the expected root device
        // path of 'root-a'.
        ctx.block_device_paths = btreemap! {
            "os".to_owned() => PathBuf::from("/dev/sda"),
            "efi".to_owned() => PathBuf::from("/dev/sda1"),
            "root-a".to_owned() => PathBuf::from("/dev/sda2"),
            "root-b".to_owned() => PathBuf::from("/dev/sda3"),
        };
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda2")
        );

        // Test case #4: After rebooting after an A/B update, should return the expected root
        // device path of 'root-b'.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda3")
        );
    }

    /// Validates that get_expected_root_device_path() returns the expected root device path when
    /// root is a verity device.
    #[test]
    fn test_get_expected_root_device_path_verity() {
        let mut ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        // Add a disk and partitions
        ctx.spec.storage.disks.push(Disk {
            id: "os".to_owned(),
            device: PathBuf::from("/dev/disk/by-bus/foobar"),
            partitions: vec![
                Partition {
                    id: "esp".to_owned(),
                    size: 100.into(),
                    partition_type: PartitionType::Esp,
                },
                Partition {
                    id: "root-data-a".to_owned(),
                    size: 900.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root-data-b".to_owned(),
                    size: 9000.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root-hash-a".to_owned(),
                    size: 900.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root-hash-b".to_owned(),
                    size: 9000.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root".to_owned(),
                    size: 9000.into(),
                    partition_type: PartitionType::RootVerity,
                },
            ],
            ..Default::default()
        });

        // Add the required A/B update configuration
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![
                AbVolumePair {
                    id: "root-data".to_string(),
                    volume_a_id: "root-data-a".to_string(),
                    volume_b_id: "root-data-b".to_string(),
                },
                AbVolumePair {
                    id: "root-hash".to_string(),
                    volume_a_id: "root-hash-a".to_string(),
                    volume_b_id: "root-hash-b".to_string(),
                },
                AbVolumePair {
                    id: "trident-overlay".to_string(),
                    volume_a_id: "trident-overlay-a".to_string(),
                    volume_b_id: "trident-overlay-b".to_string(),
                },
            ],
        });

        // Update the block device paths
        ctx.block_device_paths = btreemap! {
            "os".to_owned() => PathBuf::from("/dev/sda"),
            "efi".to_owned() => PathBuf::from("/dev/sda1"),
            "root-data-a".to_owned() => PathBuf::from("/dev/sda2"),
            "root-data-b".to_owned() => PathBuf::from("/dev/sda3"),
            "root-hash-a".to_owned() => PathBuf::from("/dev/sda4"),
            "root-hash-b".to_owned() => PathBuf::from("/dev/sda5"),
            "trident-overlay-a".to_owned() => PathBuf::from("/dev/sda6"),
            "trident-overlay-b".to_owned() => PathBuf::from("/dev/sda7"),
        };

        // Add internal mount points
        ctx.spec.storage.internal_mount_points = vec![
            InternalMountPoint {
                path: PathBuf::from("/"),
                target_id: "root".to_string(),
                filesystem: FileSystemType::Ext4,
                options: vec![],
            },
            InternalMountPoint {
                path: PathBuf::from("/var/lib/trident-overlay"),
                target_id: "trident-overlay".to_string(),
                filesystem: FileSystemType::Ext4,
                options: vec![],
            },
        ];

        // Add verity file systems
        ctx.spec.storage.verity_filesystems = vec![VerityFileSystem {
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            data_image: Image {
                url: "http://example.com/root-data.img".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://example.com/root-hash.img".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            fs_type: FileSystemType::Ext4,
            mount_point: MountPoint {
                path: ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
            },
        }];

        // Test case #0: If no internal verity devices defined, should return an error.
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::GetRootVerityDeviceConfig)
        );

        // Test case #1. Add an internal verity device configuration. Should now correctly return
        // the expected root device path of 'root-data-a', since servicing type is CleanInstall.
        ctx.spec.storage.internal_verity = vec![InternalVerityDevice {
            id: "root".into(),
            device_name: "root".into(),
            data_target_id: "root-data".into(),
            hash_target_id: "root-hash".into(),
        }];

        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda2")
        );

        // Test case #2. Change active volume to VolumeA and servicing type to AbUpdate, and
        // validate that the expected root device path is now the verity data device path of
        // 'root-data-b'.
        ctx.servicing_type = ServicingType::AbUpdate;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda3")
        );

        // Test case #3. Change active volume to VolumeB and validate that the expected root device
        // path is now the verity data device path of 'root-data-a'.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda2")
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    use std::path::PathBuf;

    use const_format::formatcp;
    use maplit::btreemap;

    use osutils::testutils::{
        repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH},
        verity::{self, VerityGuard},
    };

    use trident_api::{
        config::{
            self, AbUpdate, AbVolumePair, Disk, FileSystemType, HostConfiguration, Image,
            ImageFormat, ImageSha256, InternalMountPoint, MountOptions, MountPoint, Partition,
            PartitionType, VerityFileSystem,
        },
        constants::MOUNT_OPTION_READ_ONLY,
        status::AbVolumeSelection,
    };

    #[functional_test]
    fn test_compare_root_device_paths() {
        // Test case #0: If current root device path is the same as the expected root device path,
        // should return true.
        assert!(
            compare_root_device_paths(PathBuf::from("/dev/sda2"), PathBuf::from("/dev/sda2"))
                .unwrap()
        );

        // Test case #1: If current root device path is NOT the same as the expected root device
        // path, should return false.
        assert!(
            !compare_root_device_paths(PathBuf::from("/dev/sda2"), PathBuf::from("/dev/sda3"))
                .unwrap()
        );
    }

    #[functional_test]
    fn test_get_plain_volume_pair_paths() {
        let mut ctx = EngineContext {
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            spec: HostConfiguration {
                storage: config::Storage {
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "root".to_string(),
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Failed to get block device path for volume A with ID 'root-a'"
        );

        ctx.spec.storage.disks = vec![Disk {
            id: "os".to_owned(),
            device: PathBuf::from("/dev/sda"),
            partition_table_type: config::PartitionTableType::Gpt,
            adopted_partitions: vec![],
            partitions: vec![Partition {
                id: "root-a".to_owned(),
                partition_type: PartitionType::Root,
                size: 100.into(),
            }],
        }];
        ctx.block_device_paths
            .insert("root-a".to_string(), PathBuf::from("/dev/sda1"));

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Failed to get block device path for volume B with ID 'root-b'"
        );

        ctx.spec
            .storage
            .disks
            .iter_mut()
            .find(|d| d.id == "os")
            .unwrap()
            .partitions
            .push(Partition {
                id: "root-b".to_owned(),
                partition_type: PartitionType::Root,
                size: 100.into(),
            });
        ctx.block_device_paths
            .insert("root-b".to_string(), PathBuf::from("/dev/sda2"));

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
            )
            .unwrap(),
            VolumePairPaths {
                volume_a_path: PathBuf::from("/dev/sda1"),
                volume_b_path: PathBuf::from("/dev/sda2"),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }
        );

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
            )
            .unwrap(),
            VolumePairPaths {
                volume_a_path: PathBuf::from("/dev/sda1"),
                volume_b_path: PathBuf::from("/dev/sda2"),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }
        );

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
            )
            .unwrap(),
            VolumePairPaths {
                volume_a_path: PathBuf::from("/dev/sda1"),
                volume_b_path: PathBuf::from("/dev/sda2"),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }
        );
    }

    #[functional_test]
    fn test_get_verity_data_volume_pair_paths() {
        let mut ab_update = AbUpdate {
            volume_pairs: vec![],
        };
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    ab_update: Some(ab_update.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case #0: If there is no internal verity device configuration, returns an error.
        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for root verity device 'root'"
        );

        ctx.spec = HostConfiguration {
            storage: config::Storage {
                internal_verity: vec![config::InternalVerityDevice {
                    id: "root".to_string(),
                    device_name: "root".to_string(),
                    data_target_id: "root-data".to_string(),
                    hash_target_id: "root-hash".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case #1: If there is no volume pair for the root data device, returns an error.
        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No volume pair for root data device found"
        );

        ab_update.volume_pairs = vec![
            AbVolumePair {
                id: "root-data".to_string(),
                volume_a_id: "root-data-a".to_string(),
                volume_b_id: "root-data-b".to_string(),
            },
            AbVolumePair {
                id: "root-hash".to_string(),
                volume_a_id: "root-hash-a".to_string(),
                volume_b_id: "root-hash-b".to_string(),
            },
        ];

        // Test case #2: If there are no block devices defined, returns an error.
        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device for data volume A with ID 'root-data-a'"
        );

        ctx.spec.storage.disks = vec![Disk {
            id: "os".into(),
            device: PathBuf::from("/dev/sda"),
            partition_table_type: config::PartitionTableType::Gpt,
            adopted_partitions: vec![],
            partitions: vec![Partition {
                id: "root-data-a".to_owned(),
                partition_type: PartitionType::Root,
                size: 100.into(),
            }],
        }];
        ctx.block_device_paths
            .insert("root-data-a".to_string(), PathBuf::from("/dev/sda1"));

        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device for data volume B with ID 'root-data-b'"
        );

        ctx.spec.storage.disks[0].partitions.push(Partition {
            id: "root-data-b".to_owned(),
            partition_type: PartitionType::Root,
            size: 100.into(),
        });
        ctx.block_device_paths
            .insert("root-data-b".to_string(), PathBuf::from("/dev/sda2"));

        // Test case #3: When information is complete, returns the volume pair paths.
        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root".to_owned()).unwrap(),
            VolumePairPaths {
                volume_a_path: PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1")),
                volume_b_path: PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
                volume_a_id: "root-data-a".to_string(),
                volume_b_id: "root-data-b".to_string(),
            }
        );
    }

    #[functional_test]
    fn test_get_root_verity_data_device_path() {
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case #0: Returns an error if info is missing.
        assert_eq!(
            get_root_verity_data_device_path(&ctx, &"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for root verity device 'root'"
        );

        // Test case #1. Add an internal verity device config and ensure it is returned.
        ctx.spec.storage.internal_verity = vec![config::InternalVerityDevice {
            id: "root".to_string(),
            device_name: "root".to_string(),
            data_target_id: "root-data".to_string(),
            hash_target_id: "root-hash".to_string(),
        }];

        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![
                AbVolumePair {
                    id: "root-data".to_string(),
                    volume_a_id: "root-data-a".to_string(),
                    volume_b_id: "root-data-b".to_string(),
                },
                AbVolumePair {
                    id: "root-hash".to_string(),
                    volume_a_id: "root-hash-a".to_string(),
                    volume_b_id: "root-hash-b".to_string(),
                },
            ],
        });

        ctx.spec.storage.disks = vec![Disk {
            id: "os".into(),
            device: PathBuf::from("/dev/sda"),
            partition_table_type: config::PartitionTableType::Gpt,
            adopted_partitions: vec![],
            partitions: vec![
                Partition {
                    id: "root-data-a".to_owned(),
                    partition_type: PartitionType::Root,
                    size: 100.into(),
                },
                Partition {
                    id: "root-data-b".to_owned(),
                    partition_type: PartitionType::Root,
                    size: 100.into(),
                },
                Partition {
                    id: "root-hash-a".to_owned(),
                    partition_type: PartitionType::Root,
                    size: 100.into(),
                },
                Partition {
                    id: "root-hash-b".to_owned(),
                    partition_type: PartitionType::Root,
                    size: 100.into(),
                },
            ],
        }];
        ctx.block_device_paths
            .insert("root-data-a".to_string(), PathBuf::from("/dev/sda1"));
        ctx.block_device_paths
            .insert("root-data-b".to_string(), PathBuf::from("/dev/sda2"));

        // Test case #2: Returns an error if the verity device is not active.
        let _ = veritysetup::close("root");
        assert!(get_root_verity_data_device_path(&ctx, &"root".to_owned())
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains("stdout:\n/dev/mapper/root is inactive.\n\n"));

        // Test case #3: Since the verity device is not yet active, we should get an error.
        let expected_root_hash = verity::setup_verity_volumes();

        let verity_device_path = Path::new("/dev/mapper/root");
        if verity_device_path.exists() {
            veritysetup::close("root").unwrap();
        }

        ctx.block_device_paths = btreemap! {
            "os".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
            "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
            "root-data-a".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
            "root-hash-a".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
            "boot2".into() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1")),
            "root-data-b".into() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
            "root-hash-b".into() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}3")),
            "root".into() => PathBuf::from("/dev/mapper/root"),
        };
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        assert!(get_root_verity_data_device_path(&ctx, &"root".to_owned())
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains("stdout:\n/dev/mapper/root is inactive.\n\n"));

        // Test case #4: Returns the path to the verity device, once it is open.
        veritysetup::open(
            formatcp!("{TEST_DISK_DEVICE_PATH}3"),
            "root",
            formatcp!("{TEST_DISK_DEVICE_PATH}2"),
            &expected_root_hash,
        )
        .unwrap();
        let _verityguard = VerityGuard {
            device_name: "root",
        };

        assert_eq!(
            get_root_verity_data_device_path(&ctx, &"root".to_owned()).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
        );

        // Test case #5: When the IDs are swapped, returns the correct path.
        ctx.spec.storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id =
            "root-data-b".to_string();

        ctx.spec.storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_b_id =
            "root-data-a".to_string();

        assert_eq!(
            get_root_verity_data_device_path(&ctx, &"root".to_owned()).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
        );
    }

    #[functional_test]
    fn test_get_root_verity_device_config() {
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case #0: If there is no internal verity device configuration, returns an error.
        assert_eq!(
            get_root_verity_device_config(&ctx, &"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for root verity device 'root'"
        );

        // Test case #1. Add an internal verity device config and ensure it is returned.
        ctx.spec.storage.internal_verity = vec![config::InternalVerityDevice {
            id: "root".to_string(),
            device_name: "root".to_string(),
            data_target_id: "root-data".to_string(),
            hash_target_id: "root-hash".to_string(),
        }];

        assert_eq!(
            get_root_verity_device_config(&ctx, &"root".to_owned()).unwrap(),
            config::InternalVerityDevice {
                id: "root".to_string(),
                device_name: "root".to_string(),
                data_target_id: "root-data".to_string(),
                hash_target_id: "root-hash".to_string(),
            }
        );

        // Test case #2: Requesting config for a non-existent device should return an error.
        assert_eq!(
            get_root_verity_device_config(&ctx, &"non-existent".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for root verity device 'non-existent'"
        );
    }

    #[functional_test]
    fn test_validate_active_volume() {
        // Test case #0: Missing ab_update.
        let mut ctx = EngineContext {
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            servicing_type: ServicingType::AbUpdate,
            ..Default::default()
        };

        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No A/B update found"
        );

        // Test case #1: Missing root mount point.
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "rootq".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No mount point for root volume found"
        );

        // Test case #2: Missing volume pair for root mount point.
        ctx.spec.storage.internal_mount_points = vec![InternalMountPoint {
            target_id: "root".to_string(),
            filesystem: FileSystemType::Ext4,
            options: vec![],
            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
        }];

        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No volume pair for root volume found"
        );

        // Test case #3: Missing block device for volume A.
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume A with ID 'root-a'"
        );

        // Test case #4: Missing block device for volume B.
        ctx.block_device_paths = btreemap! {
            "root-a".to_owned() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}15")),
        };

        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume B with ID 'root-b'"
        );

        ctx.block_device_paths.insert(
            "root-b".to_owned(),
            PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
        );

        // Test case #5: Volume A path cannot be resolved.
        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}3")))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No such file or directory (os error 2)"
        );

        // Test case #6: A or B paths do not match the root volume path.
        *ctx.block_device_paths.get_mut("root-a").unwrap() =
            PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1"));

        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}3")))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to match current root device path '/dev/sda3' to either volume A path '/dev/sda1' or volume B path '/dev/sda2'"
        );

        // Test case #7: Volume A is the root device path; active volume is set correctly to volume A.
        validate_active_volume(&ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1"))).unwrap();

        // Test case #8: Volume B is the root device path; active volume is set correctly to volume B.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        validate_active_volume(&ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2"))).unwrap();

        // Test case #9: Volume A is the root device path; active volume is incorrectly set to B.
        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1")))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Volume A is active, but active volume in Host Status is set to Volume B"
        );

        // Verity tests. Set up verity devices.
        let expected_root_hash = verity::setup_verity_volumes();

        let verity_device_path = Path::new("/dev/mapper/root");
        if verity_device_path.exists() {
            veritysetup::close("root").unwrap();
        }
        veritysetup::open(
            formatcp!("{TEST_DISK_DEVICE_PATH}3"),
            "root",
            formatcp!("{TEST_DISK_DEVICE_PATH}2"),
            &expected_root_hash,
        )
        .unwrap();
        let _verityguard = VerityGuard {
            device_name: "root",
        };

        ctx.block_device_paths = btreemap! {
            "root-a".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
            "root-b".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
        };

        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        validate_active_volume(&ctx, PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))).unwrap();

        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        validate_active_volume(&ctx, PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"))).unwrap();

        ctx.block_device_paths
            .insert("root".to_string(), PathBuf::from("/dev/mapper/root"));
        ctx.spec.storage.verity_filesystems = vec![VerityFileSystem {
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            data_image: Image {
                url: "http://example.com/root-data.img".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://example.com/root-hash.img".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            fs_type: FileSystemType::Ext4,
            mount_point: MountPoint {
                path: ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
            },
        }];
        ctx.spec.storage.internal_verity = vec![config::InternalVerityDevice {
            id: "root".to_string(),
            device_name: "root".to_string(),
            data_target_id: "root-data".to_string(),
            hash_target_id: "root-hash".to_string(),
        }];
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![
                AbVolumePair {
                    id: "root-data".to_string(),
                    volume_a_id: "root-a".to_string(),
                    volume_b_id: "root-b".to_string(),
                },
                AbVolumePair {
                    id: "root-hash".to_string(),
                    volume_a_id: "root-hash-a".to_string(),
                    volume_b_id: "root-hash-b".to_string(),
                },
            ],
        });

        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        validate_active_volume(&ctx, PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"))).unwrap();
    }
}
