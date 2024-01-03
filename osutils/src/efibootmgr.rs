use std::{ffi::OsStr, path::Path, process::Command};

use anyhow::{bail, Context, Error};

use crate::exe::RunAndCheck;

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
                let parts: Vec<&str> = line.trim().splitn(2, '*').collect();
                if parts.len() != 2 {
                    bail!("Error splitting efibootmgr output line '{line}'");
                } else {
                    let key = parts[0].trim().to_string();
                    let value = parts[1].trim().to_string();

                    let entry = EfiBootEntry {
                        id: key.replace("Boot", ""),
                        label: value,
                    };
                    boot_manager_output.boot_entries.push(entry);
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
    disk: impl AsRef<Path>,
    bootloader_path: impl AsRef<Path>,
) -> Result<(), Error> {
    Command::new("efibootmgr")
        .arg("--create-only")
        .arg("--disk")
        .arg(disk.as_ref())
        .arg("--label")
        .arg(entry_label)
        .arg("--loader")
        .arg(bootloader_path.as_ref())
        .run_and_check()
        .context("Failed to add boot entry through efibootmgr")
}

/// Sets `BootNext` variable using efibootmgr.
pub fn set_bootnext(entry_number: &str) -> Result<(), Error> {
    Command::new("efibootmgr")
        .arg("--bootnext")
        .arg(entry_number)
        .run_and_check()
        .context("Failed to add temporary next boot entry through efibootmgr")
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
#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    #[test]
    fn test_boot_mgr() {
        let sample_output = indoc! {"
        BootNext: 0000
        BootCurrent: 0001
        Timeout: 0 seconds
        BootOrder: 0001,0000,0002
        Boot0000* Windows Boot Manager
        Boot0001* ubuntu
        Boot0002* UEFI: Built-in EFI Shell
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

        // Sample EfiBootManagerOutput instance
        let expected_bootmgr_output = EfiBootManagerOutput {
            boot_next: "0000".to_string(),
            boot_current: "0001".to_string(),
            boot_order: vec!["0001".to_string(), "0000".to_string(), "0002".to_string()],
            boot_entries: vec![entry1, entry2, entry3],
        };
        assert_eq!(bootmgr_output, expected_bootmgr_output);

        assert!(bootmgr_output.check_current_boot_entry("0001").unwrap());

        assert!(!bootmgr_output.check_current_boot_entry("0002").unwrap());
        let expected_bootorder = ["0001", "0000", "0002"];
        assert_eq!(
            bootmgr_output.get_boot_order().unwrap(),
            &expected_bootorder
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
}

#[cfg(all(test, feature = "functional-tests"))]
mod functional_tests {

    use super::*;

    #[test]
    fn test_efi_bootmgr() {
        let entry_label = "TestBoot1";
        let bootloader_path = Path::new(r"/EFI/BOOT/bootx64.efi");

        let output = list_bootmgr_entries().unwrap();
        let bootmgr_output_initial =
            EfiBootManagerOutput::parse_efibootmgr_output(&output).unwrap();

        let bootorder_initial = bootmgr_output_initial.get_boot_order().unwrap();

        let disk_path = "/dev/sda1";

        create_boot_entry(entry_label, disk_path, bootloader_path).unwrap();
        let output1 = list_bootmgr_entries().unwrap();
        let bootmgr_output1 = EfiBootManagerOutput::parse_efibootmgr_output(&output1).unwrap();

        let bootentry_number = bootmgr_output1.get_boot_entry_number(entry_label).unwrap();

        let bootentry_exists = bootmgr_output1.boot_entry_exists(entry_label).unwrap();
        assert!(bootentry_exists);

        set_bootnext(&bootentry_number).unwrap();
        let output2 = list_bootmgr_entries().unwrap();
        let bootmgr_output2 = EfiBootManagerOutput::parse_efibootmgr_output(&output2).unwrap();

        assert!(bootmgr_output2.boot_next == bootentry_number);
        let new_bootorder_str = bootentry_number + "," + &bootorder_initial.join(",");
        modify_boot_order(&new_bootorder_str).unwrap();
        let output3 = list_bootmgr_entries().unwrap();
        let bootmgr_output3 = EfiBootManagerOutput::parse_efibootmgr_output(&output3).unwrap();

        assert!(bootmgr_output3.boot_order.join(",") == new_bootorder_str);
    }
}
