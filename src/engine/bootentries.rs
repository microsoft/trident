use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use log::{debug, trace};

use osutils::{
    block_devices,
    efibootmgr::{self, EfiBootManagerOutput},
};
use trident_api::{
    config::RaidLevel,
    constants::{self, internal_params::ENABLE_COSI_SUPPORT},
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
    BlockDeviceId,
};

use super::{boot, EngineContext};

/// Boot efi executable
const BOOT64_EFI: &str = "bootx64.efi";

/// ESP device metadata
#[derive(Debug, PartialEq, Clone)]
struct EspDeviceMetadata {
    id: BlockDeviceId,
    path: PathBuf,
}

/// ESP device enum which can be either a standalone partition or a RAID device
#[derive(Debug, PartialEq, Clone)]
enum EspDevice {
    Partition(EspDeviceMetadata),
    Raid(Vec<EspDeviceMetadata>),
}

/// Creates a boot entry for the A/B update volume and sets the `BootNext`
/// variable to boot from the updated partition on next boot. Also updates the
/// `BootOrder` for non-qemu targets.
///
/// Takes in the path where we expect to find the entry matching the install ID.
/// During clean install, this corresponds to /mnt/newroot/boot/efi, but during
/// A/B update, both A and B share a single ESP at /boot/efi.
#[tracing::instrument(name = "set_boot_order_configuration", skip_all)]
pub fn set_boot_next_and_update_boot_order(
    ctx: &EngineContext,
    esp_path: &Path,
) -> Result<(), TridentError> {
    // Get the label and path for the EFI boot loader of the inactive A/B update volume.
    let (entry_label_new, bootloader_path_new) =
        get_label_and_path(ctx).structured(ServicingError::GetLabelandPath)?;

    // Check if the boot entry already exists, if so, delete the entry and
    // remove it from the `BootOrder`.
    let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()
        .structured(ServicingError::ListAndParseBootEntries)?;
    if bootmgr_output
        .boot_entry_exists(&entry_label_new)
        .structured(ServicingError::BootEntryCheck {
            boot_entry: entry_label_new.clone(),
        })?
    {
        debug!(
            "Boot entry already exists, deleting entries with label '{}'",
            entry_label_new.as_str()
        );
        bootmgr_output
            .delete_entries_with_label(&entry_label_new)
            .message(format!(
                "Failed to delete boot entries with label '{entry_label_new}' via efibootmgr",
            ))?;
        // Get boot entry numbers for the entries with label '{entry_label_new}'
        let entry_numbers = bootmgr_output.get_entries_with_label(&entry_label_new);
        // Get the current `BootOrder`
        let current_boot_order = bootmgr_output.get_boot_order(); // Get the modified `BootOrder` after removing the entries with label '{entry_label_new}'
        let new_boot_order: Vec<String> = current_boot_order
            .iter()
            .filter(|&x| !entry_numbers.contains(x))
            .map(|x| x.to_string())
            .collect();

        // Get the updated `BootOrder`
        let new_boot_order_after_deletion = efibootmgr::list_and_parse_bootmgr_entries()
            .structured(ServicingError::ListAndParseBootEntries)?
            .get_boot_order();
        // If the `BootOrder` has changed, update the `BootOrder`
        if current_boot_order != new_boot_order && new_boot_order_after_deletion != new_boot_order {
            efibootmgr::modify_boot_order(new_boot_order.join(",").as_str())
                .message("Failed to modify `BootOrder` via efibootmgr")?;
        }
    }

    let esp_device_info = get_esp_device_info(ctx).structured(InternalError::GetEspDeviceInfo)?;
    let esp_device_metadata = parse_esp_metadata(ctx, esp_device_info)?;
    let added_entry_numbers = create_boot_entry_helper(
        ctx,
        esp_device_metadata,
        esp_path,
        entry_label_new,
        bootloader_path_new,
    )?;

    if !added_entry_numbers.is_empty() {
        // Set the `BootNext` variable to boot from the newly added first entry on next boot.
        let boot_next_entry = added_entry_numbers[0].clone();
        efibootmgr::set_boot_next(&boot_next_entry)?;
        debug!("Set `BootNext` to newly added first entry '{boot_next_entry}'");
        // HACK: detect if we're inside qemu to avoid modifying `BootOrder`.
        // TODO(#7139): remove this special case.
        if !osutils::virt::is_qemu() {
            update_boot_order(added_entry_numbers).structured(ServicingError::UpdateBootOrder)?;
        }
    } else {
        // If we didn't have any boot entries to add, we would have failed before reaching this
        // point.
        return Err(TridentError::new(InternalError::Internal(
            "Failed to add boot entries and update `BootNext` and `BootOrder`",
        )));
    }

    Ok(())
}

