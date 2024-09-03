use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use log::debug;
use osutils::efibootmgr::EfiBootManagerOutput;
use osutils::{block_devices, efibootmgr};
use trident_api::constants;
use trident_api::error::{InternalError, ReportError, ServicingError, TridentError};
use trident_api::status::HostStatus;

/// Boot efi executable
const BOOT64_EFI: &str = "bootx64.efi";

/// Creates a boot entry for the A/B update volume and sets the `BootNext`
/// variable to boot from the updated partition on next boot. Also updates the
/// `BootOrder` for non-qemu targets.
///
/// Takes in the path where we expect to find the entry matching the install ID.
/// During clean install, this corresponds to /mnt/newroot/boot/efi, but during
/// A/B update, both A and B share a single ESP at /boot/efi.
pub fn set_boot_next_and_update_boot_order(
    host_status: &HostStatus,
    esp_path: &Path,
) -> Result<(), TridentError> {
    // Get the label and path for the EFI boot loader of the inactive A/B update volume.
    let (entry_label_new, bootloader_path_new) =
        get_label_and_path(host_status).structured(ServicingError::GetLabelandPath)?;

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
            .structured(ServicingError::DeleteEntries {
                boot_entry: entry_label_new.clone(),
            })?;
        // Get boot entry numbers for the entries with label '{entry_label_new}'
        let entry_numbers = bootmgr_output
            .get_entries_with_label(&entry_label_new)
            .structured(ServicingError::ReadEfibootmgr)?;
        // Get the current `BootOrder`
        let current_boot_order = bootmgr_output
            .get_boot_order()
            .structured(ServicingError::ReadEfibootmgr)?;
        // Get the modified `BootOrder` after removing the entries with label '{entry_label_new}'
        let new_boot_order: Vec<String> = current_boot_order
            .iter()
            .filter(|&x| !entry_numbers.contains(x))
            .map(|x| x.to_string())
            .collect();

        // Get the updated `BootOrder`
        let new_boot_order_after_deletion = efibootmgr::list_and_parse_bootmgr_entries()
            .structured(ServicingError::ListAndParseBootEntries)?
            .get_boot_order()
            .structured(ServicingError::ReadEfibootmgr)?;

        // If the `BootOrder` has changed, update the `BootOrder`
        if current_boot_order != new_boot_order && new_boot_order_after_deletion != new_boot_order {
            efibootmgr::modify_boot_order(new_boot_order.join(",").as_str())
                .structured(ServicingError::ModifyBootOrder)?;
        }
    }

    // Get the disk path of the ESP partition
    let disk_path =
        get_esp_partition_disk(host_status).structured(InternalError::GetEspPartitionDiskPath)?;
    debug!("Disk path of first ESP partition {:?}", disk_path);

    // Create a boot entry for the new OS.
    efibootmgr::create_boot_entry(&entry_label_new, disk_path, bootloader_path_new, esp_path)
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

    // Set the `BootNext` variable to boot from the newly added entry on next boot.
    efibootmgr::set_boot_next(&added_entry_number).structured(ServicingError::SetBootNext)?;
    debug!("Set `BootNext` to newly added entry '{added_entry_number}'");

    // HACK: detect if we're inside qemu to avoid modifying `BootOrder`
    // TODO(#7139): remove this special case.
    if !osutils::virt::is_qemu() {
        first_boot_order(&added_entry_number).structured(ServicingError::SetBootOrder {
            boot_entry_number: added_entry_number,
        })?;
    }
    Ok(())
}

/// Returns the path of the disk containing the ESP partition.
///
/// The information is obtained from the filesystem configuration in the host
/// configuration (`spec` field inside HostStatus). We currently only support
/// one ESP partition per host, so we pick the first one we find.
fn get_esp_partition_disk(host_status: &HostStatus) -> Result<PathBuf, Error> {
    // TODO: What about deployments with multiple ESP partitions? (in multiple disks)
    // This implementation just finds the first ESP filesystem and uses that.

    // Find the device ID of the ESP filesystem
    let esp_device_id = host_status
        .spec
        .storage
        .filesystems
        .iter()
        .find_map(|fs| fs.source.esp_image().and(fs.device_id.as_ref()))
        .context("Host configuration does not contain any ESP file systems.")?;

    // Find the device path of the ESP partition
    let device_path = host_status
        .storage
        .block_device_paths
        .get(esp_device_id)
        .with_context(|| {
            format!("Failed to find device path for ESP partition with device ID '{esp_device_id}'")
        })?;

    debug!(
        "Found ESP partition '{esp_device_id}' with device path '{}'",
        device_path.display()
    );

    block_devices::block_device_by_path(
        block_devices::get_disk_for_partition(device_path.as_path()).with_context(|| {
            format!(
                "Failed to get disk for ESP partition '{esp_device_id}' with device path '{}'",
                device_path.display()
            )
        })?,
    )
    .context("Failed to get by-path symlink for disk")
}

