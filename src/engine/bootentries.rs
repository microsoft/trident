use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use log::debug;

use osutils::{
    block_devices,
    bootloaders::BootloaderExecutable,
    efibootmgr::{self, EfiBootManagerOutput},
    virt,
};

use trident_api::{
    config::RaidLevel,
    constants::{self, internal_params::VIRTDEPLOY_BOOT_ORDER_WORKAROUND, ESP_MOUNT_POINT_PATH},
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{AbVolumeSelection, ServicingType},
    BlockDeviceId,
};

use super::{
    boot::{self, uki},
    EngineContext,
};

/// Boot EFI executable
const BOOT_EFI: &str = BootloaderExecutable::Boot.current_name();

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

/// Desired position to add boot entry into the BootOrder.
#[derive(Debug, PartialEq, Clone)]
pub enum BootOrderPosition {
    First,
    Last,
}

/// Creates a boot entry for the A/B update volume and sets the `BootNext`
/// variable to boot from the updated partition on next boot. Also updates the
/// `BootOrder` for non-virtdeploy targets.
///
/// Takes in the path where we expect to find the entry matching the install ID.
/// During clean install, this corresponds to /mnt/newroot/boot/efi, but during
/// A/B update, both A and B share a single ESP at /boot/efi.
#[tracing::instrument(name = "set_boot_order_configuration", skip_all)]
pub fn create_and_update_boot_variables(
    ctx: &EngineContext,
    esp_path: &Path,
) -> Result<(), TridentError> {
    // Get the label and path for the EFI boot loader of the inactive A/B update volume.
    let (entry_label_new, bootloader_path_new) =
        get_label_and_path(ctx).structured(ServicingError::GetLabelAndPath)?;

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

    // Update boot variables
    set_boot_next_and_update_boot_order(ctx, added_entry_numbers)?;

    if uki::is_staged(esp_path) {
        let oneshot = ctx.servicing_type != ServicingType::CleanInstall;
        uki::update_uki_boot_order(ctx, esp_path, oneshot)?;
    }

    Ok(())
}

/// Update the `BootNext` and potentially also `BootOrder`.
///
/// During clean install, both `BootNext` and `BootOrder` will be updated to point to the new entry.
/// During A/B update, only `BootNext` will be set to boot from the first entry from
/// `entry_numbers`.
///
/// When the virtdeploy workaround is enabled (either because virtdeploy was detected, or because
/// the internal parameter was passed in the Host Configuration), `BootOrder` is never updated even
/// if it otherwise would be.
pub fn set_boot_next_and_update_boot_order(
    ctx: &EngineContext,
    entry_numbers: Vec<String>,
) -> Result<(), TridentError> {
    if !entry_numbers.is_empty() {
        // Set the `BootNext` variable to boot from the first entry on next boot.
        let boot_next_entry = entry_numbers[0].clone();
        efibootmgr::set_boot_next(&boot_next_entry)?;
        debug!("Set `BootNext` to newly added first entry '{boot_next_entry}'");

        // Detect if we're inside virtdeploy to avoid modifying `BootOrder`.
        // TODO(#7139): remove this special case.
        let use_virtdeploy_workaround = virt::is_virtdeploy()
            || ctx
                .spec
                .internal_params
                .get_flag(VIRTDEPLOY_BOOT_ORDER_WORKAROUND);

        if ctx.servicing_type == ServicingType::AbUpdate {
            // During AB update, add new entry to end of the BootOrder so that UEFI will
            // consider the entry as permanent.  The entry is added to the end of the BootOrder
            // so that we can use BootNext to attempt to boot into the new OS and if anything
            // goes wrong, the system will "rollback" to the previous OS.  If the new OS is
            // successfully booted into, the BootOrder will be updated to move the new entry
            // to the head of the BootOrder list during 'commit'.
            //
            // Note: Ensuring new entries are present in the boot order is especially important
            // for some DELL machines which do not always persist boot entries that are not in
            // the BootOrder (which is vital to our tests as they run on DELL machines).
            // Boot entries disappearing does seem related to something unknown (maybe a machine
            // corruption) in the machine state, so be wary of changing this code to create boot
            // entries that are not in the BootOrder. We have fixed this and subsequently removed
            // the fix because it didn't seem neccessary (our tests continued passing), only to
            // have boot entries start disappearing again.
            update_boot_order(entry_numbers, &BootOrderPosition::Last)
                .structured(ServicingError::UpdateBootOrder)?;
        } else if ctx.servicing_type == ServicingType::CleanInstall && !use_virtdeploy_workaround {
            // During clean install, immediately set the bootorder to use the new entry.
            update_boot_order(entry_numbers, &BootOrderPosition::First)
                .structured(ServicingError::UpdateBootOrder)?;
        }
    } else {
        debug!("No changes to the boot variables are needed, skipping `BootNext` and `BootOrder` update");
    }

    Ok(())
}

