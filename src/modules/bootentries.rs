use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use log::{debug, info};
use osutils::efibootmgr::EfiBootManagerOutput;
use osutils::{block_devices, efibootmgr};
use trident_api::constants;
use trident_api::error::{ReportError, ServicingError, TridentError, TridentResultExt};
use trident_api::status::HostStatus;

use crate::datastore::DataStore;

/// Boot efi executable
const BOOT64_EFI: &str = "bootx64.efi";

/// Calls the set_boot_next to set the `BootNext` variable and then updates the host status.
///
/// This function first sets the `BootNext` variable by calling set_boot_next. Then, it retrieves
/// the output of `efibootmgr` to get information about the boot manager entries. Finally, it\
/// updates the host status with the retrieved `BootNext` variable.
///
pub fn call_set_boot_next_and_update_hs(
    host_status: &mut HostStatus,
    esp_path: &Path,
) -> Result<(), TridentError> {
    set_boot_next(host_status, esp_path).structured(ServicingError::SetBootNext)?;

    // Get the output of efibootmgr
    let bootmgr_output: EfiBootManagerOutput = efibootmgr::list_and_parse_bootmgr_entries()
        .structured(ServicingError::ListAndParseBootEntries)?;

    // Update host status with BootNext variable
    host_status.boot_next = if !bootmgr_output.boot_next.is_empty() {
        debug!(
            "Setting boot_next in host status to {:?}",
            bootmgr_output.boot_next
        );
        Some(bootmgr_output.boot_next)
    } else {
        debug!("Setting boot_next in host status to none");
        None
    };

    Ok(())
}

