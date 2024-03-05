use std::{
    ffi::OsStr,
    path::{Path, PathBuf, MAIN_SEPARATOR},
    process::Command,
};

use crate::exe::RunAndCheck;
use anyhow::{bail, Context, Error};
use regex::Regex;

use trident_api::constants::{
    ESP_MOUNT_POINT_PATH, ESP_RELATIVE_MOUNT_POINT_PATH, UPDATE_ROOT_PATH,
};

/// Represents an entry in the EFI Boot Manager.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct EfiBootEntry {
    /// The identifier for the boot entry.
    pub id: String,

    /// The label or description of the boot entry.
    pub label: String,
}

// Represents the output of the EFI Boot Manager.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct EfiBootManagerOutput {
    /// The boot entry that will be booted next.
    pub boot_next: String,

    /// The currently active boot entry.
    pub boot_current: String,

    /// The order in which boot entries are attempted.
    pub boot_order: Vec<String>,

    /// List of EFI boot entries with their associated information.
    pub boot_entries: Vec<EfiBootEntry>,
}

impl EfiBootManagerOutput {
    pub fn parse_efibootmgr_output(output: &str) -> Result<Self, Error> {
        let mut boot_manager_output = EfiBootManagerOutput::default();

        for line in output.lines() {
            if line.starts_with("BootCurrent:")
                || line.starts_with("BootNext:")
                || line.starts_with("BootOrder:")
            {
                let parts: Vec<&str> = line.trim().splitn(2, ':').collect();
                if parts.len() != 2 {
                    bail!("Error splitting efibootmgr output line '{line}'");
                } else {
                    let key = parts[0].trim();
                    let value = parts[1].trim();

                    match key {
                        "BootNext" => boot_manager_output.boot_next = value.to_string(),
                        "BootCurrent" => boot_manager_output.boot_current = value.to_string(),
                        "BootOrder" => {
                            boot_manager_output.boot_order =
                                value.split(',').map(|s| s.trim().to_string()).collect();
                        }

                        _ => {} // Ignore other keys
                    }
                }
            } else if line.starts_with("Boot") {
                let re = Regex::new(r"^Boot([0-9a-fA-F]{4})(\*?) (.+)$").unwrap();
                let captures = re.captures(line.trim());
                if let Some(captures) = captures {
                    let key = captures
                        .get(1)
                        .context("failed to parse boot entry number")?
                        .as_str()
                        .to_string();
                    let value = captures
                        .get(3)
                        .context("failed to parse boot entry name")?
                        .as_str()
                        .trim()
                        .to_string();
                    let entry = EfiBootEntry {
                        id: key,
                        label: value,
                    };
                    boot_manager_output.boot_entries.push(entry);
                } else {
                    bail!("Error splitting efibootmgr output line '{line}'");
                }
            }
        }
        Ok(boot_manager_output)
    }

    /// Checks if a boot entry with the entry label already exists.
    pub fn boot_entry_exists(&self, entry_label: &str) -> Result<bool, Error> {
        Ok(self
            .boot_entries
            .iter()
            .any(|entry| entry.label == entry_label))
    }

    /// Gets entry number of the boot entry with given entry label.
    pub fn get_boot_entry_number(&self, entry_label: &str) -> Result<String, Error> {
        let boot_number: String = self
            .boot_entries
            .iter()
            .find(|&entry| entry.label == entry_label)
            .context(format!("Cant find boot entry for '{entry_label}'"))?
            .id
            .to_string();

        Ok(boot_number)
    }

    /// Checks the `BootCurrent` entry of efibootmgr.
    pub fn check_current_boot_entry(&self, boot_number: &str) -> Result<bool, Error> {
        Ok(self.boot_current == boot_number)
    }

    /// Gets the `BootOrder` variable of efibootmgr.
    pub fn get_boot_order(&self) -> Result<Vec<String>, Error> {
        Ok(self.boot_order.clone())
    }
}

///lists boot entries using efibootmgr
pub fn list_bootmgr_entries() -> Result<String, Error> {
    Command::new("efibootmgr")
        .output_and_check()
        .context("Efibootmgr exited with an error")
}