/// Make the current boot option the default entry going forward.
///
/// The function gets the `BootCurrent` from the boot manager output and sets the `BootOrder` to
/// include all the entries with the same label as `BootCurrent` in the `BootOrder`.
pub fn persist_boot_order() -> Result<(), TridentError> {
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
    update_boot_order(boot_current_entries, &BootOrderPosition::First)
        .structured(ServicingError::UpdateBootOrder)
}

/// Returns the boot entry labels of the A/B volumes.
pub fn get_entry_labels(install_index: usize) -> Result<[String; 2], TridentError> {
    let entry_label_a = boot::make_esp_dir_name(install_index, AbVolumeSelection::VolumeA);
    let entry_label_b = boot::make_esp_dir_name(install_index, AbVolumeSelection::VolumeB);

    Ok([entry_label_a, entry_label_b])
}

// Creates boot entries for the rebuilt esp partitions and returns the boot entry order including
// the newly added boot entries.
fn create_and_get_boot_entry_order_after_rebuilding(
    ctx: &EngineContext,
    entry_labels: Vec<String>,
    disks_to_rebuild: &[BlockDeviceId],
    esp_device_metadata: Vec<EspDeviceMetadata>,
) -> Result<Vec<String>, TridentError> {
    create_boot_entries_for_rebuilt_esp_partitions(
        ctx,
        entry_labels.clone(),
        disks_to_rebuild,
        esp_device_metadata,
    )?;
    get_boot_entry_order(entry_labels, ctx.ab_active_volume)
}

/// Creates boot entries for the missing A/B volumes for the rebuilt esp partitions after rebuilding
/// the RAID1 disks.
fn create_boot_entries_for_rebuilt_esp_partitions(
    ctx: &EngineContext,
    entry_labels: Vec<String>,
    disks_to_rebuild: &[BlockDeviceId],
    esp_device_metadata: Vec<EspDeviceMetadata>,
) -> Result<(), TridentError> {
    // If disks to rebuild is empty, no need to create boot entries.
    if disks_to_rebuild.is_empty() {
        debug!("No disks to rebuild, skipping boot entry creation for the ESP partitions");
        return Ok(());
    }

    let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()
        .structured(ServicingError::ListAndParseBootEntries)?;

    let esp_metadata_cloned = esp_device_metadata.clone();
    // Create boot entries with the existing labels for the RAID1 ESP partitions on
    // the disks to rebuild.
    entry_labels
        .iter()
        .take(2)
        .filter(|&entry_label| {
            bootmgr_output
                .boot_entry_exists(entry_label)
                .unwrap_or(false)
        })
        .cloned()
        .flat_map(|entry_label| {
            esp_metadata_cloned.iter().flat_map(move |esp_device| {
                ctx.spec.storage.disks.iter().flat_map({
                    let entry_label = entry_label.clone();
                    move |disk| {
                        let partitions: Vec<_> =
                            disk.partitions.iter().map(|p| p.id.clone()).collect();

                        // Check if the ESP device is on the disk to be rebuilt.
                        if disks_to_rebuild.contains(&disk.id)
                            && partitions.contains(&esp_device.id)
                        {
                            let bootloader_path = Path::new(constants::ROOT_MOUNT_POINT_PATH)
                                .join(constants::ESP_EFI_DIRECTORY)
                                .join(&entry_label)
                                .join(BOOT_EFI);

                            create_entry(
                                ctx,
                                esp_device.clone(),
                                entry_label.clone(),
                                Path::new(ESP_MOUNT_POINT_PATH),
                                bootloader_path,
                                true,
                            )
                            .ok()
                            .map(|_| ())
                        } else {
                            None
                        }
                    }
                })
            })
        })
        .count();

    Ok(())
}

