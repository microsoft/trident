use std::{fs, io::Write, path::Path};

use log::debug;
use tempfile::NamedTempFile;

use trident_api::error::{ReportError, ServicingError, TridentError, TridentResultExt};

use crate::dependencies::{Dependency, DependencyResultExt};

const BOOTLOADER_INTERFACE_GUID: &str = "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f";

const LOADER_ENTRY_ONESHOT: &str = "LoaderEntryOneShot";
const LOADER_ENTRY_DEFAULT: &str = "LoaderEntryDefault";
const LOADER_ENTRY_SELECTED: &str = "LoaderEntrySelected";

fn encode_utf16le(data: &str) -> Vec<u8> {
    data.encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .chain([0; 2])
        .collect()
}

fn decode_utf16le(mut data: &[u8]) -> String {
    if data.len() <= 2 {
        return String::new();
    }

    // Remove null terminator
    if data[data.len() - 2..] == [0, 0] {
        data = &data[..data.len() - 2];
    }

    let utf16_data: Vec<u16> = data
        .chunks(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    String::from_utf16_lossy(&utf16_data)
}

/// Set an EFI variable using the efivar command-line tool.
/// `name` should include the GUID, e.g. "BootNext-8be4df61-93ca-11d2-aa0d-00e098032b8c"
/// `data` should be a hex string, e.g. "0100" for BootNext=0001 (little-endian)
fn set_efi_variable(name: &str, data_utf16: &[u8]) -> Result<(), TridentError> {
    debug!(
        "Setting EFI variable '{name}' to '{}'",
        decode_utf16le(data_utf16)
    );

    // Write the UTF-16LE data to a temporary file
    let mut tmpfile = NamedTempFile::new().structured(ServicingError::SetEfiVariable {
        name: name.to_string(),
    })?;
    tmpfile
        .write_all(data_utf16)
        .structured(ServicingError::SetEfiVariable {
            name: name.to_string(),
        })?;

    Dependency::Efivar
        .cmd()
        .arg("--verbose")
        .arg("--name")
        .arg(name)
        .arg("--write")
        .arg("--datafile")
        .arg(tmpfile.path())
        .run_and_check()
        .message(format!("efivar failed to set variable '{name}'"))
}

/// Set the LoaderEntryOneShot EFI variable for systemd-boot oneshot boot.
pub fn set_oneshot(entry: &str) -> Result<(), TridentError> {
    debug!("Setting oneshot boot entry to: '{entry}'");
    set_efi_variable(
        &format!("{BOOTLOADER_INTERFACE_GUID}-{LOADER_ENTRY_ONESHOT}"),
        &encode_utf16le(entry),
    )
}

/// Set the LoaderEntryDefault EFI variable for systemd-boot default boot.
pub fn set_default(entry: &str) -> Result<(), TridentError> {
    debug!("Setting default boot entry to: '{entry}'");
    set_efi_variable(
        &format!("{BOOTLOADER_INTERFACE_GUID}-{LOADER_ENTRY_DEFAULT}"),
        &encode_utf16le(entry),
    )
}

/// Returns the value of a given EFI variable given the variable name and GUID.
fn read_efi_variable(guid: &str, variable: &str) -> Result<Vec<u8>, TridentError> {
    let efi_var_path = Path::new("/sys/firmware/efi/efivars/").join(format!("{variable}-{guid}"));

    // Read the LoaderEntrySelected EFI variable from efivars
    let data = fs::read(efi_var_path).structured(ServicingError::ReadEfiVariable {
        name: variable.to_string(),
    })?;

    // The first 4 bytes are attributes, skip them
    if data.len() <= 4 {
        return Err(TridentError::new(ServicingError::ReadEfiVariable {
            name: variable.to_string(),
        }))
        .message("EFI variable file is too short");
    }
    Ok(data[4..].to_vec())
}

/// Returns whether the LoaderEntrySelected EFI variable is set.
pub fn current_var_set() -> bool {
    read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_SELECTED).is_ok()
}

/// Returns the value of the LoaderEntrySelected EFI variable. This is the current boot entry.
pub fn read_current_var() -> Result<String, TridentError> {
    let data = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_SELECTED)?;
    Ok(decode_utf16le(&data))
}

/// Set the LoaderEntryDefault EFI variable to the current boot entry
pub fn set_default_to_current() -> Result<(), TridentError> {
    let current = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_SELECTED)?;
    debug!(
        "Setting default boot entry to current: '{}'",
        decode_utf16le(&current)
    );
    set_efi_variable(
        &format!("{BOOTLOADER_INTERFACE_GUID}-{LOADER_ENTRY_DEFAULT}"),
        &current,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_utf16le() {
        let input = "Test";
        let expected = vec![84, 0, 101, 0, 115, 0, 116, 0, 0, 0];
        assert_eq!(encode_utf16le(input), expected);
    }

    #[test]
    fn test_decode_utf16le() {
        let input = vec![84, 0, 101, 0, 115, 0, 116, 0, 0, 0];
        assert_eq!(decode_utf16le(&input), "Test");
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use pytest_gen::functional_test;

    use super::*;

    #[functional_test(feature = "helpers")]
    fn test_set_oneshot() {
        let entry = "TestEntry";
        set_oneshot(entry).unwrap();
        let data = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_ONESHOT).unwrap();
        assert_eq!(decode_utf16le(&data), entry);

        set_oneshot("").unwrap();
    }

    #[functional_test(feature = "helpers")]
    fn test_set_default() {
        let entry = "TestDefaultEntry";
        set_default(entry).unwrap();
        let data = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_DEFAULT).unwrap();
        assert_eq!(decode_utf16le(&data), entry);

        set_default("").unwrap();
    }

    #[functional_test(feature = "helpers")]
    fn test_set_default_to_current() {
        assert!(!current_var_set());
        set_efi_variable(
            &format!("{BOOTLOADER_INTERFACE_GUID}-{LOADER_ENTRY_SELECTED}"),
            &encode_utf16le("CurrentEntry"),
        )
        .unwrap();

        // Check that the current entry is set
        assert!(current_var_set());
        assert_eq!(read_current_var().unwrap(), "CurrentEntry");

        // Now set the default to the current entry
        set_default_to_current().unwrap();
        let data = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_DEFAULT).unwrap();
        assert_eq!(decode_utf16le(&data), "CurrentEntry");

        set_default("").unwrap();
    }
}