/// Adds a boot entry using efibootmgr.
pub fn create_boot_entry(
    entry_label: impl AsRef<OsStr>,
    disk_path: impl AsRef<Path>,
    bootloader_path: impl AsRef<Path>,
) -> Result<(), Error> {
    // Check if disk path is valid
    if !disk_path.as_ref().exists() {
        bail!(
            "Disk path '{}' does not exist",
            disk_path.as_ref().display()
        );
    }
    // Check if the path exists in root mount point
    let mut valid = is_valid_bootloader_path(
        ESP_MOUNT_POINT_PATH,
        bootloader_path.as_ref().to_str().context(format!(
            "Failed to convert bootloader path '{}' to str",
            entry_label.as_ref().to_string_lossy()
        ))?,
    );

    if !valid {
        // Check if the path exists in new root mount point as we should support creating boot entry in new root mount point before transition.
        valid = is_valid_bootloader_path(
            &format!("{}/{}", UPDATE_ROOT_PATH, ESP_RELATIVE_MOUNT_POINT_PATH),
            bootloader_path.as_ref().to_str().context(format!(
                "Failed to convert bootloader path {} to str",
                bootloader_path.as_ref().to_string_lossy(),
            ))?,
        );
    }

    // Check if the bootloader path exists
    if !valid {
        bail!(
            "Bootloader path '{}' does not exist",
            bootloader_path.as_ref().display()
        );
    }

    let bootmgr_output =
        list_and_parse_bootmgr_entries().context("Failed to list and parse efibootmgr output")?;
    // Create only if there is no entry with the same label
    if bootmgr_output
        .boot_entry_exists(entry_label.as_ref().to_str().context(format!(
            "Failed to convert entry label {} to str",
            entry_label.as_ref().to_string_lossy()
        ))?)
        .context("Failed to check if boot entry exists")?
    {
        bail!(
            "Bootentry with the same label '{}' already exists in efibootmgr",
            entry_label.as_ref().to_string_lossy()
        );
    }
    Command::new("efibootmgr")
        .arg("--create-only")
        .arg("--disk")
        .arg(disk_path.as_ref())
        .arg("--label")
        .arg(entry_label.as_ref())
        .arg("--loader")
        .arg(bootloader_path.as_ref())
        .run_and_check()
        .context(format!(
            "Failed to add boot entry {} at disk path {} through efibootmgr ",
            entry_label.as_ref().to_string_lossy(),
            disk_path.as_ref().display()
        ))?;
    Ok(())
}

/// Sets `BootNext` variable using efibootmgr.
pub fn set_boot_next(entry_number: &str) -> Result<(), Error> {
    Command::new("efibootmgr")
        .arg("--bootnext")
        .arg(entry_number)
        .run_and_check()
        .context("Failed to add temporary next boot entry through efibootmgr")
}

/// Delete `BootNext` variable using efibootmgr.
pub fn delete_boot_next() -> Result<(), Error> {
    Command::new("efibootmgr")
        .arg("--delete-bootnext")
        .run_and_check()
        .context("Failed to delete bootnext through efibootmgr")
}

/// Modifies the `BootOrder` variable of efibootmgr.
pub fn modify_boot_order(new_boot_order: &str) -> Result<(), Error> {
    Command::new("efibootmgr")
        .arg("--bootorder")
        .arg(new_boot_order)
        .run_and_check()
        .context("Failed to set boot order through efibootmgr")
}

/// Delete the bootentry using efibootmgr.
pub fn delete_boot_entry(entry_number: &str) -> Result<(), Error> {
    Command::new("efibootmgr")
        .arg("--bootnum")
        .arg(entry_number)
        .arg("--delete-bootnum")
        .run_and_check()
        .context("Failed to delete boot entry through efibootmgr")
}

fn is_valid_bootloader_path(esp_path: &str, bootloader_path: &str) -> bool {
    let full_path =
        PathBuf::from(esp_path).join(bootloader_path.trim_start_matches(MAIN_SEPARATOR));

    full_path.exists() && full_path.is_file()
}