/// Returns boot entry numbers of all the available A/B volumes in the order of the active volume.
fn get_boot_entry_order(
    mut labels: Vec<String>,
    ab_active_volume: Option<AbVolumeSelection>,
) -> Result<Vec<String>, TridentError> {
    if ab_active_volume == Some(AbVolumeSelection::VolumeB) {
        // Reverse the entry labels if the active volume is B so that boot entries will be added in
        // the order of B, A to the BootOrder.
        labels.reverse();
    }
    let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()
        .structured(ServicingError::ListAndParseBootEntries)?;

    // Get the boot entry ids for the boot entries with the labels in the order of the active volume.
    let boot_entry_ids: Vec<String> = labels
        .iter()
        .flat_map(|entry_label| {
            bootmgr_output
                .boot_entries
                .iter()
                .filter(move |entry| entry.label == *entry_label)
                .map(|entry| entry.id.clone())
        })
        .collect();

    Ok(boot_entry_ids)
}

/// Creates boot entries for the rebuilt esp partitions and updates the boot variables i.e
/// `BootNext` and `BootOrder` after rebuilding the RAID1 disks.
pub fn create_and_update_boot_variables_after_rebuilding(
    ctx: &EngineContext,
    entry_labels: Vec<String>,
    disks_to_rebuild: &[BlockDeviceId],
) -> Result<(), TridentError> {
    let esp_device_info = get_esp_device_info(ctx).structured(ServicingError::GetEspDeviceInfo)?;

    if let EspDevice::Partition(_) = esp_device_info {
        // No need to create boot entries for standalone ESP partition.
        return Ok(());
    }
    // If Esp device is on RAID1, we need to create boot entries for all the RAID1 partitions on the
    // disks to rebuild.
    let entry_numbers = create_and_get_boot_entry_order_after_rebuilding(
        ctx,
        entry_labels,
        disks_to_rebuild,
        parse_esp_metadata(ctx, esp_device_info)?,
    )?;
    // Update the `BootOrder` to include the newly added boot entries and to rearrange the
    // `BootOrder`.
    set_boot_next_and_update_boot_order(ctx, entry_numbers)?;

    Ok(())
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
            create_entry(
                ctx,
                esp_device,
                entry_label_new.clone(),
                esp_path,
                bootloader_path_new.clone(),
                skip_duplicate,
            )
        })
        .collect::<Result<Vec<String>, TridentError>>()
}

/// Function that calls create_boot_entry to create a boot entry.
fn create_entry(
    ctx: &EngineContext,
    esp_device: EspDeviceMetadata,
    entry_label_new: String,
    esp_path: &Path,
    bootloader_path_new: PathBuf,
    skip_duplicate: bool,
) -> Result<String, TridentError> {
    let esp_device_id = esp_device.id.clone();
    let disk_path = esp_device.path.clone();

    // Get the UUID path of the ESP partition from ctx.
    let esp_uuid_path =
        ctx.partition_paths
            .get(&esp_device_id)
            .structured(ServicingError::GetBlockDevicePath {
                device_id: esp_device_id.to_string(),
            })?;

    debug!(
        "The disk path of the first ESP partition is {:?}, and the partition UUID path is {:?}",
        disk_path, esp_uuid_path
    );

    // Get the partition number of the ESP partition.
    let part_num = block_devices::get_partition_number(disk_path.clone(), esp_uuid_path.clone())
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

    let added_entry_number = get_entry_number(&entry_label_new)?;
    debug!(
        "Added boot entry '{added_entry_number}' with label '{}'",
        entry_label_new.as_str(),
    );

    Ok(added_entry_number)
}

/// Gets the entry number of the latest boot entry with the given label.
fn get_entry_number(entry_label: &str) -> Result<String, TridentError> {
    let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()
        .structured(ServicingError::ListAndParseBootEntries)?;

    bootmgr_output
        .get_boot_entry_number(entry_label)
        .structured(ServicingError::ReadEfibootmgr)
}

