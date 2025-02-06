use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Error};
use log::{debug, info, trace, warn};

use osutils::{block_devices, lsblk, veritysetup};

use trident_api::{
    config::{AbUpdate, VerityDevice},
    constants::{internal_params::VIRTDEPLOY_BOOT_ORDER_WORKAROUND, ROOT_MOUNT_POINT_PATH},
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
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
        servicing_type: ServicingType::AbUpdate,
        ab_active_volume: datastore.host_status().ab_active_volume,
        partition_paths: datastore.host_status().partition_paths.clone(),
        disk_uuids: datastore.host_status().disk_uuids.clone(),
        install_index: datastore.host_status().install_index,
        os_image: None, // Not used for boot validation logic
        storage_graph: engine::build_storage_graph(&datastore.host_status().spec.storage)?, // Build storage graph
    };

    // Get the block device path of the current root
    let current_root_path =
        get_current_root_device_path(&ctx).message("Failed to get root block device path")?;

    // Get expected root device path
    let expected_root_path =
        get_expected_root_device_path(&ctx).message("Failed to get expected root device path")?;

    if compare_root_device_paths(current_root_path.clone(), expected_root_path.clone())
        .message("Host failed to boot from expected root device")?
    {
        info!("Host successfully booted from updated runtime OS image");

        // If it's virtdeploy, after confirming that we have booted into the correct image, we need
        // to update the `BootOrder` to boot from the correct image next time.
        let use_virtdeploy_workaround = osutils::virt::is_virtdeploy()
            || ctx
                .spec
                .internal_params
                .get_flag(VIRTDEPLOY_BOOT_ORDER_WORKAROUND);

        // Persist the boot order change
        if datastore.host_status().servicing_state == ServicingState::AbUpdateFinalized
            || use_virtdeploy_workaround
        {
            bootentries::persist_boot_order()
                .message("Failed to persist boot order after reboot")?;
        }
    } else if datastore.host_status().servicing_state == ServicingState::CleanInstallStaged
        || datastore.host_status().servicing_state == ServicingState::CleanInstallFinalized
    {
        // If Trident was executing a clean install, need to re-set the Host Status.
        datastore.with_host_status(|host_status| {
            host_status.spec = Default::default();
            host_status.servicing_state = ServicingState::NotProvisioned;
        })?;

        return Err(TridentError::new(ServicingError::CleanInstallRebootCheck {
            root_device_path: current_root_path.to_string_lossy().to_string(),
            expected_device_path: expected_root_path.to_string_lossy().to_string(),
        }));
    } else {
        // If Trident was executing an A/B update, need to re-set the Host Status.
        datastore.with_host_status(|host_status| {
            host_status.spec = host_status.spec_old.clone();
            host_status.spec_old = Default::default();
            host_status.servicing_state = ServicingState::Provisioned;
        })?;

        return Err(TridentError::new(ServicingError::AbUpdateRebootCheck {
            root_device_path: current_root_path.to_string_lossy().to_string(),
            expected_device_path: expected_root_path.to_string_lossy().to_string(),
        }));
    }

    match datastore.host_status().servicing_state {
        ServicingState::CleanInstallFinalized => {
            info!("Clean install of runtime OS succeeded");
            tracing::info!(metric_name = "clean_install_success", value = true);
        }
        ServicingState::AbUpdateFinalized => {
            info!("A/B update succeeded");
            tracing::info!(metric_name = "ab_update_success", value = true);
        }
        // Because the boot validation logic is currently called only on clean install and A/B
        // update, this should be unreachable.
        // TODO: When/If `UpdateAndReboot` is used, this should be updated.
        state => {
            return Err(TridentError::new(InternalError::UnexpectedServicingState {
                state,
            }))
            .message("Validate boot failed");
        }
    }

    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::Provisioned
    );

    datastore.with_host_status(|host_status| {
        host_status.servicing_state = ServicingState::Provisioned;
        host_status.spec_old = Default::default();
        host_status.ab_active_volume = match host_status.ab_active_volume {
            None | Some(AbVolumeSelection::VolumeB) => Some(AbVolumeSelection::VolumeA),
            Some(AbVolumeSelection::VolumeA) => Some(AbVolumeSelection::VolumeB),
        };
    })?;

    Ok(())
}