pub fn list_and_parse_bootmgr_entries() -> Result<EfiBootManagerOutput, Error> {
    let output = list_bootmgr_entries().context("Failed to list boot manager entries")?;
    EfiBootManagerOutput::parse_efibootmgr_output(&output)
        .context("Failed to parse efibootmgr output")
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::tempdir;
    #[test]
    fn test_boot_mgr() {
        let sample_output = indoc! {"
        BootNext: 0000
        BootCurrent: 0001
        Timeout: 0 seconds
        BootOrder: 0001,0000,0002,000A
        Boot0000  Windows Boot Manager
        Boot0001* ubuntu
        Boot0002* UEFI: Built-in EFI Shell
        Boot000A* Mariner
    "};

        let bootmgr_output: EfiBootManagerOutput =
            EfiBootManagerOutput::parse_efibootmgr_output(sample_output).unwrap();

        let entry1 = EfiBootEntry {
            id: "0000".to_string(),
            label: "Windows Boot Manager".to_string(),
        };

        let entry2 = EfiBootEntry {
            id: "0001".to_string(),
            label: "ubuntu".to_string(),
        };

        let entry3 = EfiBootEntry {
            id: "0002".to_string(),
            label: "UEFI: Built-in EFI Shell".to_string(),
        };
        let entry4 = EfiBootEntry {
            id: "000A".to_string(),
            label: "Mariner".to_string(),
        };

        // Sample EfiBootManagerOutput instance
        let expected_bootmgr_output = EfiBootManagerOutput {
            boot_next: "0000".to_string(),
            boot_current: "0001".to_string(),
            boot_order: vec![
                "0001".to_string(),
                "0000".to_string(),
                "0002".to_string(),
                "000A".to_string(),
            ],
            boot_entries: vec![entry1, entry2, entry3, entry4],
        };
        assert_eq!(bootmgr_output, expected_bootmgr_output);

        assert!(bootmgr_output.check_current_boot_entry("0001").unwrap());

        assert!(!bootmgr_output.check_current_boot_entry("0002").unwrap());
        let expected_boot_order = ["0001", "0000", "0002", "000A"];
        assert_eq!(
            bootmgr_output.get_boot_order().unwrap(),
            &expected_boot_order
        );
        assert_eq!(
            bootmgr_output
                .get_boot_entry_number("Windows Boot Manager")
                .unwrap(),
            "0000"
        );
        assert!(bootmgr_output
            .boot_entry_exists("Windows Boot Manager")
            .unwrap());
        assert_eq!(bootmgr_output.boot_next, "0000");
    }

    #[test]
    fn test_valid_bootloader_path() {
        let temp_dir = tempdir().unwrap();
        let esp_path = temp_dir.path();
        let bootloader_file_name = "bootx64.efi";
        let bootloader_path = esp_path.join(bootloader_file_name);

        // Create a dummy bootloader file
        let mut file = File::create(bootloader_path).unwrap();
        writeln!(file, "EFI").unwrap();

        assert!(is_valid_bootloader_path(
            esp_path.to_str().unwrap(),
            bootloader_file_name
        ));
    }

    #[test]
    fn test_invalid_bootloader_path_file_does_not_exist() {
        let temp_dir = tempdir().unwrap();
        let esp_path = temp_dir.path();
        let bootloader_file_name = "nonexistent.efi";

        assert!(!is_valid_bootloader_path(
            esp_path.to_str().unwrap(),
            bootloader_file_name
        ));
    }

    #[test]
    fn test_invalid_bootloader_path_is_directory() {
        let temp_dir = tempdir().unwrap();
        let esp_path = temp_dir.path();
        let bootloader_dir_name = "EFI";
        let bootloader_path = esp_path.join(bootloader_dir_name);

        fs::create_dir(bootloader_path).unwrap();

        assert!(!is_valid_bootloader_path(
            esp_path.to_str().unwrap(),
            bootloader_dir_name
        ));
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_efi_bootmgr_pass() {
        // Define the boot entry label, disk path and bootloader path
        let entry_label = "TestBoot1";
        let disk_path = "/dev/sda1";
        let bootloader_path = Path::new(r"/EFI/AZLA/bootx64.efi");

        // Get the initial boot order
        let bootmgr_output_initial = list_and_parse_bootmgr_entries().unwrap();

        let boot_order_initial = bootmgr_output_initial.get_boot_order().unwrap();

        // Create a boot entry
        create_boot_entry(entry_label, disk_path, bootloader_path).unwrap();
        let bootmgr_output1 = list_and_parse_bootmgr_entries().unwrap();

        // Get the boot entry number of the boot entry that is created above
        let bootentry_number = bootmgr_output1.get_boot_entry_number(entry_label).unwrap();

        // Verify if the boot entry exists
        let bootentry_exists = bootmgr_output1.boot_entry_exists(entry_label).unwrap();
        assert!(bootentry_exists);

        // Set bootnext to the new boot entry that is created above
        set_boot_next(&bootentry_number).unwrap();
        let bootmgr_output2 = list_and_parse_bootmgr_entries().unwrap();

        assert!(bootmgr_output2.boot_next == bootentry_number);
        let new_boot_order_str = bootentry_number + "," + &boot_order_initial.join(",");

        // Modify boot order to set the new boot entry as the first boot entry
        modify_boot_order(&new_boot_order_str).unwrap();

        let bootmgr_output3 = list_and_parse_bootmgr_entries().unwrap();

        assert!(bootmgr_output3.boot_order.join(",") == new_boot_order_str);

        // Delete the boot entry thats created above
        delete_boot_entry(&bootmgr_output3.boot_next).unwrap();

        let bootmgr_output4 = list_and_parse_bootmgr_entries().unwrap();
        let bootentry_exists = bootmgr_output4.boot_entry_exists(entry_label).unwrap();
        assert!(!bootentry_exists);

        // Delete bootnext
        delete_boot_next().unwrap();
        let bootmgr_output5 = list_and_parse_bootmgr_entries().unwrap();
        assert!(bootmgr_output5.boot_next.is_empty());
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_efi_bootmgr_delete_boot_next_fail() {
        // Define the boot entry label, disk path and bootloader path
        let entry_label = "TestBoot1";
        let disk_path = "/dev/sda1";
        let bootloader_path = Path::new(r"/EFI/AZLA/bootx64.efi");

        // Create a boot entry
        create_boot_entry(entry_label, disk_path, bootloader_path).unwrap();
        let bootmgr_output1 = list_and_parse_bootmgr_entries().unwrap();

        // Get the boot entry number of the boot entry that is created above
        let bootentry_number = bootmgr_output1.get_boot_entry_number(entry_label).unwrap();

        // Set bootnext to the new boot entry that is created above
        set_boot_next(&bootentry_number).unwrap();
        let bootmgr_output2 = list_and_parse_bootmgr_entries().unwrap();

        assert!(bootmgr_output2.boot_next == bootentry_number);

        // Delete the boot entry thats created above
        delete_boot_entry(&bootmgr_output2.boot_next).unwrap();

        // Delete bootnext
        delete_boot_next().unwrap();
        let bootmgr_output3 = list_and_parse_bootmgr_entries().unwrap();
        assert!(bootmgr_output3.boot_next.is_empty());

        // Delete bootnext again should fail
        assert_eq!(
            delete_boot_next().unwrap_err().root_cause().to_string(),
            "Process output:\nstderr:\nCould not delete BootNext: No such file or directory\n\n",
            "Unexpected error message for deleting bootnext"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_create_boot_entry_fail() {
        // Create a boot entry TestBoot1
        // Define the boot entry label, disk path and bootloader path
        let entry_label = "TestBoot1";
        let disk_path = "/dev/sda1";
        let bootloader_path = Path::new(r"/EFI/AZLA/bootx64.efi");

        // Create a boot entry
        create_boot_entry(entry_label, disk_path, bootloader_path).unwrap();

        // Creating a boot entry with the same label should fail
        let result = create_boot_entry(entry_label, disk_path, bootloader_path);
        assert_eq!(
            result.unwrap_err().root_cause().to_string(),
            format!(
                "Bootentry with the same label '{}' already exists in efibootmgr",
                entry_label
            ),
            "Failed to return error when creating boot entry with the same label"
        );

        // Try creating an entry with invalid bootloader path
        let bootloader_path_invalid: &Path = Path::new(r"/doesnotexist/bootx64.efi");
        // Creating a boot entry with invalid bootloader path should fail
        let result = create_boot_entry(entry_label, disk_path, bootloader_path_invalid);
        assert_eq!(
            result.unwrap_err().root_cause().to_string(),
            format!(
                "Bootloader path '{}' does not exist",
                bootloader_path_invalid.display()
            ),
            "Failed to return error when creating boot entry with invalid bootloader path"
        );

        // Try creating an entry with invalid disk path
        let disk_path_invalid = "/dev/abc";
        // Creating a boot entry with invalid disk path should fail
        let result = create_boot_entry(entry_label, disk_path_invalid, bootloader_path);
        assert_eq!(
            result.unwrap_err().root_cause().to_string(),
            format!("Disk path '{}' does not exist", disk_path_invalid),
            "Failed to return error when creating boot entry with invalid disk path"
        );

        // Cleanup
        let bootmgr_output1 = list_and_parse_bootmgr_entries().unwrap();
        let bootentry_number = bootmgr_output1.get_boot_entry_number(entry_label).unwrap();
        // Delete the boot entry thats created above
        delete_boot_entry(&bootentry_number).unwrap();
    }
}
