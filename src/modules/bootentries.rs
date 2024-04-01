use std::fs;
use std::path::{Path, PathBuf, MAIN_SEPARATOR};

use anyhow::{bail, Context, Error};
use log::{debug, info};
use osutils::efibootmgr;
use osutils::efibootmgr::EfiBootManagerOutput;
use trident_api::config::PartitionType;
use trident_api::constants;
use trident_api::error::{ManagementError, ReportError, TridentError, TridentResultExt};
use trident_api::status::{AbVolumeSelection, HostStatus};

use crate::datastore::DataStore;
use crate::modules::{BOOT_ENTRY_A, BOOT_ENTRY_B};
use crate::{modules, TRIDENT_DATASTORE_PATH};

use super::BOOT64_EFI;

/// Calls the set_boot_next to set the boot next variable and  then updates the host status.
///
/// This function first sets the boot next variable by calling set_boot_next.
/// Then it retrieves the output of `efibootmgr` to get information about boot manager entries.
/// After that, it opens the datastore to update the host status with the retrieved boot next variable.
///
pub fn call_set_boot_next_and_update_hs(
    host_status: &HostStatus,
    new_root_path: &Path,
) -> Result<(), TridentError> {
    set_boot_next(host_status, new_root_path).structured(ManagementError::SetBootNext)?;

    // Get the output of efibootmgr
    let bootmgr_output: EfiBootManagerOutput = efibootmgr::list_and_parse_bootmgr_entries()
        .structured(ManagementError::ListAndParseBootEntries)?;

    // Open datastore
    let datastore_path = host_status
        .trident
        .datastore_path
        .as_deref()
        .unwrap_or(Path::new(TRIDENT_DATASTORE_PATH));
    debug!("Opening datastore at path: {}", datastore_path.display());
    let new_path = new_root_path.join(
        datastore_path
            .to_str()
            .unwrap()
            .trim_start_matches(MAIN_SEPARATOR),
    );
    let mut datastore =
        DataStore::open(&new_path).message("Failed to open datastore while setting boot_next")?;

    // Update host status with BootNext variable
    datastore.with_host_status(|s| {
        s.boot_next = if !bootmgr_output.boot_next.is_empty() {
            Some(bootmgr_output.boot_next)
        } else {
            None
        };
        debug!("Updating host status with BootNext: {:?}", s.boot_next);
    })?;

    //TODO-  https://dev.azure.com/mariner-org/ECF/_workitems/edit/6807 better way to close datastore
    datastore.close();

    Ok(())
}