/// Returns the current root device path, i.e., the path of the root block device that the host
/// booted from. The path is given in its canonical form.
fn get_current_root_device_path(ctx: &EngineContext) -> Result<PathBuf, TridentError> {
    // If root is on verity, will need to use 'veritysetup' to get the data device path.
    let current_root_device_path = if ctx.storage_graph.root_fs_is_verity() {
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
        // No verity device found, fallback to old logic.
        block_devices::get_root_device_path()?
    };

    // Try to canonicalize the path
    let root_path_canonical = match current_root_device_path.canonicalize() {
        Ok(canonical_path) => {
            // If the paths are different, log both
            if canonical_path != current_root_device_path {
                info!(
                    "Current root device path: '{}' ('{}')",
                    canonical_path.display(),
                    current_root_device_path.display(),
                );
            } else if let Some(partuuid_path) =
                construct_by_partuuid_path(&current_root_device_path)
            {
                // If they are the same, try to construct the by-partuuid path
                info!(
                    "Current root device path: '{}' ('{}')",
                    canonical_path.display(),
                    partuuid_path.display(),
                );
            } else {
                info!("Current root device path: '{}'", canonical_path.display(),);
            }
            canonical_path
        }
        Err(err) => {
            warn!(
                "Failed to canonicalize root device path '{}': {}",
                current_root_device_path.display(),
                err
            );

            // Attempt to construct the by-partuuid path
            if let Some(partuuid_path) = construct_by_partuuid_path(&current_root_device_path) {
                info!(
                    "Current root device path: '{}' ('{}')",
                    current_root_device_path.display(),
                    partuuid_path.display(),
                );
            }

            current_root_device_path.clone()
        }
    };

    Ok(root_path_canonical)
}

/// Returns the path of the root device that the host was expected to boot from. The path is given
/// in its canonical form.
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

    let expected_root_path = if ctx.storage_graph.root_fs_is_verity() {
        // If root is on verity, fetch the block device path of the verity data device. Because
        // get_block_device_path(), which is called eventually, already has the logic for
        // determining the update volume, i.e. volume we expect to have booted from, getting the
        // block device path of the verity data device is sufficient.
        let root_verity_device_config = get_verity_config(ctx, root_device_id)
            .structured(ServicingError::GetRootVerityDeviceConfig)?;

        let (verity_data_path, _) =
            verity::get_verity_device_paths(ctx, &root_verity_device_config)
                .structured(ServicingError::GetRootVerityDataDevPath)?;

        verity_data_path
    } else {
        // Fetch the expected root device path
        ctx.get_block_device_path(root_device_id).structured(
            ServicingError::GetBlockDevicePath {
                device_id: root_device_id.to_string(),
            },
        )?
    };

    // Try to canonicalize the path
    let root_path_canonical = match expected_root_path.canonicalize() {
        Ok(canonical_path) => {
            if canonical_path != expected_root_path {
                info!(
                    "Expected root device path: '{}' ('{}')",
                    canonical_path.display(),
                    expected_root_path.display(),
                );
            } else if let Some(partuuid_path) = construct_by_partuuid_path(&expected_root_path) {
                info!(
                    "Expected root device path: '{}' ('{}')",
                    canonical_path.display(),
                    partuuid_path.display(),
                );
            } else {
                info!("Expected root device path: '{}'", canonical_path.display(),);
            }
            canonical_path
        }
        Err(err) => {
            warn!(
                "Failed to canonicalize root device path '{}': {}",
                expected_root_path.display(),
                err
            );

            if let Some(partuuid_path) = construct_by_partuuid_path(&expected_root_path) {
                info!(
                    "Current root device path: '{}' ('{}')",
                    expected_root_path.display(),
                    partuuid_path.display(),
                );
            }
            expected_root_path.clone()
        }
    };

    Ok(root_path_canonical)
}

/// Returns the by-partuuid path of the given device path, if it exists.
fn construct_by_partuuid_path(device_path: &PathBuf) -> Option<PathBuf> {
    if let Ok(block_device) = lsblk::get(device_path) {
        if let Some(part_uuid) = block_device.part_uuid {
            return Some(PathBuf::from(format!(
                "/dev/disk/by-partuuid/{}",
                part_uuid
            )));
        }
    }

    None
}