/// This function is used for QEMU targets to set the boot entries after reboot.
/// The function gets the `BootCurrent` from the boot manager output and sets the `BootOrder` to
/// include all the entries with the same label as `BootCurrent` in the `BootOrder`.
pub fn set_bootentries_after_reboot_for_qemu() -> Result<(), TridentError> {
    // Get `BootCurrent` from the boot manager output.
    let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()
        .structured(ServicingError::ListAndParseBootEntries)?;
    let boot_current = &bootmgr_output.boot_current;
    // Get the label of the `BootCurrent` entry.
    let boot_current_label = bootmgr_output
        .get_boot_entry_label(boot_current)
        .structured(ServicingError::ReadEfibootmgr)?;

    // Get the boot entry numbers with the label of `BootCurrent`.
    let boot_current_entries = bootmgr_output.get_entries_with_label(&boot_current_label);
    debug!(
        "Found boot entries with label '{}': {:?}",
        boot_current_label, boot_current_entries
    );
    // Modify `BootOrder` to include all the entries with the same label as
    // `BootCurrent`` in the `BootOrder`.
    update_boot_order(boot_current_entries).structured(ServicingError::UpdateBootOrder)
}

/// Parses the ESP device info and returns the ESP device metadata
/// If the ESP device is a standalone partition, the metadata for the partition is returned.
/// If the ESP device is on RAID1, the metadata for the RAID1 partitions is returned.
fn parse_esp_metadata(
    ctx: &EngineContext,
    esp_device_info: EspDevice,
) -> Result<Vec<EspDeviceMetadata>, TridentError> {
    Ok(match esp_device_info {
        EspDevice::Partition(esp_device_metadata) => vec![esp_device_metadata],
        EspDevice::Raid(esp_device_metadata) => {
            let esp_device_id =
                get_esp_device_id(ctx).structured(InternalError::GetEspDeviceInfo)?;

            let esp_raid_info = ctx
                .spec
                .storage
                .raid
                .software
                .iter()
                .find(|r| r.id == esp_device_id)
                .structured(InternalError::GetEspDeviceInfo)?;

            if esp_raid_info.level == RaidLevel::Raid1 {
                esp_device_metadata
            } else {
                // This point should never be reached, as the host configuration validation ensures
                // the ESP RAID level.
                return Err(TridentError::new(InternalError::Internal(
                    "Unsupported RAID level for ESP device",
                )));
            }
        }
    })
}

/// This function creates a boot entry for the new OS and returns the added entry numbers.
fn create_boot_entry_helper(
    ctx: &EngineContext,
    esp_device_metadata: Vec<EspDeviceMetadata>,
    esp_path: &Path,
    entry_label_new: String,
    bootloader_path_new: PathBuf,
) -> Result<Vec<String>, TridentError> {
    // Skip duplicate check for RAID1 ESP as we create boot entries with same label for
    // all the RAID1 partitions.
    let skip_duplicate = esp_device_metadata.len() > 1;
    esp_device_metadata
        .into_iter()
        .map(|esp_device| {
            let esp_device_id = esp_device.id.clone();
            let disk_path = esp_device.path.clone();

            // Get the UUID path of the ESP partition from ctx.
            let esp_uuid_path = ctx.block_device_paths.get(&esp_device_id).structured(
                ServicingError::GetBlockDevicePath {
                    device_id: esp_device_id.to_string(),
                },
            )?;

            debug!(
                "The disk path of the first ESP partition is {:?}, and the partition UUID path is {:?}",
                disk_path, esp_uuid_path
            );

            // Get the partition number of the ESP partition.
            let part_num =
                block_devices::get_partition_number(disk_path.clone(), esp_uuid_path.clone())
                    .structured(ServicingError::GetPartitionNumber {
                        disk_path: disk_path.to_string_lossy().to_string(),
                        part_uuid_path: esp_uuid_path.to_string_lossy().to_string(),
                    })?;

            debug!("ESP partition number: {}", part_num);

            // Create a boot entry for the new OS.
            efibootmgr::create_boot_entry(
                &entry_label_new,
                disk_path.clone(),
                bootloader_path_new.clone(),
                esp_path,
                part_num,
                skip_duplicate,
            )
            .structured(ServicingError::CreateBootEntry {
                boot_entry: entry_label_new.clone(),
            })?;

            let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()
                .structured(ServicingError::ListAndParseBootEntries)?;

            let added_entry_number = bootmgr_output
                .get_boot_entry_number(&entry_label_new)
                .structured(ServicingError::ReadEfibootmgr)?;

            debug!(
                "Added boot entry '{added_entry_number}' with label '{}'",
                entry_label_new.as_str()
            );

            Ok(added_entry_number)
        })
        .collect::<Result<Vec<String>, TridentError>>()
}

