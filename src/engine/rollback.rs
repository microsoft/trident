use std::path::PathBuf;

use anyhow::{Context, Error};
use log::{debug, info, trace, warn};

use osutils::{block_devices, container, efivar, lsblk, pcrlock, veritysetup, virt};
use sysdefs::tpm2::Pcr;

use trident_api::{
    constants::internal_params::{ENABLE_UKI_SUPPORT, VIRTDEPLOY_BOOT_ORDER_WORKAROUND},
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{AbVolumeSelection, ServicingState, ServicingType},
    BlockDeviceId,
};

use crate::{
    engine::{
        self, bootentries,
        context::EngineContext,
        storage::{encryption, verity},
    },
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
        image: None, // Not used for boot validation logic
        storage_graph: engine::build_storage_graph(&datastore.host_status().spec.storage)?, // Build storage graph
        filesystems: Vec::new(), // Left empty since context does not have image
        is_uki: Some(
            datastore
                .host_status()
                .spec
                .internal_params
                .get_flag(ENABLE_UKI_SUPPORT),
        ),
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
        let use_virtdeploy_workaround = virt::is_virtdeploy()
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

        // If the bootloader set the LoaderEntrySelected variable, then make its value the default
        // boot entry. Systemd-boot sets this variable, but GRUB does not.
        if efivar::current_var_set() {
            efivar::set_default_to_current()
                .message("Failed to set default boot entry to current")?;
        }

        // If this is a UKI image, then we need to re-generate pcrlock policy to include current
        // boot only.
        //
        // TODO: Remove this override once UKI & encryption tests are fixed. Related ADO:
        // https://dev.azure.com/mariner-org/polar/_workitems/edit/13344/.
        let override_pcrlock_encryption = ctx
            .spec
            .internal_params
            .get_flag("overridePcrlockEncryption")
            || container::is_running_in_container()?;
        if ctx.is_uki_image()? && ctx.spec.storage.encryption.is_some() {
            if !override_pcrlock_encryption {
                debug!("Regenerating pcrlock policy for current boot");
                // TODO: Add PCR 7 once SecureBoot is enabled in a follow up PR. Related ADO task:
                // https://dev.azure.com/mariner-org/polar/_workitems/edit/14286/.
                let pcrs = Pcr::Pcr4 | Pcr::Pcr11;

                // Get UKI and bootloader binaries for .pcrlock file generation
                let (uki_binaries, bootloader_binaries) =
                    encryption::get_binary_paths_pcrlock(&ctx, None)
                        .structured(ServicingError::GetBinaryPathsForPcrlockEncryption)?;

                // Generate a pcrlock policy
                pcrlock::generate_pcrlock_policy(pcrs, uki_binaries, bootloader_binaries)?;
            } else {
                warn!(
                    "Skipping pcrlock policy generation because overridePcrlockEncryption is set or running in a container"
                );
            }
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
            }));
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
    let current_root_device_path = if ctx.storage_graph.root_fs_is_verity() {
        let root_device_id = ctx
            .get_root_block_device_id()
            .structured(ServicingError::GetRootBlockDeviceId)?;
        debug!("Root device ID: {}", root_device_id);

        // Fetch the actual verity data device path in the system
        get_verity_data_device_path(ctx, &root_device_id)
            .structured(ServicingError::GetRootVerityDataDevPath)?
    } else {
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
        .get_root_block_device_id()
        .structured(ServicingError::GetRootBlockDeviceId)?;

    let expected_root_path = if ctx.storage_graph.root_fs_is_verity() {
        // If root is on verity, fetch the block device path of the verity data device. Because
        // get_block_device_path(), which is called eventually, already has the logic for
        // determining the update volume, i.e. volume we expect to have booted from, getting the
        // block device path of the verity data device is sufficient.
        let root_verity_device_config = ctx.get_verity_config(&root_device_id).structured(
            // This should never happen. At this point we know the root device
            // is a verity device and we've already found the BlockDeviceId of
            // the verity device, so this search MUST always succeed, otherwise
            // the graph is severely corrupted.
            InternalError::Internal(
                "Graph is invalid: verity config for root device could not be found.",
            ),
        )?;

        let (verity_data_path, _) =
            verity::get_verity_device_paths(ctx, &root_verity_device_config)
                .structured(ServicingError::GetRootVerityDataDevPath)?;

        verity_data_path
    } else {
        // Fetch the expected root device path
        ctx.get_block_device_path(&root_device_id).structured(
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
            return Some(PathBuf::from(format!("/dev/disk/by-partuuid/{part_uuid}")));
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

/// Validates that the A/B active volume is set correctly.
///
/// This function is called before starting any update servicing, to confirm that the A/B active
/// volume set in the Host Status/context accurately reflects the actual active volume, i.e., the
/// device that the firmware booted from.
pub(crate) fn validate_ab_active_volume(ctx: &EngineContext) -> Result<(), TridentError> {
    let root_device_path = block_devices::get_root_device_path()?;
    validate_ab_active_volume_internal(ctx, root_device_path)
}

/// This is an internal helper function that:
/// - Fetches paths of A/B volumes,
/// - In case that root is a verity device, fetches root verity data device path,
/// - Validates that A/B active volume in engine context matches actual root device path.
fn validate_ab_active_volume_internal(
    ctx: &EngineContext,
    root_device_path: PathBuf,
) -> Result<(), TridentError> {
    let root_device_id = ctx
        .get_root_block_device_id()
        .ok_or_else(|| TridentError::new(ServicingError::GetRootBlockDeviceId))?;

    let (root_volume_pair, root_data_device_path) = if ctx.storage_graph.root_fs_is_verity() {
        debug!("Root is a verity device");

        // If root is a verity device, need to first fetch the device ID of the data device
        let verity_device_config = ctx.get_verity_config(&root_device_id).structured(
            // This should never happen. At this point we know the root device
            // is a verity device and we've already found the BlockDeviceId of
            // the verity device, so this search MUST always succeed, otherwise
            // the graph is severely corrupted.
            InternalError::Internal(
                "Graph is invalid: verity config for root device could not be found.",
            ),
        )?;

        // Fetch the A/B volume pair for the verity data device
        let volume_pair = ctx
            .get_ab_volume_pair(&verity_device_config.data_device_id)
            .structured(ServicingError::GetRootAbVolumePair {
                device_id: root_device_id.clone(),
            })?;

        // Fetch the actual verity data device path in the system
        let root_data_device_path = get_verity_data_device_path(ctx, &root_device_id)
            .structured(ServicingError::GetRootVerityDataDevPath)?;

        (volume_pair, root_data_device_path)
    } else {
        debug!("Root is not on verity");

        // Fetch the A/B volume pair for the root device
        let volume_pair = ctx.get_ab_volume_pair(&root_device_id).structured(
            ServicingError::GetRootAbVolumePair {
                device_id: root_device_id,
            },
        )?;

        (volume_pair, root_device_path)
    };

    debug!(
        "Root volume A path: {} (device ID: {})",
        root_volume_pair.volume_a_path.display(),
        root_volume_pair.volume_a_id
    );
    debug!(
        "Root volume B path: {} (device ID: {})",
        root_volume_pair.volume_b_path.display(),
        root_volume_pair.volume_b_id
    );

    let volume_a_path_canonical = root_volume_pair.volume_a_path.canonicalize().structured(
        ServicingError::CanonicalizePath {
            path: root_volume_pair.volume_a_path.display().to_string(),
        },
    )?;
    let volume_b_path_canonical = root_volume_pair.volume_b_path.canonicalize().structured(
        ServicingError::CanonicalizePath {
            path: root_volume_pair.volume_b_path.display().to_string(),
        },
    )?;

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
            return Err(TridentError::new(ServicingError::ValidateAbActiveVolume {
                active_volume: AbVolumeSelection::VolumeA.to_string(),
                hs_active_volume: ctx
                    .ab_active_volume
                    .map_or("None".to_string(), |v| v.to_string()),
            }));
        }
    } else if volume_b_path_canonical == root_data_device_path {
        debug!(
            "Current root device path '{}' matches root volume B path '{}'",
            root_data_device_path.display(),
            volume_b_path_canonical.display()
        );

        if ctx.ab_active_volume != Some(AbVolumeSelection::VolumeB) {
            return Err(TridentError::new(ServicingError::ValidateAbActiveVolume {
                active_volume: AbVolumeSelection::VolumeB.to_string(),
                hs_active_volume: ctx
                    .ab_active_volume
                    .map_or("None".to_string(), |v| v.to_string()),
            }));
        }
    } else {
        return Err(TridentError::new(
            ServicingError::RootDevicePathAbActiveVolumeMismatch {
                root_device_path: root_data_device_path.to_string_lossy().to_string(),
                root_volume_a_path: volume_a_path_canonical.to_string_lossy().to_string(),
                root_volume_b_path: volume_b_path_canonical.to_string_lossy().to_string(),
            },
        ));
    }

    Ok(())
}

/// Returns the path of the verity data device for the given block device ID. Uses the
/// `veritysetup` utility to fetch the actual data device path in the system.
fn get_verity_data_device_path(
    ctx: &EngineContext,
    device_id: &BlockDeviceId,
) -> Result<PathBuf, Error> {
    let verity_device_config = ctx.get_verity_config(device_id).context(format!(
        "Failed to get configuration for verity device '{device_id}'"
    ))?;

    // Run veritysetup to get the data device path
    let verity_status = veritysetup::status(&verity_device_config.name)
        .with_context(|| {
            format!(
                "Failed to get verity status for device '{}'",
                verity_device_config.name
            )
        })?
        .active()
        .with_context(|| {
            format!(
                "Verity device '{}' is not active.",
                verity_device_config.name
            )
        })?;

    trace!(
        "Verity status for verity device '{}' with block device ID '{}': {:?}",
        verity_device_config.name,
        device_id,
        verity_status
    );

    Ok(verity_status.data_device_path)
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
            AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, MountOptions, MountPoint,
            Partition, PartitionType, VerityDevice,
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
            &ErrorKind::Servicing(ServicingError::GetRootBlockDeviceId)
        );

        // Test case #1: If no root ID in block devices, should return an error.
        ctx.spec.storage.filesystems = vec![FileSystem {
            mount_point: Some(MountPoint {
                path: PathBuf::from("/"),
                options: MountOptions::empty(),
            }),
            device_id: Some("root".to_string()),
            source: FileSystemSource::Image,
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

        // Add verity dev
        ctx.spec.storage.verity = vec![VerityDevice {
            id: "root".into(),
            name: "root".into(),
            data_device_id: "root-data".into(),
            hash_device_id: "root-hash".into(),
            ..Default::default()
        }];

        // Add root FS
        ctx.spec.storage.filesystems = vec![FileSystem {
            device_id: Some("root".into()),
            mount_point: Some(MountPoint {
                path: PathBuf::from("/"),
                options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
            }),
            source: FileSystemSource::Image,
        }];

        // Build storage graph
        ctx.storage_graph = ctx.spec.storage.build_graph().unwrap();

        // Test case #1. Should correctly return the expected root device path
        // of 'root-data-a', since servicing type is CleanInstall.
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
        verity::{self},
    };

    use trident_api::{
        config::{
            self, AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
            MountOptions, MountPoint, Partition, PartitionType, VerityDevice,
        },
        constants::{MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
        error::ErrorKind,
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
    fn test_construct_by_partuuid_path() {
        // Test with a valid device path having a part_uuid
        let device_path = PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1"));
        construct_by_partuuid_path(&device_path).unwrap();

        // Test with an invalid device path
        let device_path = PathBuf::from("/dev/invalid");
        assert_eq!(construct_by_partuuid_path(&device_path), None);
    }

    /// Validates that validate_ab_active_volume_internal() works as a expected.
    #[functional_test]
    fn test_validate_ab_active_volume_internal() {
        let mut ctx = EngineContext {
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            servicing_type: ServicingType::AbUpdate,
            ..Default::default()
        };

        // Test case #0: If no internal mount points defined, should return an error.
        assert_eq!(
            validate_ab_active_volume_internal(&ctx, PathBuf::from(OS_DISK_DEVICE_PATH))
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::GetRootBlockDeviceId)
        );

        ctx.spec.storage.filesystems = vec![FileSystem {
            mount_point: Some(MountPoint {
                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                options: MountOptions::empty(),
            }),
            device_id: Some("root".to_string()),
            source: FileSystemSource::Image,
        }];

        // Test case #1: Missing A/B volume pair for root mount point.
        assert_eq!(
            validate_ab_active_volume_internal(&ctx, PathBuf::from(OS_DISK_DEVICE_PATH))
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::GetRootAbVolumePair {
                device_id: "root".to_string()
            })
        );

        // Test case #2: Missing block device for volume A.
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        assert_eq!(
            validate_ab_active_volume_internal(&ctx, PathBuf::from(OS_DISK_DEVICE_PATH))
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::GetRootAbVolumePair {
                device_id: "root".to_string()
            })
        );

        // Test case #3: Missing block device for volume B.
        ctx.partition_paths = btreemap! {
            "root-a".to_owned() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}15")),
        };

        assert_eq!(
            validate_ab_active_volume_internal(&ctx, PathBuf::from(OS_DISK_DEVICE_PATH))
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::GetRootAbVolumePair {
                device_id: "root".to_string()
            })
        );

        ctx.partition_paths.insert(
            "root-b".to_owned(),
            PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
        );

        // Test case #4: Volume A path cannot be resolved.
        assert_eq!(
            validate_ab_active_volume_internal(
                &ctx,
                PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2"))
            )
            .unwrap_err()
            .kind(),
            &ErrorKind::Servicing(ServicingError::CanonicalizePath {
                path: formatcp!("{OS_DISK_DEVICE_PATH}15").to_string(),
            })
        );

        // Test case #5: A or B paths do not match the root volume path.
        *ctx.partition_paths.get_mut("root-a").unwrap() =
            PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1"));
        assert_eq!(
            validate_ab_active_volume_internal(
                &ctx,
                PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}3"))
            )
            .unwrap_err()
            .kind(),
            &ErrorKind::Servicing(ServicingError::RootDevicePathAbActiveVolumeMismatch {
                root_device_path: formatcp!("{OS_DISK_DEVICE_PATH}3").to_string(),
                root_volume_a_path: formatcp!("{OS_DISK_DEVICE_PATH}1").to_string(),
                root_volume_b_path: formatcp!("{OS_DISK_DEVICE_PATH}2").to_string(),
            })
        );

        // Test case #6: Volume A is the root device path; active volume is set correctly to volume A.
        validate_ab_active_volume_internal(
            &ctx,
            PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1")),
        )
        .unwrap();

        // Test case #7: Volume B is the root device path; active volume is incorrectly set to A.
        assert_eq!(
            validate_ab_active_volume_internal(
                &ctx,
                PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2"))
            )
            .unwrap_err()
            .kind(),
            &ErrorKind::Servicing(ServicingError::ValidateAbActiveVolume {
                active_volume: AbVolumeSelection::VolumeB.to_string(),
                hs_active_volume: AbVolumeSelection::VolumeA.to_string(),
            })
        );

        // Test case #8: Volume B is the root device path; active volume is set correctly to volume B.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        validate_ab_active_volume_internal(
            &ctx,
            PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
        )
        .unwrap();

        // Test case #9: Volume A is the root device path; active volume is incorrectly set to B.
        assert_eq!(
            validate_ab_active_volume_internal(
                &ctx,
                PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1"))
            )
            .unwrap_err()
            .kind(),
            &ErrorKind::Servicing(ServicingError::ValidateAbActiveVolume {
                active_volume: AbVolumeSelection::VolumeA.to_string(),
                hs_active_volume: AbVolumeSelection::VolumeB.to_string(),
            })
        );
    }

    /// Validates that validate_ab_active_volume_internal() works as expected when root is a verity
    /// device.
    #[functional_test]
    fn test_validate_ab_active_volume_internal_verity() {
        let mut ctx = EngineContext {
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            servicing_type: ServicingType::AbUpdate,
            ..Default::default()
        };

        // Test case #1: Set up verity devices to test a scenario where root is an A/B volume on
        // verity.
        let verity_vol = verity::setup_verity_volumes();
        let verity_dev = verity_vol.verity_device("root");

        // Close the device if it is open
        verity_dev.close().unwrap();

        let _verityguard = verity_dev.open_with_guard();

        // Add partitions
        ctx.partition_paths = btreemap! {
            "os".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
            "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}5")),
            "root-data-a".into() => verity_vol.data_volume.clone(),
            "root-hash-a".into() => verity_vol.hash_volume.clone(),
            "root-data-b".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
            "root-hash-b".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}5")),
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

        // Add a verity device
        ctx.spec.storage.verity = vec![VerityDevice {
            id: "root".to_string(),
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            ..Default::default()
        }];

        ctx.spec.storage.filesystems = vec![FileSystem {
            device_id: Some("root".into()),
            mount_point: Some(MountPoint {
                path: ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
            }),
            source: FileSystemSource::Image,
        }];

        // Build storage graph to validate
        ctx.storage_graph = ctx.spec.storage.build_graph().unwrap();

        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        validate_ab_active_volume_internal(&ctx, PathBuf::from("/dev/mapper/root")).unwrap();
    }

    /// Validates that get_verity_data_device_path() correctly returns the actual path of the
    /// verity data device in the system.
    #[functional_test]
    fn test_get_verity_data_device_path() {
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
            get_verity_data_device_path(&ctx, &"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for verity device 'root'"
        );

        // Test case #1. Add an internal verity device config and ensure it is returned.
        ctx.spec.storage.verity = vec![VerityDevice {
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
        assert_eq!(
            get_verity_data_device_path(&ctx, &"root".to_owned())
                .unwrap_err()
                .to_string(),
            "Verity device 'root' is not active.",
        );

        // Test case #3: Since the verity device is not yet active, we should get an error.
        let verity_vol = verity::setup_verity_volumes();
        let verity_dev = verity_vol.verity_device("root");

        // Close the device if it is open
        verity_dev.close().unwrap();

        ctx.partition_paths = btreemap! {
            "os".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
            "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
            "root-data-a".into() => verity_vol.data_volume.clone(),
            "root-hash-a".into() => verity_vol.hash_volume.clone(),
            "boot2".into() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1")),
            "root-data-b".into() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
            "root-hash-b".into() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}3")),
            "root".into() => PathBuf::from("/dev/mapper/root"),
        };
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        assert_eq!(
            get_verity_data_device_path(&ctx, &"root".to_owned())
                .unwrap_err()
                .to_string(),
            "Verity device 'root' is not active."
        );

        // Test case #4: Returns the path to the verity device, once it is open.
        let _verityguard = verity_dev.open_with_guard();

        assert_eq!(
            get_verity_data_device_path(&ctx, &"root".to_owned()).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
        );

        // Test case #5: When the IDs are swapped, returns the correct path.
        ctx.spec.storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id =
            "root-data-b".to_string();

        ctx.spec.storage.ab_update.as_mut().unwrap().volume_pairs[0].volume_b_id =
            "root-data-a".to_string();

        assert_eq!(
            get_verity_data_device_path(&ctx, &"root".to_owned()).unwrap(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
        );
    }
}