/// Compares the expected root device path with the current root device path that the host booted
/// from. Returns true if they match; false otherwise.
fn compare_root_device_paths(
    root_dev_path: PathBuf,
    expected_root_dev_path: PathBuf,
) -> Result<bool, TridentError> {
    // If current root device path is NOT the same as the expected root device path, return false.
    if root_dev_path != expected_root_dev_path {
        return Ok(false);
    }

    info!(
        "Host booted from expected root device '{}'",
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

    let (volume_pair_paths, root_data_device_path) = if ctx.storage_graph.root_fs_is_verity() {
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

    let volume_a_path = ctx
        .get_block_device_path(&root_device_pair.volume_a_id)
        .context(format!(
            "Failed to get block device path for volume A with ID '{}'",
            root_device_pair.volume_a_id
        ))?;
    let volume_b_path = ctx
        .get_block_device_path(&root_device_pair.volume_b_id)
        .context(format!(
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
    let root_verity_device_config = get_verity_config(ctx, root_device_id)?;

    let root_data_device_pair = ab_update
        .volume_pairs
        .iter()
        .find(|vp| vp.id == root_verity_device_config.data_device_id)
        .context("No volume pair for root data device found")?;
    debug!("Root data device pair: {:?}", root_data_device_pair);

    let volume_a_path = ctx
        .get_block_device_path(&root_data_device_pair.volume_a_id)
        .context(format!(
            "Failed to get block device for data volume A with ID '{}'",
            &root_data_device_pair.volume_a_id
        ))?;
    let volume_b_path = ctx
        .get_block_device_path(&root_data_device_pair.volume_b_id)
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
    let root_verity_device_config = get_verity_config(ctx, root_device_id).context(format!(
        "Failed to get configuration for root verity device '{}'",
        root_device_id
    ))?;

    // Run 'veritysetup' to get the data device path
    let root_verity_status =
        veritysetup::status(&root_verity_device_config.name).context(format!(
            "Failed to get verity status for device '{}'",
            root_verity_device_config.name
        ))?;
    trace!("Root verity status: {:?}", root_verity_status);

    Ok(root_verity_status.data_device_path)
}

/// Gets the configuration for the root verity device for the given root device ID.
///
/// TODO: Remove old verity API.
pub fn get_verity_config(
    ctx: &EngineContext,
    root_device_id: &BlockDeviceId,
) -> Result<VerityDevice, Error> {
    // Prefer old API: Try to get the config from internal_verity first. Then, check the new API
    let root_verity_device_config = ctx
        .spec
        .storage
        .internal_verity
        .iter()
        .chain(ctx.spec.storage.verity.iter())
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

    use const_format::formatcp;
    use maplit::btreemap;

    use osutils::testutils::repart::TEST_DISK_DEVICE_PATH;

    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystemType, Image, ImageFormat, ImageSha256,
            InternalMountPoint, MountOptions, MountPoint, Partition, PartitionType,
            VerityFileSystem,
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
        ctx.partition_paths = btreemap! {
            "os".to_owned() => PathBuf::from(TEST_DISK_DEVICE_PATH),
            "efi".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
            "root-a".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
            "root-b".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
        };
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"))
        );

        // Test case #4: After rebooting after an A/B update, should return the expected root
        // device path of 'root-b'.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
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
                    size: 4096.into(),
                    partition_type: PartitionType::Esp,
                },
                Partition {
                    id: "root-data-a".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::Root,
                },
                Partition {
                    id: "root-data-b".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::Root,
                },
                Partition {
                    id: "root-hash-a".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root-hash-b".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "trident-overlay-a".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::LinuxGeneric,
                },
                Partition {
                    id: "trident-overlay-b".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::LinuxGeneric,
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
        ctx.partition_paths = btreemap! {
            "os".to_owned() => PathBuf::from(TEST_DISK_DEVICE_PATH),
            "esp".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
            "root-data-a".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
            "root-data-b".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
            "root-hash-a".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
            "root-hash-b".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}5")),
            "trident-overlay-a".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}6")),
            "trident-overlay-b".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}7")),
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

        // Add verity file systems per old API
        // TODO: Remove old verity API!
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

        // Build storage graph
        ctx.storage_graph = ctx.spec.storage.build_graph().unwrap();

        // Test case #0: If no internal verity devices defined, should return an error
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::GetRootVerityDeviceConfig)
        );

        // Test case #1. Add an internal verity device configuration. Should now correctly return
        // the expected root device path of 'root-data-a', since servicing type is CleanInstall.
        ctx.spec.storage.internal_verity = vec![VerityDevice {
            id: "root".into(),
            name: "root".into(),
            data_device_id: "root-data".into(),
            hash_device_id: "root-hash".into(),
            ..Default::default()
        }];

        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"))
        );

        // Test case #2. Change active volume to VolumeA and servicing type to AbUpdate, and
        // validate that the expected root device path is now the verity data device path of
        // 'root-data-b'.
        ctx.servicing_type = ServicingType::AbUpdate;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
        );

        // Test case #3. Change active volume to VolumeB and validate that the expected root device
        // path is now the verity data device path of 'root-data-a'.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"))
        );

        // Test case #4. Remove internal verity device configuration and add a verity device
        // configuration. Should still correctly return.
        ctx.spec.storage.internal_verity = vec![];
        ctx.spec.storage.verity = vec![VerityDevice {
            id: "root".into(),
            name: "root".into(),
            data_device_id: "root-data".into(),
            hash_device_id: "root-hash".into(),
            ..Default::default()
        }];

        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"))
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
        assert!(compare_root_device_paths(
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"))
        )
        .unwrap());

        // Test case #1: If current root device path is NOT the same as the expected root device
        // path, should return false.
        assert!(!compare_root_device_paths(
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
        )
        .unwrap());
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
            device: PathBuf::from(TEST_DISK_DEVICE_PATH),
            partition_table_type: config::PartitionTableType::Gpt,
            adopted_partitions: vec![],
            partitions: vec![Partition {
                id: "root-a".to_owned(),
                partition_type: PartitionType::Root,
                size: 100.into(),
            }],
        }];
        ctx.partition_paths.insert(
            "root-a".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
        );

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
        ctx.partition_paths.insert(
            "root-b".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
        );

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
            )
            .unwrap(),
            VolumePairPaths {
                volume_a_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                volume_b_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
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
                volume_a_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                volume_b_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
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
                volume_a_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                volume_b_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
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
                internal_verity: vec![VerityDevice {
                    id: "root".to_string(),
                    name: "root".to_string(),
                    data_device_id: "root-data".to_string(),
                    hash_device_id: "root-hash".to_string(),
                    ..Default::default()
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
            device: PathBuf::from(TEST_DISK_DEVICE_PATH),
            partition_table_type: config::PartitionTableType::Gpt,
            adopted_partitions: vec![],
            partitions: vec![Partition {
                id: "root-data-a".to_owned(),
                partition_type: PartitionType::Root,
                size: 100.into(),
            }],
        }];
        ctx.partition_paths.insert(
            "root-data-a".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
        );

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
        ctx.partition_paths.insert(
            "root-data-b".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
        );

        // Test case #3: When information is complete, returns the volume pair paths.
        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root".to_owned()).unwrap(),
            VolumePairPaths {
                volume_a_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                volume_b_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                volume_a_id: "root-data-a".to_string(),
                volume_b_id: "root-data-b".to_string(),
            }
        );

        // Test case #4: Remove the internal verity device configuration and add a verity device
        // configuration. Should still correctly return.
        ctx.spec.storage.internal_verity = vec![];
        ctx.spec.storage.verity = vec![VerityDevice {
            id: "root".into(),
            name: "root".into(),
            data_device_id: "root-data".into(),
            hash_device_id: "root-hash".into(),
            ..Default::default()
        }];

        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root".to_owned()).unwrap(),
            VolumePairPaths {
                volume_a_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                volume_b_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
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
        ctx.spec.storage.internal_verity = vec![VerityDevice {
            id: "root".to_string(),
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            ..Default::default()
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
            device: PathBuf::from(TEST_DISK_DEVICE_PATH),
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
        ctx.partition_paths.insert(
            "root-data-a".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
        );
        ctx.partition_paths.insert(
            "root-data-b".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
        );

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

        ctx.partition_paths = btreemap! {
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

        // Test case #6: Remove the internal verity device configuration and add a verity device
        // configuration. Should still correctly return.
        ctx.spec.storage.internal_verity = vec![];
        ctx.spec.storage.verity = vec![VerityDevice {
            id: "root".into(),
            name: "root".into(),
            data_device_id: "root-data".into(),
            hash_device_id: "root-hash".into(),
            ..Default::default()
        }];

        assert_eq!(
            get_root_verity_data_device_path(&ctx, &"root".to_owned()).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
        );
    }

    #[functional_test]
    fn test_get_verity_config() {
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
            get_verity_config(&ctx, &"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for root verity device 'root'"
        );

        // Test case #1. Add an internal verity device config and ensure it is returned.
        ctx.spec.storage.internal_verity = vec![VerityDevice {
            id: "root".to_string(),
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            ..Default::default()
        }];

        assert_eq!(
            get_verity_config(&ctx, &"root".to_owned()).unwrap(),
            VerityDevice {
                id: "root".to_string(),
                name: "root".to_string(),
                data_device_id: "root-data".to_string(),
                hash_device_id: "root-hash".to_string(),
                ..Default::default()
            }
        );

        // Test case #2: Requesting config for a non-existent device should return an error.
        assert_eq!(
            get_verity_config(&ctx, &"non-existent".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for root verity device 'non-existent'"
        );

        // Test case #3: If there is no internal verity device configuration, check for a verity
        // device configuration (new API) and ensure it is returned.
        ctx.spec.storage.internal_verity = vec![];
        ctx.spec.storage.verity = vec![VerityDevice {
            id: "root".to_string(),
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            ..Default::default()
        }];

        assert_eq!(
            get_verity_config(&ctx, &"root".to_owned()).unwrap(),
            VerityDevice {
                id: "root".to_string(),
                name: "root".to_string(),
                data_device_id: "root-data".to_string(),
                hash_device_id: "root-hash".to_string(),
                ..Default::default()
            }
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
            validate_active_volume(&ctx, PathBuf::from(OS_DISK_DEVICE_PATH))
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
            validate_active_volume(&ctx, PathBuf::from(OS_DISK_DEVICE_PATH))
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
            validate_active_volume(&ctx, PathBuf::from(OS_DISK_DEVICE_PATH))
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
            validate_active_volume(&ctx, PathBuf::from(OS_DISK_DEVICE_PATH))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume A with ID 'root-a'"
        );

        // Test case #4: Missing block device for volume B.
        ctx.partition_paths = btreemap! {
            "root-a".to_owned() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}15")),
        };

        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from(OS_DISK_DEVICE_PATH))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume B with ID 'root-b'"
        );

        ctx.partition_paths.insert(
            "root-b".to_owned(),
            PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
        );

        // Test case #5: Volume A path cannot be resolved.
        assert_eq!(
            validate_active_volume(&ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No such file or directory (os error 2)"
        );

        // Test case #6: A or B paths do not match the root volume path.
        *ctx.partition_paths.get_mut("root-a").unwrap() =
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

        // Test case #10: Set up verity devices to test a scenario where root is an A/B volume on
        // verity.
        let expected_root_hash = verity::setup_verity_volumes();

        let verity_device_path = Path::new("/dev/mapper/root");
        if verity_device_path.exists() {
            veritysetup::close("root").unwrap();
        }
        veritysetup::open(
            formatcp!("{TEST_DISK_DEVICE_PATH}1"), // Data device path
            "root",
            formatcp!("{TEST_DISK_DEVICE_PATH}2"), // Hash device path
            &expected_root_hash,
        )
        .unwrap();
        let _verityguard = VerityGuard {
            device_name: "root",
        };

        // Add partitions
        ctx.partition_paths = btreemap! {
            "os".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
            "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}5")),
            "root-data-a".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
            "root-hash-a".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
            "root-data-b".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
            "root-hash-b".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
            "root".into() => PathBuf::from("/dev/mapper/root"),
        };
        ctx.spec.storage.disks.push(Disk {
            id: "os".to_owned(),
            device: PathBuf::from(TEST_DISK_DEVICE_PATH),
            partitions: vec![
                Partition {
                    id: "esp".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::Esp,
                },
                Partition {
                    id: "root-data-a".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::Root,
                },
                Partition {
                    id: "root-data-b".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::Root,
                },
                Partition {
                    id: "root-hash-a".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root-hash-b".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "trident-overlay-a".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::LinuxGeneric,
                },
                Partition {
                    id: "trident-overlay-b".to_owned(),
                    size: 4096.into(),
                    partition_type: PartitionType::LinuxGeneric,
                },
            ],
            ..Default::default()
        });

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

        // Add verity file systems per old API.
        // TODO: Remove old verity API!
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
        ctx.spec.storage.internal_verity = vec![VerityDevice {
            id: "root".to_string(),
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            ..Default::default()
        }];

        // Build storage graph to validate
        ctx.storage_graph = ctx.spec.storage.build_graph().unwrap();

        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        validate_active_volume(&ctx, PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1"))).unwrap();

        // Test case #11: Remove the old verity API and add a verity device configuration.
        // Should still correctly return.
        ctx.spec.storage.internal_verity = vec![];
        ctx.spec.storage.verity_filesystems = vec![];
        ctx.spec.storage.verity = vec![VerityDevice {
            id: "root".into(),
            name: "root".into(),
            data_device_id: "root-data".into(),
            hash_device_id: "root-hash".into(),
            ..Default::default()
        }];

        validate_active_volume(&ctx, PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1"))).unwrap();
    }

    #[functional_test]
    fn test_construct_by_partuuid_path() {
        // Test with a valid device path having a part_uuid
        let device_path = PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1"));
        construct_by_partuuid_path(&device_path).unwrap();

        // Test with an invalid device path
        let device_path = PathBuf::from("/dev/invalid");
        assert_eq!(construct_by_partuuid_path(&device_path), None);
    }
}