/// Retrieves the label and path for the EFI boot loader of the inactive A/B update volume.
///
/// This function takes a reference to a `HostStatus` object and returns a tuple containing
/// the label associated with the inactive A/B update volume and the path to its EFI boot loader.
///
fn get_label_and_path(host_status: &HostStatus) -> Result<(String, PathBuf), Error> {
    let esp_dir_name = host_status
        .get_update_esp_dir_name()
        .context("Failed to get install id")?;

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
            .context(format!("Failed to modify `BootOrder` to {new_boot_order}"))?;
    } else {
        debug!("Skipping `BootOrder` modification as it is already up-to-date");
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
    use osutils::efibootmgr::EfiBootEntry;
    use trident_api::{
        config::{self, AbUpdate, HostConfiguration},
        status::{AbVolumeSelection, ServicingState, ServicingType},
    };

    use super::*;

    /// Validates logic for determining which A/B volume to use for updates
    #[test]
    fn test_get_label_and_path() {
        let mut host_status = HostStatus {
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
            servicing_state: ServicingState::Staging,
            ..Default::default()
        };

        // Test that clean-install will always use volume A for updates
        assert_eq!(
            get_label_and_path(&host_status).unwrap(),
            (
                host_status.get_update_esp_dir_name().unwrap(),
                Path::new(constants::ROOT_MOUNT_POINT_PATH)
                    .join(constants::ESP_EFI_DIRECTORY)
                    .join(host_status.get_update_esp_dir_name().unwrap())
                    .join(BOOT64_EFI)
            )
        );

        // Test that servicing types HotPatch, NormalUpdate, UpdateAndReboot will always use the
        // active volume for updates
        host_status.servicing_type = ServicingType::NormalUpdate;
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_label_and_path(&host_status).unwrap(),
            (
                host_status.get_update_esp_dir_name().unwrap(),
                Path::new(constants::ROOT_MOUNT_POINT_PATH)
                    .join(constants::ESP_EFI_DIRECTORY)
                    .join(host_status.get_update_esp_dir_name().unwrap())
                    .join(BOOT64_EFI)
            )
        );

        // Test that servicing type NoActiveServicing will return None
        host_status.servicing_type = ServicingType::NoActiveServicing;
        let error_message = get_label_and_path(&host_status).unwrap_err().to_string();
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
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use crate::engine::storage::partitioning::create_partitions;

    use super::*;
    use constants::ESP_MOUNT_POINT_PATH;

    use trident_api::config::{
        self, Disk, FileSystemType, HostConfiguration, MountOptions, MountPoint, Partition,
        PartitionSize, PartitionType,
    };

    use osutils::{
        efibootmgr::{self, EfiBootManagerOutput},
        files::create_file,
        path::join_relative,
        testutils::repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH},
    };
    use pytest_gen::functional_test;

    use std::{iter::Iterator, str::FromStr};

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
        )
        .unwrap();
        efibootmgr::create_boot_entry(
            "TestBoot2",
            OS_DISK_DEVICE_PATH,
            bootloader_path,
            tempdir.path(),
        )
        .unwrap();
        efibootmgr::create_boot_entry(
            "TestBoot3",
            OS_DISK_DEVICE_PATH,
            bootloader_path,
            tempdir.path(),
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
    fn test_get_esp_partition_disk() {
        let mut host_status = HostStatus {
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
        };

        // Borrow checker bypass :)
        let config = host_status.spec.clone();
        create_partitions(&mut host_status, &config).unwrap();

        let disk_path = get_esp_partition_disk(&host_status).unwrap();

        let canon_path = disk_path.canonicalize().unwrap();

        assert_eq!(
            canon_path.as_path(),
            Path::new(TEST_DISK_DEVICE_PATH),
            "Disk path mismatch"
        );
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
}