/// Returns the ESP partition device id from Engine Context
fn get_esp_device_id(ctx: &EngineContext) -> Result<BlockDeviceId, Error> {
    Ok(
        // We need to check for the OS Image manually because the COSI image is
        // not re-read for finalize, so ctx.os_image is None.
        if ctx.spec.os_image.is_some() && ctx.spec.internal_params.get_flag(ENABLE_COSI_SUPPORT) {
            // There is an OS Image, therefore the only means we have to identify the ESP is through
            // the mount point.
            trace!("Using ESP mount point to identify ESP device ID");
            ctx.spec
                .storage
                .esp_filesystem()
                .map(|(id, _)| id)
                .context("Host Configuration does not contain an ESP filesystem.")?
                .clone()
        } else {
            // TODO: Remove this branch once COSI is fully implemented.
            trace!("Using ESP image to identify ESP device ID");
            ctx.spec
                .storage
                .filesystems
                .iter()
                .find_map(|fs| fs.source.esp_image().and(fs.device_id.as_ref()))
                .context("Host Configuration does not contain an ESP filesystem.")?
                .clone()
        },
    )
}

/// Gets the EFI System Partition (ESP) device IDs and the path of the disks containing the ESP
/// partitions.
///
/// This information is extracted from the filesystem configuration within the `EngineContext`.
/// If the ESP partition is not on RAID, the information for the first encountered ESP will be
/// returned.
/// Currently, we only copy the bootloader to the first ESP partition found.
/// TODO: https://dev.azure.com/mariner-org/ECF/_workitems/edit/9622
/// TODO: https://dev.azure.com/mariner-org/ECF/_workitems/edit/9411
/// If the ESP partition is on RAID1, the information for the RAID1 partitions will be returned.
///
/// # Arguments
///
/// * `ctx` - A reference to the `EngineContext` which contains the host's configuration.
///
/// # Returns
///
/// * `Result<EspDevice, Error>` - On success, returns an EspDevice enum containing the ESP device
///   information. On failure, returns an `Error`.
///
fn get_esp_device_info(ctx: &EngineContext) -> Result<EspDevice, Error> {
    // TODO: What about deployments with multiple ESP partitions? (in multiple disks)
    // This implementation just finds the first ESP filesystem and uses that.

    // Find the device ID of the ESP filesystem
    let esp_device_id = get_esp_device_id(ctx).context("Could not find ESP device ID.")?;

    if let Some(raid) = ctx
        .spec
        .storage
        .raid
        .software
        .iter()
        .find(|r| r.id == esp_device_id)
    {
        Ok(EspDevice::Raid(
            raid.devices
                .iter()
                .map(|id| get_esp_metadata(id, ctx))
                .collect::<Result<_, _>>()?,
        ))
    } else {
        // ESP is a standalone partition, not on RAID
        Ok(EspDevice::Partition(get_esp_metadata(&esp_device_id, ctx)?))
    }
}

/// Retrieves the metadata for the ESP partition device.
fn get_esp_metadata(
    esp_device_id: &BlockDeviceId,
    ctx: &EngineContext,
) -> Result<EspDeviceMetadata, Error> {
    let device_path = ctx.block_device_paths.get(esp_device_id).with_context(|| {
        format!("Failed to find device path for ESP partition with device ID '{esp_device_id}'",)
    })?;

    let esp_disk_path = block_devices::block_device_by_path(
        block_devices::get_disk_for_partition(device_path.as_path()).with_context(|| {
            format!(
                "Failed to get disk for ESP partition '{esp_device_id}' with device path '{}'",
                device_path.display()
            )
        })?,
    )
    .context("Failed to get by-path symlink for disk")?;
    debug!(
        "Found disk for ESP partition '{esp_device_id}' with device path '{}'",
        esp_disk_path.display()
    );

    Ok(EspDeviceMetadata {
        id: esp_device_id.clone(),
        path: esp_disk_path,
    })
}

/// Retrieves the label and path for the EFI boot loader of the inactive A/B update volume.
///
/// This function takes a reference to a `EngineContext` object and returns a tuple containing
/// the label associated with the inactive A/B update volume and the path to its EFI boot loader.
///
fn get_label_and_path(ctx: &EngineContext) -> Result<(String, PathBuf), Error> {
    let esp_dir_name = boot::get_update_esp_dir_name(ctx).context("Failed to get install id")?;

    let path = Path::new(constants::ROOT_MOUNT_POINT_PATH)
        .join(constants::ESP_EFI_DIRECTORY)
        .join(&esp_dir_name)
        .join(BOOT64_EFI);

    Ok((esp_dir_name, path))
}