/// Creates a boot entry for the updated AB partition and sets the `BootNext` variable to
/// boot from the updated partition on next boot.
fn set_boot_next(host_status: &HostStatus, new_root_path: &Path) -> Result<(), Error> {
    let (entry_label_new, bootloader_path_new) =
        get_label_and_path(host_status).context("Failed to get label and path")?;
    let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()?;

    if bootmgr_output.boot_entry_exists(entry_label_new)? {
        debug!("Boot entry already exists, deleting and adding new entry with label '{entry_label_new}'");
        let entry_number = bootmgr_output.get_boot_entry_number(entry_label_new)?;
        efibootmgr::delete_boot_entry(&entry_number)?;
        let mut boot_order = bootmgr_output.get_boot_order()?;
        if boot_order.contains(&entry_number) {
            boot_order.retain(|x| x != &entry_number);
            efibootmgr::modify_boot_order(boot_order.join(",").as_str())?;
        }
    }
    let disk_path = get_first_partition_of_type(host_status, PartitionType::Esp)
        .context("Failed to fetch esp disk path ")?;
    debug!("Disk path of first esp partition {:?}", disk_path);
    efibootmgr::create_boot_entry(
        entry_label_new,
        disk_path,
        bootloader_path_new,
        new_root_path,
    )
    .context("Failed to add boot entry")?;
    let bootmgr_output = efibootmgr::list_and_parse_bootmgr_entries()?;

    let added_entry_number = bootmgr_output
        .get_boot_entry_number(entry_label_new)
        .context("Failed to get boot entry number")?;
    debug!("Added boot entry: {added_entry_number}");

    // HACK: detect if we're inside qemu to avoid modifying boot order
    // TODO(#7139): remove this special case.
    let hdd_model = fs::read_to_string("/sys/block/sda/device/model").unwrap_or("".to_owned());
    let is_qemu = hdd_model.trim() == "QEMU HARDDISK";

    if !is_qemu {
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

/// Returns disk path based on partitionType
fn get_first_partition_of_type(
    host_status: &HostStatus,
    partition_ty: PartitionType,
) -> Result<PathBuf, Error> {
    return host_status
        .spec
        .storage
        .disks
        .iter()
        .find_map(|disk| {
            disk.partitions
                .iter()
                .find(|partition| partition.partition_type == partition_ty)
                .and_then(|_| host_status.storage.block_devices.get(&disk.id))
                .map(|block_device| block_device.path.clone())
        })
        .context("Failed to find disk path");
}

/// Retrieves the label and path for the EFI boot loader of the inactive A/B update volume.
///
/// This function takes a reference to a `HostStatus` object and returns a tuple containing
/// the label associated with the inactive A/B update volume and the path to its EFI boot loader.
///
fn get_label_and_path(host_status: &HostStatus) -> Result<(&str, PathBuf), Error> {
    match modules::get_ab_update_volume(host_status, false) {
        Some(AbVolumeSelection::VolumeA) => Ok((
            BOOT_ENTRY_A,
            Path::new(constants::ROOT_MOUNT_POINT_PATH)
                .join(constants::ESP_EFI_DIRECTORY)
                .join(BOOT_ENTRY_A)
                .join(BOOT64_EFI)
                .to_path_buf(),
        )),

        Some(AbVolumeSelection::VolumeB) => Ok((
            BOOT_ENTRY_B,
            Path::new(constants::ROOT_MOUNT_POINT_PATH)
                .join(constants::ESP_EFI_DIRECTORY)
                .join(BOOT_ENTRY_B)
                .join(BOOT64_EFI)
                .to_path_buf(),
        )),

        None => bail!("Unsupported AB volume selection"),
    }
}

/// Sets the EFI boot variables based on host status and current boot order.
/// This function opens the Trident DataStore, retrieves host status, lists EFI boot entries,
/// determines whether the boot order needs modification, and performs the necessary actions.
///
// TODO - https://dev.azure.com/mariner-org/ECF/_workitems/edit/6807 needs refactoring
pub fn set_boot_order(datastore_path: &Path) -> Result<(), TridentError> {
    let mut datastore = DataStore::open(datastore_path)
        .message("Failed to open datastore while setting boot order")?;
    let host_status = datastore.host_status();

    let bootmgr_output: EfiBootManagerOutput = efibootmgr::list_and_parse_bootmgr_entries()
        .structured(ManagementError::ListAndParseBootEntries)?;

    let (new_boot_order, clear_boot_next) = update_efi_boot_order(host_status, &bootmgr_output);

    if let Some(new_boot_order) = new_boot_order {
        debug!("Modifying boot order to: {}", new_boot_order);
        efibootmgr::modify_boot_order(&new_boot_order)
            .structured(ManagementError::ModifyBootOrder)?;
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
    use std::{path::PathBuf, str::FromStr};

    use maplit::btreemap;
    use osutils::efibootmgr::EfiBootEntry;
    use trident_api::{
        config::{
            self, AbUpdate, Disk, HostConfiguration, Partition, PartitionSize, PartitionType,
        },
        status::{BlockDeviceContents, BlockDeviceInfo, ReconcileState, Storage, UpdateKind},
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
            reconcile_state: ReconcileState::CleanInstall,
            ..Default::default()
        };

        // Test that clean-install will always use volume A for updates
        assert_eq!(
            get_label_and_path(&host_status).unwrap(),
            (
                BOOT_ENTRY_A,
                Path::new(constants::ROOT_MOUNT_POINT_PATH)
                    .join(constants::ESP_EFI_DIRECTORY)
                    .join(BOOT_ENTRY_A)
                    .join(BOOT64_EFI)
            )
        );

        // Test that UpdateInProgress(HostPatch, NormalUpdate, UpdateAndReboot)
        // will always use the active volume for updates
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate);
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_label_and_path(&host_status).unwrap(),
            (
                BOOT_ENTRY_B,
                Path::new(constants::ROOT_MOUNT_POINT_PATH)
                    .join(constants::ESP_EFI_DIRECTORY)
                    .join(BOOT_ENTRY_B)
                    .join(BOOT64_EFI)
            )
        );

        // Test that UpdateInProgress(Incompatible) will return None
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::Incompatible);
        let error_message = get_label_and_path(&host_status).unwrap_err().to_string();
        assert_eq!(error_message, "Unsupported AB volume selection");
    }

    #[test]
    fn test_get_first_partition_of_type() {
        let host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        partitions: vec![
                            Partition {
                                id: "efi".to_string(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("1M").unwrap(),
                            },
                            Partition {
                                id: "root-a".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "efi".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-a".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda2"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-b".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda3"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    }
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let result = get_first_partition_of_type(&host_status, PartitionType::Esp);
        assert_eq!(result.unwrap(), PathBuf::from("/dev/sda"));

        let result = get_first_partition_of_type(&host_status, PartitionType::Root);
        assert_eq!(result.unwrap(), PathBuf::from("/dev/sda"));
        let result = get_first_partition_of_type(&host_status, PartitionType::Var);
        assert!(result.is_err());
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
    use crate::TRIDENT_TEMPORARY_DATASTORE_PATH;

    use super::*;
    use maplit::btreemap;
    use tempfile::TempDir;
    use trident_api::{
        config::{self, AbUpdate, AbVolumePair, Disk, HostConfiguration, Partition, PartitionSize},
        constants::{ESP_RELATIVE_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
        status::{BlockDeviceInfo, ReconcileState, UpdateKind},
    };

    use osutils::{
        efibootmgr::{self, EfiBootManagerOutput},
        testutils::repart::OS_DISK_DEVICE_PATH,
    };
    use pytest_gen::functional_test;

    use std::{
        fs::{create_dir_all, File},
        iter::Iterator,
        str::FromStr,
    };
    use trident_api::status::{BlockDeviceContents, Storage};

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
        efibootmgr::create_boot_entry(
            "TestBoot1",
            OS_DISK_DEVICE_PATH,
            "/EFI/AZLA/bootx64.efi",
            tempdir.path(),
        )
        .unwrap();
        efibootmgr::create_boot_entry(
            "TestBoot2",
            OS_DISK_DEVICE_PATH,
            "/EFI/AZLA/bootx64.efi",
            tempdir.path(),
        )
        .unwrap();
        efibootmgr::create_boot_entry(
            "TestBoot3",
            OS_DISK_DEVICE_PATH,
            "/EFI/AZLA/bootx64.efi",
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

        set_boot_order(datastore_path.as_path()).unwrap();

        // We clear the boot_next variable in set_boot_order, check if it is cleared.
        let datastore1 = DataStore::open(datastore_path.as_path()).unwrap();
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

        set_boot_order(datastore_path.as_path()).unwrap();
        let datastore1 = DataStore::open(datastore_path.as_path()).unwrap();
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

        set_boot_order(datastore_path.as_path()).unwrap();
        let datastore1 = DataStore::open(datastore_path.as_path()).unwrap();
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

    fn test_helper_set_boot_entries(entry_label: &str, host_status: &HostStatus) {
        let bootmgr_output1: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        if bootmgr_output1.boot_entry_exists(entry_label).unwrap() {
            let boot_entry_num1 = bootmgr_output1.get_boot_entry_number(entry_label).unwrap();
            efibootmgr::delete_boot_entry(&boot_entry_num1).unwrap();
        }
        let tempdir = tempfile::tempdir().unwrap();
        set_boot_next(host_status, tempdir.path()).unwrap();
        let bootmgr_output2: EfiBootManagerOutput =
            efibootmgr::list_and_parse_bootmgr_entries().unwrap();
        let boot_entry_num2 = bootmgr_output2.get_boot_entry_number(entry_label).unwrap();
        assert_eq!(bootmgr_output2.boot_next, boot_entry_num2);

        // // TODO(#7139): Re-enable this one QEMU boot order issues are resolved.
        // assert_eq!(
        //     bootmgr_output2.get_boot_order().unwrap().last().unwrap(),
        //     &boot_entry_num2
        // );

        efibootmgr::delete_boot_entry(&boot_entry_num2).unwrap();
    }

    #[functional_test(feature = "helpers")]
    fn test_set_boot_entries() {
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        partitions: vec![
                            Partition {
                                id: "efi".to_string(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("1M").unwrap(),
                            },
                            Partition {
                                id: "root-a".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
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
            storage: Storage {
                block_devices: btreemap! {
                    "os".to_string() => BlockDeviceInfo {
                        path: PathBuf::from(OS_DISK_DEVICE_PATH),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "efi".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-a".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda2"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-b".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda3"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    }
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // For cleanInstall add A partition entry
        test_helper_set_boot_entries(BOOT_ENTRY_A, &host_status);

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);

        test_helper_set_boot_entries(BOOT_ENTRY_A, &host_status);

        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate);
        let dir_path = Path::new(&format!(
            "{}/{}",
            ROOT_MOUNT_POINT_PATH, ESP_RELATIVE_MOUNT_POINT_PATH
        ))
        .join(constants::ESP_EFI_DIRECTORY)
        .join(BOOT_ENTRY_B);
        create_dir_all(dir_path.clone()).unwrap();
        // Create B partition bootloader entry
        let file_path = dir_path.join(BOOT64_EFI).to_path_buf();
        File::create(file_path.clone()).unwrap();
        test_helper_set_boot_entries(BOOT_ENTRY_B, &host_status);
        // Delete the file
        std::fs::remove_file(file_path.clone()).unwrap();
    }
}
