use std::{fs, io::Write, path::Path};

use log::debug;
use tempfile::NamedTempFile;

use trident_api::error::{ReportError, ServicingError, TridentError, TridentResultExt};

use crate::dependencies::{Dependency, DependencyResultExt};

pub const BOOTLOADER_INTERFACE_GUID: &str = "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f";
const EFI_GLOBAL_VARIABLE_GUID: &str = "8be4df61-93ca-11d2-aa0d-00e098032b8c";

const SECURE_BOOT: &str = "SecureBoot";

const LOADER_ENTRY_ONESHOT: &str = "LoaderEntryOneShot";
const LOADER_ENTRY_DEFAULT: &str = "LoaderEntryDefault";
pub const LOADER_ENTRY_SELECTED: &str = "LoaderEntrySelected";
const LOADER_ENTRIES_DEFAULT: &str = "LoaderEntries";

/// Converts a UTF‑8 Rust string to a UTF-16LE byte array.
pub fn encode_utf16le(data: &str) -> Vec<u8> {
    data.encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .chain([0; 2])
        .collect()
}

/// Converts a UTF-16LE byte array to a UTF‑8 Rust string.
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

/// Converts a UTF-16LE byte array to a UTF‑8 Rust string.
fn decode_utf16le_to_strings(data: &[u8]) -> Vec<String> {
    let mut result = Vec::new();
    if data.len() <= 2 {
        return result;
    }

    let mut start = 0;
    let u16_null = u16::from_le_bytes([0, 0]);

    // Iterate through the byte slice
    for (i, &byte) in data.iter().enumerate() {
        // Combine 2 u8 bytes into a u16
        if i % 2 == 0 {
            // Only judge on u16 boundaries
            continue;
        }
        // We are at the second byte of a u16
        let u16_byte = u16::from_le_bytes([data[i - 1], byte]);
        if u16_byte == u16_null {
            // Skip the null-terminating character
            let end = i - 1;
            let current_bytes = &data[start..end];

            // If we encounter an empty string (two consecutive nulls, or a null at the very beginning/end)
            // this usually signifies the end of the list itself.
            if current_bytes.is_empty() {
                // Check if this is the final, extra null terminator for the list
                if i == data.len() - 1 && data.ends_with(b"\0\0") {
                    break; // End of list found
                }
            } else {
                let utf16_data: Vec<u16> = current_bytes
                    .chunks(2)
                    .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
                    .collect();
                let decoded_string = String::from_utf16_lossy(&utf16_data);
                result.push(decoded_string);
            }
            start = i + 1; // Move the start position past the null terminator
        }
    }

    result
}

/// Sets an EFI variable using the efivar command-line tool.
/// - `name` should include the GUID, e.g. "BootNext-8be4df61-93ca-11d2-aa0d-00e098032b8c"
/// - `data` should be a hex string, e.g. "0100" for BootNext=0001 (little-endian)
pub fn set_efi_variable(name: &str, data_utf16: &[u8]) -> Result<(), TridentError> {
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

/// Sets the LoaderEntryOneShot EFI variable for systemd-boot oneshot boot.
pub fn set_oneshot(entry: &str) -> Result<(), TridentError> {
    debug!("Setting oneshot boot entry to: '{entry}'");
    set_efi_variable(
        &format!("{BOOTLOADER_INTERFACE_GUID}-{LOADER_ENTRY_ONESHOT}"),
        &encode_utf16le(entry),
    )
}

/// Sets the LoaderEntryDefault EFI variable for systemd-boot default boot.
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

    // Read the EFI variable from efivars
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

/// Returns whether `SecureBoot` is currently enabled. If the variable is not currently set,
/// `SecureBoot` is considered disabled.
pub fn secure_boot_is_enabled() -> bool {
    let Ok(data) = read_efi_variable(EFI_GLOBAL_VARIABLE_GUID, SECURE_BOOT) else {
        return false;
    };

    // SecureBoot is a single byte: 0x00 = disabled, 0x01 = enabled
    !data.is_empty() && data[0] == 1
}

/// Returns whether the LoaderEntrySelected EFI variable is set and indicates a UKI boot.
pub fn current_var_is_uki() -> bool {
    let Ok(current) = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_SELECTED) else {
        return false;
    };

    decode_utf16le(&current).ends_with(".efi")
}

/// Returns the value of the LoaderEntrySelected EFI variable. This is the current boot entry.
pub fn read_current_var() -> Result<String, TridentError> {
    let data = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_SELECTED)?;
    Ok(decode_utf16le(&data))
}

/// Sets the LoaderEntryDefault EFI variable to the current boot entry
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

/// Sets the LoaderEntryDefault EFI variable to the previous boot entry
pub fn set_default_to_previous() -> Result<(), TridentError> {
    let current = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_SELECTED)?;
    let current_decoded = decode_utf16le(&current);
    let boot_entries = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRIES_DEFAULT)?;
    let boot_entries_decoded = decode_utf16le_to_strings(&boot_entries);
    if boot_entries_decoded.len() < 2 {
        return Err(TridentError::new(ServicingError::SetEfiVariable {
            name: LOADER_ENTRIES_DEFAULT.to_string(),
        }))
        .message("Not enough boot entries to determine previous entry");
    }
    if boot_entries_decoded[0] != current_decoded {
        return Err(TridentError::new(ServicingError::SetEfiVariable {
            name: LOADER_ENTRIES_DEFAULT.to_string(),
        }))
        .message("Current boot entry does not match first entry in boot entries list");
    }
    let previous = &boot_entries_decoded[1];

    set_efi_variable(
        &format!("{BOOTLOADER_INTERFACE_GUID}-{LOADER_ENTRY_DEFAULT}"),
        &encode_utf16le(previous),
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
    use super::*;

    use pytest_gen::functional_test;

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
        // Generate a random current entry
        let current_entry = format!("CurrentEntry-{}.efi", rand::random::<u32>());

        set_efi_variable(
            &format!("{BOOTLOADER_INTERFACE_GUID}-{LOADER_ENTRY_SELECTED}"),
            &encode_utf16le(&current_entry),
        )
        .unwrap();

        // Check that the current entry is set
        assert!(current_var_is_uki());
        assert_eq!(read_current_var().unwrap(), current_entry);

        // Now set the default to the current entry
        set_default_to_current().unwrap();
        let data = read_efi_variable(BOOTLOADER_INTERFACE_GUID, LOADER_ENTRY_DEFAULT).unwrap();
        assert_eq!(decode_utf16le(&data), current_entry);

        set_default("").unwrap();

        // Unset the current entry
        set_efi_variable(
            &format!("{BOOTLOADER_INTERFACE_GUID}-{LOADER_ENTRY_SELECTED}"),
            &encode_utf16le(""),
        )
        .unwrap();
    }

    #[functional_test(feature = "helpers")]
    fn test_secure_boot_is_enabled() {
        let secure_boot_enabled = secure_boot_is_enabled();

        // The function should return true b/c SecureBoot is now enabled on FT VM
        assert!(secure_boot_enabled);
    }
}