/// Lists EFI boot manager entries, checks if the `BootOrder` requires
/// updates based on the given boot entry, and updates the `BootOrder` if
/// needed.
///
#[tracing::instrument(skip_all)]
pub fn first_boot_order(boot_entry: &String) -> Result<(), Error> {
    let bootmgr_output: EfiBootManagerOutput = efibootmgr::list_and_parse_bootmgr_entries()
        .context("Failed to list and parse boot manager entries")?;

    let new_boot_order = generate_new_boot_order(&bootmgr_output, boot_entry);

    if let Some(new_boot_order) = new_boot_order {
        debug!("Modifying `BootOrder` to {}", new_boot_order);
        efibootmgr::modify_boot_order(&new_boot_order)
            .unstructured(format!("Failed to modify `BootOrder` to {new_boot_order}"))?;
    } else {
        debug!("Skipping `BootOrder` modification as it is already up-to-date");
    }

    Ok(())
}

/// This function sets the `BootOrder` to the specified boot entries, processing them in reverse
/// order to ensure they are added to the beginning of the `BootOrder` list.
///
#[tracing::instrument(skip_all)]
pub fn update_boot_order(boot_current_entries: Vec<String>) -> Result<(), Error> {
    for added_entry_number in boot_current_entries.iter().rev() {
        debug!(
            "Adding boot entry '{}' to the beginning of `BootOrder`",
            added_entry_number
        );
        first_boot_order(added_entry_number)?;
    }
    Ok(())
}

