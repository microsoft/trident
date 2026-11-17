//! Validates MSHV boot components independently of the surrounding bootloader chain.

use std::{fs, io::ErrorKind, path::Path};

use anyhow::{ensure, Context, Error};

use trident_api::constants::{EFI_DEFAULT_BIN_RELATIVE_PATH, ESP_EFI_DIRECTORY};

use super::files;

const MSHV_BOOT_FILES: &[&str] = &["HvLoader.efi", "lxhvloader.dll"];
const MSHV_BOOT_COMPONENTS_DIR: &str = "Windows";

/// Detects MSHV boot components and validates their required loader files.
pub(crate) fn has_slot_mshv_boot_components(boot_dir: &Path) -> Result<bool, Error> {
    for marker in MSHV_BOOT_FILES
        .iter()
        .copied()
        .chain([MSHV_BOOT_COMPONENTS_DIR])
    {
        match fs::symlink_metadata(boot_dir.join(marker)) {
            Ok(_) => {
                validate_boot_components(boot_dir)?;
                return Ok(true);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Failed to inspect MSHV boot components"),
        }
    }
    Ok(false)
}

/// Requires the MSHV loaders and validates the optional Windows tree.
/// Supporting bootloaders, including ETL's second stage, are validated separately.
pub(crate) fn validate_boot_components(boot_dir: &Path) -> Result<(), Error> {
    for name in MSHV_BOOT_FILES {
        let path = boot_dir.join(name);
        ensure!(
            fs::symlink_metadata(&path)
                .with_context(|| format!("Missing MSHV boot component: {name}"))?
                .is_file(),
            "Invalid MSHV boot component: {name}"
        );
    }
    let windows = boot_dir.join(MSHV_BOOT_COMPONENTS_DIR);
    match fs::symlink_metadata(&windows) {
        Ok(_) => {
            files::filepaths(&windows)?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Failed to inspect MSHV boot component directory"),
    }
    Ok(())
}

/// Validates discovered installed MSHV components before initializing Host Status.
pub(crate) fn validate_installed_boot_files(esp: &Path) -> Result<(), Error> {
    if has_slot_mshv_boot_components(&esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH))? {
        let installed = esp.join(ESP_EFI_DIRECTORY).join("AZLA");
        if installed.try_exists()? {
            validate_boot_components(&installed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) const FILES: &[&str] = &[
    "bootx64.efi",
    "grubx64.efi",
    "second.efi",
    "HvLoader.efi",
    "lxhvloader.dll",
];