/// Returns the ESP partition device id from Engine Context
fn get_esp_device_id(ctx: &EngineContext) -> Result<BlockDeviceId, Error> {
    Ok(ctx
        .spec
        .storage
        .esp_filesystem()
        .map(|(id, _)| id)
        .context("Host Configuration does not contain an ESP filesystem.")?
        .clone())
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
    let device_path = ctx.partition_paths.get(esp_device_id).with_context(|| {
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
        .join(BOOT_EFI);

    Ok((esp_dir_name, path))
}

/// Lists EFI boot manager entries, checks if the `BootOrder` requires
/// updates based on the given boot entry, and updates the `BootOrder` if
/// needed according to the specified position.
///
#[tracing::instrument(skip_all)]
pub fn first_or_last_boot_order(
    boot_entry: &String,
    boot_order_position: &BootOrderPosition,
) -> Result<(), Error> {
    let bootmgr_output: EfiBootManagerOutput = efibootmgr::list_and_parse_bootmgr_entries()
        .context("Failed to list and parse boot manager entries")?;

    let new_boot_order = generate_new_boot_order(&bootmgr_output, boot_entry, boot_order_position);

    if let Some(new_boot_order) = new_boot_order {
        debug!("Modifying `BootOrder` to {}", new_boot_order);
        efibootmgr::modify_boot_order(&new_boot_order)
            .unstructured(format!("Failed to modify `BootOrder` to {new_boot_order}"))?;
    } else {
        debug!("Skipping `BootOrder` modification as it is already up-to-date");
    }

    Ok(())
}

/// This function ensures that the specified boot entries are added to the `BootOrder`
/// according to the specified position.
///
/// #[tracing::instrument(skip_all)]
pub fn update_boot_order(
    boot_current_entries: Vec<String>,
    boot_order_position: &BootOrderPosition,
) -> Result<(), Error> {
    for added_entry_number in boot_current_entries.iter().rev() {
        debug!(
            "Adding boot entry '{}' to the beginning of `BootOrder`",
            added_entry_number
        );
        first_or_last_boot_order(added_entry_number, boot_order_position)?;
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
    boot_order_position: &BootOrderPosition,
) -> Option<String> {
    let mut boot_order_initial: Vec<String> = bootmgr_output.boot_order.clone();

    let add_to_boot_order = |bo: &mut Vec<String>| match boot_order_position {
        BootOrderPosition::First => {
            bo.insert(0, boot_entry.to_string());
        }
        BootOrderPosition::Last => {
            bo.push(boot_entry.to_string());
        }
    };

    if boot_order_initial.contains(boot_entry) {
        if let Some(index) = boot_order_initial.iter().position(|x| x == boot_entry) {
            match boot_order_position {
                BootOrderPosition::First => {
                    if index == 0 {
                        // Boot entry is already at the first position in `BootOrder`. No need to modify.
                        return None;
                    }
                }
                BootOrderPosition::Last => {
                    if index == boot_order_initial.len() - 1 {
                        // Boot entry is already at the last position in `BootOrder`. No need to modify.
                        return None;
                    }
                }
            };
            // Boot entry is part of `BootOrder` but not at the first position. Move it to the first position.
            boot_order_initial.remove(index);
            add_to_boot_order(&mut boot_order_initial);
        }
    } else {
        add_to_boot_order(&mut boot_order_initial);
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
            AbUpdate, Disk, FileSystem, FileSystemSource, HostConfiguration, ImageSha384,
            MountOptions, MountPoint, OsImage, Partition, PartitionSize, PartitionType, Raid,
            SoftwareRaidArray, Storage,
        },
        error::ErrorKind,
        status::{AbVolumeSelection, ServicingType},
    };
    use url::Url;

    use super::*;
    use boot::get_update_esp_dir_name;

    use constants::ESP_MOUNT_POINT_PATH;

    /// Validates logic for determining which A/B volume to use for updates
    #[test]
    fn test_get_label_and_path() {
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: Storage {
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
                    .join(BOOT_EFI)
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
                    .join(BOOT_EFI)
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

        // Test first-case where boot entry is already at the first position in `BootOrder`
        let result = generate_new_boot_order(
            &bootmgr_output,
            &String::from("0001"),
            &BootOrderPosition::First,
        );
        assert_eq!(result, None);

        // Test first-case where boot entry is not part of `BootOrder`
        let result = generate_new_boot_order(
            &bootmgr_output,
            &String::from("0002"),
            &BootOrderPosition::First,
        );
        assert_eq!(result, Some("0002,0001,0000".to_string()));

        // Test first-case where boot entry is part of `BootOrder` but not at the first position
        let result = generate_new_boot_order(
            &bootmgr_output,
            &String::from("0000"),
            &BootOrderPosition::First,
        );
        assert_eq!(result, Some("0000,0001".to_string()));

        // Test last-case where boot entry is not part of `BootOrder`
        let result = generate_new_boot_order(
            &bootmgr_output,
            &String::from("0002"),
            &BootOrderPosition::Last,
        );
        assert_eq!(result, Some("0001,0000,0002".to_string()));
        // Test last-case where boot entry is part of `BootOrder` in the last position
        let result = generate_new_boot_order(
            &bootmgr_output,
            &String::from("0000"),
            &BootOrderPosition::Last,
        );
        assert_eq!(result, None);
        // Test last-case where boot entry is part of `BootOrder` but not in the last position
        let result = generate_new_boot_order(
            &bootmgr_output,
            &String::from("0001"),
            &BootOrderPosition::Last,
        );
        assert_eq!(result, Some("0000,0001".to_string()));
    }

    pub(crate) fn get_esp_on_raid_ctx() -> EngineContext {
        EngineContext {
            spec: HostConfiguration {
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/image").unwrap(),
                    sha384: ImageSha384::Ignored,
                }),
                storage: Storage {
                    filesystems: vec![FileSystem {
                        source: FileSystemSource::Image,
                        device_id: Some("esp".to_string()),
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
                    raid: Raid {
                        software: vec![SoftwareRaidArray {
                            id: "esp".into(),
                            name: "esp".to_string(),
                            level: RaidLevel::Raid1,
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
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/image").unwrap(),
                    sha384: ImageSha384::Ignored,
                }),
                storage: Storage {
                    filesystems: vec![FileSystem {
                        source: FileSystemSource::Image,
                        device_id: Some("esp".to_string()),
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
        ctx.spec.storage.raid.software[0].level = RaidLevel::Raid0;

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

    #[test]
    fn test_get_entry_labels() {
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: Storage {
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            get_entry_labels(ctx.install_index).unwrap(),
            ["AZLA".to_string(), "AZLB".to_string()]
        );

        assert_eq!(
            get_entry_labels(ctx.install_index).unwrap(),
            ["AZLA".to_string(), "AZLB".to_string()]
        );

        ctx.install_index = 1;
        assert_eq!(
            get_entry_labels(ctx.install_index).unwrap(),
            ["AZL2A".to_string(), "AZL2B".to_string()]
        );

        ctx.install_index = 0;
        assert_eq!(
            get_entry_labels(ctx.install_index).unwrap(),
            ["AZLA".to_string(), "AZLB".to_string()]
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::{iter::Iterator, str::FromStr};

    use url::Url;

    use osutils::{
        efibootmgr::{self, EfiBootManagerOutput},
        files::create_file,
        path::join_relative,
        sfdisk,
        testutils::repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH},
    };
    use pytest_gen::functional_test;
    use trident_api::config::{
        self, Disk, HostConfiguration, ImageSha384, MountOptions, MountPoint, OsImage, Partition,
        PartitionSize, PartitionType,
    };

    use crate::engine::{storage::partitioning, EngineContext};

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
        let bootloader_path = Path::new(r"/EFI/AZLA").join(BOOT_EFI);
        // create_boot_entry() will call is_valid_bootloader_path() to verify if file exists at
        // {tempdir}/{bootloader_path}. So, create a dummy bootloader file
        let bootloader_file_path = join_relative(tempdir.path(), &bootloader_path);
        create_file(bootloader_file_path).unwrap();

        efibootmgr::create_boot_entry(
            "TestBoot1",
            OS_DISK_DEVICE_PATH,
            &bootloader_path,
            tempdir.path(),
            1,
            false,
        )
        .unwrap();
        efibootmgr::create_boot_entry(
            "TestBoot2",
            OS_DISK_DEVICE_PATH,
            &bootloader_path,
            tempdir.path(),
            2,
            false,
        )
        .unwrap();
        efibootmgr::create_boot_entry(
            "TestBoot3",
            OS_DISK_DEVICE_PATH,
            &bootloader_path,
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
        let esp_partition_path = ctx.partition_paths.get(&esp_id).unwrap();
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
        let esp_partition_path = ctx.partition_paths.get(&esp_id1).unwrap();
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
    fn test_first_or_last_boot_order_when_update_success() {
        delete_boot_next();
        set_some_boot_entries();

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let boot_current = &bootmgr_output.boot_current;
        let initial_boot_order = bootmgr_output.boot_order;

        // Test that target was able to boot into the updated partition.
        first_or_last_boot_order(boot_current, &BootOrderPosition::First).unwrap();

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
    fn test_first_or_last_boot_order_skip_boot_order_update() {
        delete_boot_next();
        set_some_boot_entries();

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let initial_boot_order = bootmgr_output.boot_order;
        let boot_entry = initial_boot_order[0].clone();

        first_or_last_boot_order(&boot_entry, &BootOrderPosition::First).unwrap();

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

        update_boot_order(
            vec![boot_entry1.clone(), boot_entry2.clone()],
            &BootOrderPosition::First,
        )
        .unwrap();

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
        let bootloader_path = Path::new(r"/EFI/AZLA").join(BOOT_EFI);
        // create_boot_entry() will call is_valid_bootloader_path() to verify if file exists at
        // {tempdir}/{bootloader_path}. So, create a dummy bootloader file
        let bootloader_file_path = join_relative(tempdir.path(), &bootloader_path);
        create_file(bootloader_file_path).unwrap();

        efibootmgr::create_boot_entry(
            boot_current_label.clone(),
            OS_DISK_DEVICE_PATH,
            &bootloader_path,
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
            &bootloader_path,
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

        persist_boot_order().unwrap();

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

    fn get_esp_on_raid_ctx() -> EngineContext {
        EngineContext {
            spec: HostConfiguration {
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/image").unwrap(),
                    sha384: ImageSha384::Ignored,
                }),
                storage: trident_api::config::Storage {
                    filesystems: vec![config::FileSystem {
                        source: config::FileSystemSource::Image,
                        device_id: Some("esp".to_string()),
                        mount_point: Some(MountPoint {
                            path: ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                    }],
                    disks: vec![
                        Disk {
                            id: "disk1".into(),
                            device: TEST_DISK_DEVICE_PATH.into(),
                            partitions: vec![Partition {
                                id: "esp1".into(),
                                size: PartitionSize::from_str("512M").unwrap(),
                                partition_type: PartitionType::Esp,
                            }],
                            ..Default::default()
                        },
                        Disk {
                            id: "disk2".into(),
                            device: TEST_DISK_DEVICE_PATH.into(),
                            partitions: vec![Partition {
                                id: "esp2".into(),
                                size: PartitionSize::from_str("512M").unwrap(),
                                partition_type: PartitionType::Esp,
                            }],
                            ..Default::default()
                        },
                    ],
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

    fn create_test_boot_entries(
        labels: Vec<String>,
    ) -> (Vec<String>, Vec<String>, Vec<EspDeviceMetadata>) {
        let tempdir = tempfile::tempdir().unwrap();
        // Create bootloader path
        let bootloader_path = Path::new(r"/EFI/TESTA/").join(BOOT_EFI);
        // create_boot_entry() will call is_valid_bootloader_path() to verify if file exists at
        // {tempdir}/{bootloader_path}. So, create a dummy bootloader file
        let bootloader_file_path = join_relative(tempdir.path(), &bootloader_path);
        create_file(bootloader_file_path).unwrap();

        let bootloader_path = Path::new(r"/EFI/TESTB").join(BOOT_EFI);
        // create_boot_entry() will call is_valid_bootloader_path() to verify if file exists at
        // {tempdir}/{bootloader_path}. So, create a dummy bootloader file
        let bootloader_file_path = join_relative(tempdir.path(), &bootloader_path);
        create_file(bootloader_file_path).unwrap();

        for label in &labels {
            efibootmgr::create_boot_entry(
                label,
                OS_DISK_DEVICE_PATH,
                &bootloader_path,
                tempdir.path(),
                3,
                false,
            )
            .unwrap();

            efibootmgr::create_boot_entry(
                label,
                OS_DISK_DEVICE_PATH,
                &bootloader_path,
                tempdir.path(),
                3,
                true,
            )
            .unwrap();
        }

        let bootloader_path1 = Path::new(r"/boot/efi/EFI/TESTA").join(BOOT_EFI);
        create_file(bootloader_path1).unwrap();
        let bootloader_path2 = Path::new(r"/boot/efi/EFI/TESTB").join(BOOT_EFI);
        create_file(bootloader_path2).unwrap();

        let bootmgr_output_initial: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();

        // Get all the entries with label "TESTA" and "TESTB"
        let testa_entries = bootmgr_output_initial.get_entries_with_label("TESTA");

        let testb_entries = bootmgr_output_initial.get_entries_with_label("TESTB");

        for label in &labels {
            // Get the entry number of the boot entry
            let entry_number = efibootmgr::list_and_parse_bootmgr_entries()
                .unwrap()
                .get_boot_entry_number(label)
                .unwrap();
            // Delete one entry from each label
            efibootmgr::delete_boot_entry(&entry_number).unwrap();
        }

        let esp_meta_data_vec = vec![
            EspDeviceMetadata {
                id: "esp1".into(),
                path: PathBuf::from(OS_DISK_DEVICE_PATH),
            },
            EspDeviceMetadata {
                id: "esp2".into(),
                path: PathBuf::from(TEST_DISK_DEVICE_PATH),
            },
        ];

        (testa_entries, testb_entries, esp_meta_data_vec)
    }

    fn cleanup(expected_output: Vec<String>) {
        // Cleanup
        // Delete all the entry numbers in the expected_output
        for entry_number in &expected_output {
            efibootmgr::delete_boot_entry(entry_number).unwrap();
        }
        // Delete the files created
        let _ = std::fs::remove_file(Path::new("/boot/efi/EFI/TESTA").join(BOOT_EFI));
        let _ = std::fs::remove_file(Path::new("/boot/efi/EFI/TESTB").join(BOOT_EFI));
    }

    #[functional_test]
    fn test_create_and_get_boot_entry_order_after_rebuilding_a_b_boot_order() {
        let labels = vec!["TESTA".to_string(), "TESTB".to_string()];
        let (testa_entries, testb_entries, esp_meta_data_vec) =
            create_test_boot_entries(labels.clone());

        let disks_to_rebuild = vec!["disk2".to_string()];
        let mut ctx = get_esp_on_raid_ctx();

        // Set up the environment
        // Create the esp2 partition
        let mut ctx1 = tests::get_esp_on_raid_ctx();
        partitioning::create_partitions(&mut ctx1).unwrap();
        // Get the UUID path of the ESP partition from ctx1.
        let esp_uuid_path = Box::new(ctx1.partition_paths.get("esp2")).unwrap();
        // Add this to the ctx partition_paths
        ctx.partition_paths
            .insert("esp2".to_string(), esp_uuid_path.clone());

        // TestCase 1 : where active volume is VolumeA
        ctx.ab_active_volume = Some(trident_api::status::AbVolumeSelection::VolumeA);
        // Append testa_entries and testb_entries and create a vector of all the entries
        let expected_output: Vec<String> = testa_entries
            .iter()
            .chain(testb_entries.iter())
            .cloned()
            .collect();
        let output = create_and_get_boot_entry_order_after_rebuilding(
            &ctx,
            labels.clone(),
            &disks_to_rebuild,
            esp_meta_data_vec.clone(),
        )
        .unwrap();

        assert_eq!(output, expected_output.clone());

        cleanup(expected_output);
    }

    #[functional_test]
    fn test_create_and_get_boot_entry_order_after_rebuilding_b_a_boot_order() {
        let labels = vec!["TESTA".to_string(), "TESTB".to_string()];
        let (testa_entries, testb_entries, esp_meta_data_vec) =
            create_test_boot_entries(labels.clone());

        let disks_to_rebuild = vec!["disk2".to_string()];
        let mut ctx = get_esp_on_raid_ctx();

        // Set up the environment to test create_and_get_boot_entry_numbers() function
        // Create the esp2 partition
        let mut ctx1 = tests::get_esp_on_raid_ctx();
        partitioning::create_partitions(&mut ctx1).unwrap();
        // Get the UUID path of the ESP partition from ctx1.
        let esp_uuid_path = Box::new(ctx1.partition_paths.get("esp2")).unwrap();
        // Add this to the ctx partition_paths
        ctx.partition_paths
            .insert("esp2".to_string(), esp_uuid_path.clone());

        // TestCase : where active volume is VolumeB
        ctx.ab_active_volume = Some(trident_api::status::AbVolumeSelection::VolumeB);
        // Append testa_entries and testb_entries and create a vector of all the entries
        let expected_output: Vec<String> = testb_entries
            .iter()
            .chain(testa_entries.iter())
            .cloned()
            .collect();

        let output = create_and_get_boot_entry_order_after_rebuilding(
            &ctx,
            labels,
            &disks_to_rebuild,
            esp_meta_data_vec.clone(),
        )
        .unwrap();

        assert_eq!(output, expected_output.clone());

        cleanup(expected_output);
    }

    #[functional_test]
    fn test_create_bootentries_after_rebuilding_active_volume_a() {
        let labels = vec!["TESTA".to_string(), "TESTB".to_string()];
        let (_testa_entries, _testb_entries, _esp_meta_data_vec) =
            create_test_boot_entries(labels.clone());

        let mut ctx = get_esp_on_raid_ctx();

        // Set up the environment
        // Create the esp2 partition
        let mut ctx1 = tests::get_esp_on_raid_ctx();
        partitioning::create_partitions(&mut ctx1).unwrap();
        // Get the UUID path of the ESP partition from ctx1.
        let esp_uuid_path1 = Box::new(ctx1.partition_paths.get("esp1")).unwrap();

        // Add this to the ctx partition_paths
        ctx.partition_paths
            .insert("esp1".to_string(), esp_uuid_path1.clone());
        let esp_uuid_path2 = Box::new(ctx1.partition_paths.get("esp2")).unwrap();

        // Add this to the ctx partition_paths
        ctx.partition_paths
            .insert("esp2".to_string(), esp_uuid_path2.clone());

        // TestCase 1 : where active volume is VolumeA
        ctx.ab_active_volume = Some(trident_api::status::AbVolumeSelection::VolumeA);

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        print!("{bootmgr_output:?}");

        let disk1_uuid = sfdisk::get_disk_uuid(&PathBuf::from("/dev/sda"))
            .unwrap()
            .unwrap();
        ctx.disk_uuids
            .insert("disk1".to_string(), disk1_uuid.as_uuid().unwrap());

        let disks_to_rebuild = vec!["disk2".to_string()];
        create_and_update_boot_variables_after_rebuilding(&ctx, labels, &disks_to_rebuild).unwrap();

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        print!("{bootmgr_output:?}");
        // Get all the entries with label "TESTA"
        let testa_entries = bootmgr_output.get_entries_with_label("TESTA");
        // Get all the entries with label "TESTB"
        let testb_entries = bootmgr_output.get_entries_with_label("TESTB");
        assert_eq!(testa_entries.len(), 2);
        assert_eq!(testb_entries.len(), 2);

        // Check BootNext when active volume is VolumeA
        assert_eq!(bootmgr_output.boot_next, testa_entries[0]);

        // Cleanup
        cleanup(testa_entries);
        cleanup(testb_entries);
    }

    #[functional_test]
    fn test_create_bootentries_after_rebuilding_active_volume_b() {
        let labels = vec!["TESTA".to_string(), "TESTB".to_string()];
        let (_testa_entries, _testb_entries, _esp_meta_data_vec) =
            create_test_boot_entries(labels.clone());

        let mut ctx = get_esp_on_raid_ctx();

        // Set up the environment
        // Create the esp2 partition
        let mut ctx1 = tests::get_esp_on_raid_ctx();
        partitioning::create_partitions(&mut ctx1).unwrap();
        // Get the UUID path of the ESP partition from ctx1.
        let esp_uuid_path1 = Box::new(ctx1.partition_paths.get("esp1")).unwrap();

        // Add this to the ctx partition_paths
        ctx.partition_paths
            .insert("esp1".to_string(), esp_uuid_path1.clone());
        let esp_uuid_path2 = Box::new(ctx1.partition_paths.get("esp2")).unwrap();

        // Add this to the ctx partition_paths
        ctx.partition_paths
            .insert("esp2".to_string(), esp_uuid_path2.clone());

        // TestCase 2 : where active volume is VolumeB
        ctx.ab_active_volume = Some(trident_api::status::AbVolumeSelection::VolumeB);

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        print!("{bootmgr_output:?}");

        let disk1_uuid = sfdisk::get_disk_uuid(&PathBuf::from("/dev/sda"))
            .unwrap()
            .unwrap();
        ctx.disk_uuids
            .insert("disk1".to_string(), disk1_uuid.as_uuid().unwrap());

        let disks_to_rebuild = vec!["disk2".to_string()];
        create_and_update_boot_variables_after_rebuilding(&ctx, labels, &disks_to_rebuild).unwrap();

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();

        // Get all the entries with label "TESTA"
        let testa_entries = bootmgr_output.get_entries_with_label("TESTA");
        // Get all the entries with label "TESTB"
        let testb_entries = bootmgr_output.get_entries_with_label("TESTB");
        assert_eq!(testa_entries.len(), 2);
        assert_eq!(testb_entries.len(), 2);

        // Check BootNext when active volume is VolumeB
        assert_eq!(bootmgr_output.boot_next, testb_entries[0]);

        // Cleanup
        cleanup(testa_entries);
        cleanup(testb_entries);
    }
}