/// Creates a boot entry for the A/B update volume and sets the `BootNext`
/// variable to boot from the updated partition on next boot.
///
/// Takes in the path where we expect to find the entry matching the install ID.
/// During clean install, this corresponds to /mnt/newroot/boot/efi, but during
/// A/B update, both A and B share a single ESP at /boot/efi.
fn set_boot_next(host_status: &HostStatus, esp_path: &Path) -> Result<(), Error> {
    // Get the label and path for the EFI boot loader of the inactive A/B update volume.
    let (entry_label_new, bootloader_path_new) =
        get_label_and_path(host_status).context("Failed to get label and path")?;

    // Check if the boot entry already exists, if so, delete the entry and
    // remove it from the boot order.
    let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()?;
    if bootmgr_output.boot_entry_exists(&entry_label_new)? {
        debug!("Boot entry already exists, deleting entries with label '{entry_label_new}'");
        bootmgr_output.delete_entries_with_label(&entry_label_new)?;
        // Get boot entry numbers for the entries with label '{entry_label_new}'
        let entry_numbers = bootmgr_output.get_entries_with_label(&entry_label_new)?;
        // Get the current boot order
        let current_boot_order = bootmgr_output.get_boot_order()?;
        // Get the modified boot order after removing the entries with label '{entry_label_new}'
        let new_boot_order: Vec<String> = current_boot_order
            .iter()
            .filter(|&x| !entry_numbers.contains(x))
            .map(|x| x.to_string())
            .collect();

        // Get the updated boot order
        let new_boot_order_after_deletion =
            efibootmgr::list_and_parse_bootmgr_entries()?.get_boot_order()?;

        if current_boot_order != new_boot_order && new_boot_order_after_deletion != new_boot_order {
            efibootmgr::modify_boot_order(new_boot_order.join(",").as_str())?;
        }
    }

    // Get the disk path of the ESP partition
    let disk_path = get_esp_partition_disk(host_status).context("Failed to fetch esp disk path")?;
    debug!("Disk path of first esp partition {:?}", disk_path);

    // Create a boot entry for the new OS.
    efibootmgr::create_boot_entry(&entry_label_new, disk_path, bootloader_path_new, esp_path)
        .context(format!(
            "Failed to add boot entry with label '{entry_label_new}'"
        ))?;
    let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()?;

    let added_entry_number = bootmgr_output
        .get_boot_entry_number(&entry_label_new)
        .context("Failed to get boot entry number")?;
    debug!("Added boot entry: {added_entry_number}");

    // HACK: detect if we're inside qemu to avoid modifying boot order
    // TODO(#7139): remove this special case.
    if !osutils::virt::is_qemu() {
        let mut boot_order = bootmgr_output.get_boot_order()?;
        boot_order.push(added_entry_number.clone());
        efibootmgr::modify_boot_order(&boot_order.join(","))
            .context("Failed to append new entry to boot order")?;
        debug!("Appended entry to boot order");
    }

    efibootmgr::set_boot_next(&added_entry_number).context("Failed to get set `BootNext`")?;
    debug!("Set `BootNext` to new entry");

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

/// Sets the EFI boot variables based on host status and current boot order.
/// This function opens the Trident DataStore, retrieves host status, lists EFI boot entries,
/// determines whether the boot order needs modification, and performs the necessary actions.
///
// TODO - https://dev.azure.com/mariner-org/ECF/_workitems/edit/6807 needs refactoring
#[tracing::instrument(skip_all)]
pub fn set_boot_order(datastore_path: &Path) -> Result<(), TridentError> {
    let mut datastore = DataStore::open(datastore_path)
        .message("Failed to open datastore while setting boot order")?;
    let host_status = datastore.host_status();

    let bootmgr_output: EfiBootManagerOutput = efibootmgr::list_and_parse_bootmgr_entries()
        .structured(ServicingError::ListAndParseBootEntries)?;

    let (new_boot_order, clear_boot_next) = update_efi_boot_order(host_status, &bootmgr_output);

    if let Some(new_boot_order) = new_boot_order {
        debug!("Modifying boot order to: {}", new_boot_order);
        efibootmgr::modify_boot_order(&new_boot_order)
            .structured(ServicingError::ModifyBootOrder)?;
    } else {
        info!("Boot order not modified");
    }

    if clear_boot_next {
        debug!("Clearing boot_next variable from host status");
        datastore.with_host_status(|s| {
            s.boot_next = None;
        })?;
    }

    Ok(())
}

/// Analyzes whether the EFI boot order should be modified based on the `boot_next` value in host status.
///
/// If the `boot_next` value is set, this function compares it with the current EFI boot entries
/// and adjusts the boot order accordingly.
///
/// # Returns
/// - `new_boot_order`: A string representing the new boot order after adjustments.
/// - `clear_boot_next`: A boolean indicating whether the `boot_next` variable needs to be cleared.
///
#[tracing::instrument(skip_all)]
fn update_efi_boot_order(
    host_status: &HostStatus,
    bootmgr_output: &EfiBootManagerOutput,
) -> (Option<String>, bool) {
    if let Some(hs_boot_next) = &host_status.boot_next {
        if !bootmgr_output.boot_next.is_empty() {
            info!("Bootnext is not empty, Trident reran before trying to reboot from the updated partition");
            return (None, false);
        }
        let mut boot_order_initial: Vec<String> = bootmgr_output.boot_order.clone();
        let boot_current = &bootmgr_output.boot_current;
        if boot_current.as_str() == *hs_boot_next {
            info!("Booted from the updated partition");
            if boot_order_initial.contains(boot_current) {
                if let Some(index) = boot_order_initial.iter().position(|x| x == boot_current) {
                    if index != 0 {
                        boot_order_initial.remove(index);
                        boot_order_initial.insert(0, boot_current.to_string());
                    } else {
                        debug!("Boot_current is already at the first position in boot_order");
                        return (None, true);
                    }
                }
            } else {
                boot_order_initial.insert(0, boot_current.to_string());
            }
            let new_boot_order_str = boot_order_initial.join(",");
            return (Some(new_boot_order_str), true);
        } else {
            info!("Booted from the old partition");
        }
    } else {
        debug!("Bootnext is None, skipping update of boot order");
        return (None, false);
    }

    (None, true)
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
            servicing_type: Some(ServicingType::CleanInstall),
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
        host_status.servicing_type = Some(ServicingType::NormalUpdate);
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

        // Test that servicing type None will return None
        host_status.servicing_type = None;
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

    /// This function sets the host_status.boot_next to the boot_current variable of efibootmgr and checks if the boot order is updated with boot_next as the first entry.
    /// Which indicates that the target was able to boot into the updated partition and hence boot order was modified with boot_next as the first entry.
    #[test]
    fn test_boot_order_when_booted_from_updated_partition() {
        let host_status = HostStatus {
            boot_next: Some(String::from("0003")),
            ..Default::default()
        };

        let bootmgr_output = get_bootmgr_output();
        let result = update_efi_boot_order(&host_status, &bootmgr_output);
        assert_eq!(result, (Some(String::from("0003,0001,0000")), true));
    }

    /// This function sets the host_status.boot_next not equal to the boot_current variable of efibootmgr.
    /// Which indicates that the target was not able to boot into the updated partition and the test verifies that boot order was not modified.
    #[test]
    fn test_boot_order_when_booted_from_old_partition() {
        let host_status = HostStatus {
            boot_next: Some(String::from("0001")),
            ..Default::default()
        };
        let bootmgr_output = get_bootmgr_output();
        let result = update_efi_boot_order(&host_status, &bootmgr_output);
        assert_eq!(result, (None, true));
    }

    /// This function sets the host_status.boot_next to none.
    /// Which indicates that there was no update and hence the test verifies that boot order was not modified.
    #[test]
    fn test_boot_order_when_boot_next_none() {
        let host_status = HostStatus {
            ..Default::default()
        };

        let bootmgr_output = get_bootmgr_output();
        let result = update_efi_boot_order(&host_status, &bootmgr_output);
        assert_eq!(result, (None, false));
    }

    /// This function sets the host_status.boot_next to boot_current which is already part of boot order.
    /// The test verifies that boot_order is modified with boot_next as the first entry.
    #[test]
    fn test_boot_order_when_boot_entry_exists() {
        let host_status = HostStatus {
            boot_next: Some(String::from("0003")),
            ..Default::default()
        };

        let mut bootmgr_output = get_bootmgr_output();
        bootmgr_output.boot_order =
            vec!["0001".to_string(), "0003".to_string(), "0000".to_string()];

        let result = update_efi_boot_order(&host_status, &bootmgr_output);
        assert_eq!(result, (Some(String::from("0003,0001,0000")), true));
    }

    /// This function sets the host_status.boot_next not equal boot_current which is already part of boot order.
    /// The test verifies that boot_order is not modified.
    #[test]
    fn test_boot_order_when_boot_entry_exists_boot_next_not_equal_boot_order() {
        let host_status = HostStatus {
            boot_next: Some(String::from("0000")),
            ..Default::default()
        };

        let mut bootmgr_output = get_bootmgr_output();
        bootmgr_output.boot_order =
            vec!["0001".to_string(), "0003".to_string(), "0000".to_string()];

        let result = update_efi_boot_order(&host_status, &bootmgr_output);
        assert_eq!(result, (None, true));
    }

    /// This function sets the host_status.boot_next to boot_current which is already the first entry of boot order.
    /// The test verifies that the boot order is not modified.
    #[test]
    fn test_boot_order_when_boot_order_is_updated() {
        let host_status = HostStatus {
            boot_next: Some(String::from("0003")),
            ..Default::default()
        };

        let mut bootmgr_output = get_bootmgr_output();
        bootmgr_output.boot_order =
            vec!["0003".to_string(), "0001".to_string(), "0000".to_string()];

        let result = update_efi_boot_order(&host_status, &bootmgr_output);
        assert_eq!(result, (None, true));
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use crate::{
        modules::storage::partitioning::create_partitions, TRIDENT_TEMPORARY_DATASTORE_PATH,
    };

    use super::*;
    use constants::ESP_MOUNT_POINT_PATH;

    use tempfile::TempDir;
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
        // Create new boot entries for testing
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

        // Add to bootorder
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

    /// Function to test set_boot_order after setting boot_next of host status equal to the boot_current variable of efibootmgr.
    /// This tests the update success case i.e  the target was able to boot into the updated partition which was set as boot_next making the entry as boot_current.
    /// Boot order should be updated with boot_next as the first entry.
    #[functional_test(feature = "helpers")]
    fn test_set_boot_order_when_update_success() {
        delete_boot_next();
        set_some_boot_entries();

        // Create a temporary datastore
        let _ = std::fs::remove_file(TRIDENT_TEMPORARY_DATASTORE_PATH);
        let temp_dir = TempDir::new().unwrap();
        let datastore_path = temp_dir.path().join("db.sqlite");

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let boot_current = &bootmgr_output.boot_current;
        let initial_boot_order = bootmgr_output.boot_order;
        let mut datastore = DataStore::open_temporary().unwrap();

        // Set boot_next of host_status to boot_current which indicates that the target was able to boot into the updated partition.
        datastore
            .with_host_status(|s| {
                s.boot_next = Some(boot_current.to_string());
            })
            .unwrap();
        datastore.persist(&datastore_path).unwrap();
        datastore.close();

        set_boot_order(&datastore_path).unwrap();

        // We clear the boot_next variable in set_boot_order, check if it is cleared.
        let datastore1 = DataStore::open(&datastore_path).unwrap();
        let host_status: &HostStatus = datastore1.host_status();

        // Check if boot_next is cleared
        assert!(host_status.boot_next.is_none());

        // Get the modified boot_order
        let bootmgr_output1: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let final_boot_order = bootmgr_output1.boot_order;

        // Set expected boot_order i.e boot_next as the first entry and rest of the entries in the same order as initial_boot_order.
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

    /// Function to test set_boot_order after setting boot_next of host status not equal to the boot_current variable of efibootmgr.
    /// This tests the update fail case i.e the target was not able to boot into the updated partition which was set as boot_next.
    /// Boot order should not be updated.
    #[functional_test(feature = "helpers")]
    fn test_set_boot_order_when_rollback() {
        delete_boot_next();
        set_some_boot_entries();

        // Create a temporary datastore
        let _ = std::fs::remove_file(TRIDENT_TEMPORARY_DATASTORE_PATH);
        let temp_dir = TempDir::new().unwrap();
        let datastore_path = temp_dir.path().join("db.sqlite");

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let entry_number1 = bootmgr_output.get_boot_entry_number("TestBoot2").unwrap();
        let initial_boot_order = bootmgr_output.boot_order;
        let mut datastore = DataStore::open_temporary().unwrap();
        let new_boot_next = entry_number1;

        // Set boot_next of host_status to a newly added entry TESTBOOT1 which is not equal to boot_current.
        datastore
            .with_host_status(|s| {
                s.boot_next = Some(new_boot_next);
            })
            .unwrap();
        datastore.persist(&datastore_path).unwrap();
        datastore.close();

        set_boot_order(&datastore_path).unwrap();
        let datastore1 = DataStore::open(&datastore_path).unwrap();
        let host_status = datastore1.host_status();

        // Check if boot_next is cleared
        assert!(host_status.boot_next.is_none());

        // Get the modified boot_order
        let bootmgr_output1: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let final_boot_order = bootmgr_output1.boot_order;

        // Cleanup
        delete_created_boot_entries();

        assert_eq!(initial_boot_order, final_boot_order);
    }

    /// Function to test set_boot_order after setting boot_next None.
    /// This tests that there was no update from the last boot.
    /// Boot order should not be updated.
    #[functional_test(feature = "helpers")]
    fn test_set_boot_order_boot_next_none() {
        delete_boot_next();
        set_some_boot_entries();

        // Create a temporary datastore
        let _ = std::fs::remove_file(TRIDENT_TEMPORARY_DATASTORE_PATH);
        let temp_dir = TempDir::new().unwrap();
        let datastore_path = temp_dir.path().join("db.sqlite");

        let bootmgr_output: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let initial_boot_order = bootmgr_output.boot_order;
        let mut datastore = DataStore::open_temporary().unwrap();

        // Set boot_next to None
        datastore
            .with_host_status(|s| {
                s.boot_next = None;
            })
            .unwrap();
        datastore.persist(&datastore_path).unwrap();
        datastore.close();

        set_boot_order(&datastore_path).unwrap();
        let datastore1 = DataStore::open(&datastore_path).unwrap();
        let host_status = datastore1.host_status();

        // Check if boot_next of host_status is not modified
        assert!(host_status.boot_next.is_none());

        // Get the modified boot_order
        let bootmgr_output1: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let final_boot_order = bootmgr_output1.boot_order;

        // Cleanup
        delete_created_boot_entries();

        assert_eq!(initial_boot_order, final_boot_order)
    }
}