/// Analyzes whether the EFI `BootOrder` should be modified based on the given boot entry value.
///
/// # Returns
/// - `Some(new_boot_order)`: A string representing the new `BootOrder` after adjustments.
/// - `None`: If no modifications to the `BootOrder` are needed.
#[tracing::instrument(skip_all)]
fn generate_new_boot_order(
    bootmgr_output: &EfiBootManagerOutput,
    boot_entry: &String,
) -> Option<String> {
    let mut boot_order_initial: Vec<String> = bootmgr_output.boot_order.clone();

    if boot_order_initial.contains(boot_entry) {
        if let Some(index) = boot_order_initial.iter().position(|x| x == boot_entry) {
            if index != 0 {
                // Boot entry is part of `BootOrder` but not at the first position. Move it to the first position.
                boot_order_initial.remove(index);
                boot_order_initial.insert(0, boot_entry.to_string());
            } else {
                // Boot entry is already at the first position in `BootOrder`. No need to modify.
                return None;
            }
        }
    } else {
        boot_order_initial.insert(0, boot_entry.to_string());
    }

    let new_boot_order_str = boot_order_initial.join(",");

    Some(new_boot_order_str)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use osutils::{
        efibootmgr::{EfiBootEntry, EfiBootManagerOutput},
        testutils::repart::TEST_DISK_DEVICE_PATH,
    };
    use trident_api::{
        config::{
            self, AbUpdate, Disk, FileSystemType, HostConfiguration, MountOptions, MountPoint,
            Partition, PartitionSize, PartitionType,
        },
        error::ErrorKind,
        status::{AbVolumeSelection, ServicingType},
    };

    use super::*;
    use boot::get_update_esp_dir_name;

    use constants::ESP_MOUNT_POINT_PATH;

    /// Validates logic for determining which A/B volume to use for updates
    #[test]
    fn test_get_label_and_path() {
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    ab_update: Some(AbUpdate {
                        volume_pairs: Vec::new(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        // Test that clean-install will always use volume A for updates
        assert_eq!(
            get_label_and_path(&ctx).unwrap(),
            (
                get_update_esp_dir_name(&ctx).unwrap(),
                Path::new(constants::ROOT_MOUNT_POINT_PATH)
                    .join(constants::ESP_EFI_DIRECTORY)
                    .join(get_update_esp_dir_name(&ctx).unwrap())
                    .join(BOOT64_EFI)
            )
        );

        // Test that servicing types HotPatch, NormalUpdate, UpdateAndReboot will always use the
        // active volume for updates
        ctx.servicing_type = ServicingType::NormalUpdate;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_label_and_path(&ctx).unwrap(),
            (
                get_update_esp_dir_name(&ctx).unwrap(),
                Path::new(constants::ROOT_MOUNT_POINT_PATH)
                    .join(constants::ESP_EFI_DIRECTORY)
                    .join(get_update_esp_dir_name(&ctx).unwrap())
                    .join(BOOT64_EFI)
            )
        );

        // Test that servicing type NoActiveServicing will return None
        ctx.servicing_type = ServicingType::NoActiveServicing;
        let error_message = get_label_and_path(&ctx).unwrap_err().to_string();
        assert_eq!(error_message, "Failed to get install id");
    }

    fn get_bootmgr_output() -> EfiBootManagerOutput {
        let entry1 = EfiBootEntry {
            id: "0001".to_string(),
            label: "ubuntu".to_string(),
        };

        let entry2 = EfiBootEntry {
            id: "0002".to_string(),
            label: "UEFI: Built-in EFI Shell".to_string(),
        };

        let entry3 = EfiBootEntry {
            id: "0003".to_string(),
            ..Default::default()
        };

        // Sample EfiBootManagerOutput instance
        EfiBootManagerOutput {
            boot_next: String::new(),
            boot_current: "0003".to_string(),
            boot_order: vec!["0001".to_string(), "0000".to_string()],
            boot_entries: vec![entry1, entry2, entry3],
        }
    }

    /// This function tests the update_efi_boot_order function which modifies
    /// the `BootOrder` by placing the boot entry at the first position.
    #[test]
    fn test_update_efi_boot_order() {
        let bootmgr_output = get_bootmgr_output();

        // Test case where boot entry is already at the first position in `BootOrder`
        let result = generate_new_boot_order(&bootmgr_output, &String::from("0001"));
        assert_eq!(result, None);

        // Test case where boot entry is not part of `BootOrder`
        let result = generate_new_boot_order(&bootmgr_output, &String::from("0002"));
        assert_eq!(result, Some("0002,0001,0000".to_string()));

        // Test case where boot entry is part of `BootOrder` but not at the first position
        let result = generate_new_boot_order(&bootmgr_output, &String::from("0000"));
        assert_eq!(result, Some("0000,0001".to_string()));
    }

    pub(crate) fn get_esp_on_raid_ctx() -> EngineContext {
        EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    filesystems: vec![config::FileSystem {
                        source: config::FileSystemSource::EspImage(config::Image {
                            url: "http://example.com/esp.img".to_string(),
                            sha256: config::ImageSha256::Ignored,
                            format: config::ImageFormat::RawZst,
                        }),
                        device_id: Some("esp".to_string()),
                        fs_type: FileSystemType::Vfat,
                        mount_point: Some(MountPoint {
                            path: ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                    }],
                    disks: vec![Disk {
                        id: "disk1".into(),
                        device: TEST_DISK_DEVICE_PATH.into(),
                        partitions: vec![
                            Partition {
                                id: "esp1".into(),
                                size: PartitionSize::from_str("512M").unwrap(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "esp2".into(),
                                size: PartitionSize::from_str("512M").unwrap(),
                                partition_type: PartitionType::Esp,
                            },
                        ],
                        ..Default::default()
                    }],
                    raid: config::Raid {
                        software: vec![config::SoftwareRaidArray {
                            id: "esp".into(),
                            name: "esp".to_string(),
                            level: config::RaidLevel::Raid1,
                            devices: vec!["esp1".into(), "esp2".into()],
                        }],
                        sync_timeout: Some(180),
                    },

                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    pub(crate) fn get_esp_on_partition() -> EngineContext {
        EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    filesystems: vec![config::FileSystem {
                        source: config::FileSystemSource::EspImage(config::Image {
                            url: "http://example.com/esp.img".to_string(),
                            sha256: config::ImageSha256::Ignored,
                            format: config::ImageFormat::RawZst,
                        }),
                        device_id: Some("esp".to_string()),
                        fs_type: FileSystemType::Vfat,
                        mount_point: Some(MountPoint {
                            path: ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                    }],
                    disks: vec![Disk {
                        id: "disk1".into(),
                        device: TEST_DISK_DEVICE_PATH.into(),
                        partitions: vec![Partition {
                            id: "esp".into(),
                            size: PartitionSize::from_str("512M").unwrap(),
                            partition_type: PartitionType::Esp,
                        }],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// parse_esp_metadata() function tests
    #[test]
    fn test_parse_esp_metadata_esp_on_raid() {
        let mut ctx = get_esp_on_raid_ctx();
        let esp_meta_data_vec = vec![
            EspDeviceMetadata {
                id: "esp1".into(),
                path: PathBuf::from(TEST_DISK_DEVICE_PATH),
            },
            EspDeviceMetadata {
                id: "esp2".into(),
                path: PathBuf::from(TEST_DISK_DEVICE_PATH),
            },
        ];
        let esp_device_info = EspDevice::Raid(esp_meta_data_vec.clone());
        let esp_device_metadata = parse_esp_metadata(&ctx, esp_device_info.clone()).unwrap();
        assert_eq!(esp_device_metadata, esp_meta_data_vec);

        // Test case where ESP RAID level is not RAID1
        ctx.spec.storage.raid.software[0].level = config::RaidLevel::Raid0;

        assert_eq!(
            parse_esp_metadata(&ctx, esp_device_info)
                .unwrap_err()
                .kind(),
            &ErrorKind::Internal(InternalError::Internal(
                "Unsupported RAID level for ESP device"
            ))
        );
    }

    #[test]
    fn test_parse_esp_metadata_esp_on_partition() {
        let ctx = get_esp_on_partition();
        let esp_meta_data = EspDeviceMetadata {
            id: "esp".into(),
            path: PathBuf::from(TEST_DISK_DEVICE_PATH),
        };
        let esp_device_info = EspDevice::Partition(esp_meta_data.clone());
        let esp_device_metadata = parse_esp_metadata(&ctx, esp_device_info).unwrap();
        assert_eq!(esp_device_metadata, vec![esp_meta_data]);
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::iter::Iterator;

    use osutils::{
        efibootmgr::{self, EfiBootManagerOutput},
        files::create_file,
        path::join_relative,
        testutils::repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH},
    };
    use pytest_gen::functional_test;

    use crate::engine::storage::partitioning;

    #[allow(dead_code)]
    fn delete_boot_next() {
        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        if !bootmgr_output.boot_next.is_empty() {
            // Unset the boot_next variable using efibootmgr
            efibootmgr::delete_boot_next().unwrap();
        }
    }

    fn set_some_boot_entries() {
        // Create new boot manager entries for testing
        let tempdir = tempfile::tempdir().unwrap();
        // Create bootloader path
        let bootloader_path = Path::new(r"/EFI/AZLA/bootx64.efi");
        // create_boot_entry() will call is_valid_bootloader_path() to verify if file exists at
        // {tempdir}/{bootloader_path}. So, create a dummy bootloader file
        let bootloader_file_path = join_relative(tempdir.path(), bootloader_path);
        create_file(bootloader_file_path).unwrap();

        efibootmgr::create_boot_entry(
            "TestBoot1",
            OS_DISK_DEVICE_PATH,
            bootloader_path,
            tempdir.path(),
            1,
            false,
        )
        .unwrap();
        efibootmgr::create_boot_entry(
            "TestBoot2",
            OS_DISK_DEVICE_PATH,
            bootloader_path,
            tempdir.path(),
            2,
            false,
        )
        .unwrap();
        efibootmgr::create_boot_entry(
            "TestBoot3",
            OS_DISK_DEVICE_PATH,
            bootloader_path,
            tempdir.path(),
            3,
            false,
        )
        .unwrap();
        let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries().unwrap();

        let _entry_number1 = bootmgr_output.get_boot_entry_number("TestBoot1").unwrap();
        let entry_number2 = bootmgr_output.get_boot_entry_number("TestBoot2").unwrap();
        let entry_number3 = bootmgr_output.get_boot_entry_number("TestBoot3").unwrap();

        let initial_boot_order = bootmgr_output.boot_order;
        let mut boot_order = initial_boot_order.clone();
        boot_order.insert(0, entry_number2.to_string());
        boot_order.insert(1, entry_number3.to_string());

        // Add to `BootOrder`
        efibootmgr::modify_boot_order(&boot_order.join(",")).unwrap();
    }

    fn delete_created_boot_entries() {
        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();

        let entry_number1 = bootmgr_output.get_boot_entry_number("TestBoot1").unwrap();
        let entry_number2 = bootmgr_output.get_boot_entry_number("TestBoot2").unwrap();
        let entry_number3 = bootmgr_output.get_boot_entry_number("TestBoot3").unwrap();
        efibootmgr::delete_boot_entry(&entry_number1).unwrap();
        efibootmgr::delete_boot_entry(&entry_number2).unwrap();
        efibootmgr::delete_boot_entry(&entry_number3).unwrap();
    }

    #[functional_test]
    fn test_get_esp_device_info() {
        let mut ctx = tests::get_esp_on_partition();
        partitioning::create_partitions(&mut ctx).unwrap();

        let esp_device_info = get_esp_device_info(&ctx).unwrap();

        let (esp_id, disk_path) = match &esp_device_info {
            EspDevice::Partition(esp_device_metadata) => (
                esp_device_metadata.id.clone(),
                esp_device_metadata.path.clone(),
            ),
            _ => panic!("ESP device info is not of type Partition"),
        };
        assert_eq!(esp_id, "esp", "ESP device id mismatch");

        let canon_path = disk_path.canonicalize().unwrap();
        assert_eq!(
            canon_path.as_path(),
            Path::new(TEST_DISK_DEVICE_PATH),
            "Disk path mismatch"
        );

        // Test parse_esp_metadata() function
        let esp_metadata = parse_esp_metadata(&ctx, esp_device_info).unwrap();
        assert_eq!(esp_metadata.len(), 1, "ESP metadata length mismatch");
        assert_eq!(esp_metadata[0].id, "esp", "ESP device id mismatch");

        // Test case where get_partition_number() finds the ESP partition number
        let esp_partition_path = ctx.block_device_paths.get(&esp_id).unwrap();
        let part_num = block_devices::get_partition_number(&disk_path, esp_partition_path).unwrap();
        assert_eq!(part_num, 1, "Partition number mismatch");
        // Test case where get_partition_number() fails to find the ESP partition number
        let esp_partition_path1 = Path::new("/dev/sda1");
        let part_num = block_devices::get_partition_number(&disk_path, esp_partition_path1);
        debug_assert_eq!(
            part_num.unwrap_err().root_cause().to_string(),
            format!(
                "Failed to find the partition '/dev/sda1' in disk '{}'",
                disk_path.display()
            )
        );
        // Test case where get_partition_number() fails to get the disk information
        let doesnotexist = Path::new("/dev/doesnotexist");
        let part_num = block_devices::get_partition_number(doesnotexist, esp_partition_path);
        debug_assert!(part_num.unwrap_err().root_cause().to_string().contains(
            "stderr:\nsfdisk: cannot open /dev/doesnotexist: No such file or directory\n\n"
        ));
    }

    #[functional_test]
    fn test_get_esp_device_info_raided_esp() {
        let mut ctx = tests::get_esp_on_raid_ctx();
        partitioning::create_partitions(&mut ctx).unwrap();

        let esp_device_info = get_esp_device_info(&ctx).unwrap();

        let (esp_id1, disk_path1) = match &esp_device_info {
            EspDevice::Raid(esp_device_metadata) => (
                esp_device_metadata[0].id.clone(),
                esp_device_metadata[0].path.clone(),
            ),
            _ => panic!("ESP device info is not of type Raid"),
        };
        assert_eq!(esp_id1, "esp1", "ESP device id mismatch");
        let canon_path = disk_path1.canonicalize().unwrap();
        assert_eq!(
            canon_path.as_path(),
            Path::new(TEST_DISK_DEVICE_PATH),
            "Disk path mismatch"
        );

        let (esp_id2, disk_path2) = match &esp_device_info {
            EspDevice::Raid(esp_device_metadata) => (
                esp_device_metadata[1].id.clone(),
                esp_device_metadata[1].path.clone(),
            ),
            _ => panic!("ESP device info is not of type Raid"),
        };
        assert_eq!(esp_id2, "esp2", "ESP device id mismatch");
        let canon_path = disk_path2.canonicalize().unwrap();
        assert_eq!(
            canon_path.as_path(),
            Path::new(TEST_DISK_DEVICE_PATH),
            "Disk path mismatch"
        );

        // Test parse_esp_metadata() function
        let esp_metadata = parse_esp_metadata(&ctx, esp_device_info).unwrap();
        assert_eq!(esp_metadata.len(), 2);
        assert_eq!(esp_metadata[0].id, "esp1", "ESP device id mismatch");
        assert_eq!(esp_metadata[1].id, "esp2", "ESP device id mismatch");

        let canon_path = esp_metadata[0].path.canonicalize().unwrap();
        assert_eq!(
            canon_path.as_path(),
            Path::new(TEST_DISK_DEVICE_PATH),
            "Disk path mismatch"
        );
        let canon_path = esp_metadata[1].path.canonicalize().unwrap();
        assert_eq!(
            canon_path.as_path(),
            Path::new(TEST_DISK_DEVICE_PATH),
            "Disk path mismatch"
        );

        // Test case where get_partition_number() finds the ESP partition number
        let esp_partition_path = ctx.block_device_paths.get(&esp_id1).unwrap();
        let part_num =
            block_devices::get_partition_number(&disk_path1, esp_partition_path).unwrap();
        assert_eq!(part_num, 1, "Partition number mismatch");
        // Test case where get_partition_number() fails to find the ESP partition number
        let esp_partition_path1 = Path::new("/dev/sda1");
        let part_num = block_devices::get_partition_number(&disk_path1, esp_partition_path1);
        debug_assert_eq!(
            part_num.unwrap_err().root_cause().to_string(),
            format!(
                "Failed to find the partition '/dev/sda1' in disk '{}'",
                disk_path1.display()
            )
        );
        // Test case where get_partition_number() fails to get the disk information
        let doesnotexist = Path::new("/dev/doesnotexist");
        let part_num = block_devices::get_partition_number(doesnotexist, esp_partition_path);
        debug_assert!(part_num.unwrap_err().root_cause().to_string().contains(
            "stderr:\nsfdisk: cannot open /dev/doesnotexist: No such file or directory\n\n"
        ));
    }

    #[functional_test(feature = "helpers")]
    fn test_first_boot_order_when_update_success() {
        delete_boot_next();
        set_some_boot_entries();

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let boot_current = &bootmgr_output.boot_current;
        let initial_boot_order = bootmgr_output.boot_order;

        // Test that target was able to boot into the updated partition.
        first_boot_order(boot_current).unwrap();

        // Get the modified boot_order
        let bootmgr_output1: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let final_boot_order = bootmgr_output1.boot_order;

        // Set expected `BootOrder` i.e `BootCurrent` as the first entry and rest of the entries in the same order as initial_boot_order.
        let mut expected_boot_order = initial_boot_order.clone();
        if expected_boot_order.contains(boot_current) {
            let index = expected_boot_order
                .iter()
                .position(|x| x == boot_current)
                .unwrap();
            if index != 0 {
                expected_boot_order.remove(index);
                expected_boot_order.insert(0, boot_current.to_string());
            }
        } else {
            expected_boot_order.insert(0, boot_current.to_string());
        }

        // Cleanup
        delete_created_boot_entries();

        assert_eq!(expected_boot_order[0], boot_current.to_string());
        assert_eq!(expected_boot_order, final_boot_order);
    }

    /// Test that the `BootOrder` is not modified if the boot entry is already at the first position.
    #[functional_test(feature = "helpers")]
    fn test_first_boot_order_skip_boot_order_update() {
        delete_boot_next();
        set_some_boot_entries();

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let initial_boot_order = bootmgr_output.boot_order;
        let boot_entry = initial_boot_order[0].clone();

        first_boot_order(&boot_entry).unwrap();

        // Get the modified `BootOrder`
        let bootmgr_output1: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let final_boot_order = bootmgr_output1.boot_order;

        // Cleanup
        delete_created_boot_entries();

        assert_eq!(initial_boot_order, final_boot_order);
    }

    /// Test update_boot_order() function
    #[functional_test(feature = "helpers")]
    fn test_update_boot_order() {
        delete_boot_next();
        set_some_boot_entries();

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let initial_boot_order = bootmgr_output.boot_order;
        let boot_entry1 = initial_boot_order[0].clone();
        let boot_entry2 = initial_boot_order[1].clone();

        update_boot_order(vec![boot_entry1.clone(), boot_entry2.clone()]).unwrap();

        // Get the modified `BootOrder`
        let bootmgr_output1: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let final_boot_order = bootmgr_output1.boot_order;

        // Cleanup
        delete_created_boot_entries();

        assert_eq!(final_boot_order[0], boot_entry1);
        assert_eq!(final_boot_order[1], boot_entry2);
    }

    /// Test set_bootentries_after_reboot_for_qemu() function
    #[functional_test(feature = "helpers")]
    fn test_set_bootentries_after_reboot_for_qemu() {
        delete_boot_next();
        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let boot_current = bootmgr_output.boot_current.clone();
        // Get the label of the `BootCurrent` entry.
        let boot_current_label = bootmgr_output.get_boot_entry_label(&boot_current).unwrap();

        // Create 2 entries with the same label as `BootCurrent`
        // Create new boot manager entries for testing
        let tempdir = tempfile::tempdir().unwrap();
        // Create bootloader path
        let bootloader_path = Path::new(r"/EFI/AZLA/bootx64.efi");
        // create_boot_entry() will call is_valid_bootloader_path() to verify if file exists at
        // {tempdir}/{bootloader_path}. So, create a dummy bootloader file
        let bootloader_file_path = join_relative(tempdir.path(), bootloader_path);
        create_file(bootloader_file_path).unwrap();

        efibootmgr::create_boot_entry(
            boot_current_label.clone(),
            OS_DISK_DEVICE_PATH,
            bootloader_path,
            tempdir.path(),
            1,
            true,
        )
        .unwrap();

        let boot_entry_number1 = efibootmgr::list_and_parse_bootmgr_entries()
            .unwrap()
            .get_boot_entry_number(&boot_current_label.clone())
            .unwrap();
        assert_ne!(boot_entry_number1, boot_current);

        efibootmgr::create_boot_entry(
            boot_current_label.clone(),
            OS_DISK_DEVICE_PATH,
            bootloader_path,
            tempdir.path(),
            2,
            true,
        )
        .unwrap();

        let boot_entry_number2 = efibootmgr::list_and_parse_bootmgr_entries()
            .unwrap()
            .get_boot_entry_number(&boot_current_label)
            .unwrap();
        assert_ne!(boot_entry_number1, boot_entry_number2);

        set_bootentries_after_reboot_for_qemu().unwrap();

        // Get the modified `BootOrder`
        let bootmgr_output1: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let boot_order = bootmgr_output1.boot_order.clone();

        assert_eq!(boot_order[0], boot_current);
        assert_eq!(boot_order[1], boot_entry_number1);
        assert_eq!(boot_order[2], boot_entry_number2);

        // Cleanup
        let entry_numbers = bootmgr_output1.get_entries_with_label(&boot_current_label);
        for entry_number in entry_numbers {
            if entry_number != boot_current {
                efibootmgr::delete_boot_entry(&entry_number).unwrap();
            }
        }
    }
}
