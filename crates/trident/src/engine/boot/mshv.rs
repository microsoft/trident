//! Validates MSHV boot components independently of the surrounding bootloader chain.

use std::{fs, io::ErrorKind, path::Path};

use anyhow::{ensure, Context, Error};

use trident_api::constants::{EFI_DEFAULT_BIN_RELATIVE_PATH, ESP_EFI_DIRECTORY};

use super::files;

const MSHV_BOOT_FILES: &[&str] = &["HvLoader.efi", "lxhvloader.dll"];
const MSHV_BOOT_COMPONENTS_DIR: &str = "Windows";

/// Detects MSHV components in a slot or fallback BOOT directory.
///
/// Any loader filename or Windows entry triggers validation of both required
/// loaders and the optional Windows tree. Returns true only after validation;
/// partial components are an error, not an ordinary non-MSHV directory.
/// Returns false when no markers exist, including when the directory is absent.
/// Filesystem inspection errors propagate without modifying the directory.
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

/// Requires regular HvLoader.efi and lxhvloader.dll files in `boot_dir`.
///
/// Windows is optional, but when present must be a directory tree containing
/// only regular files/directories, with no links or reserved replacement names.
/// Checks filesystem structure, not binary contents or runtime hypervisor identity.
/// Supporting bootloaders, including ETL's second stage, are validated separately.
/// This function does not modify the directory.
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

/// Validates MSHV components used to seed or adopt the initial AZLA slot.
///
/// MSHV markers in EFI/BOOT trigger fallback validation and, if EFI/AZLA already
/// exists, validation of its required loaders and optional Windows tree. A missing
/// AZLA is left for the caller to bootstrap. Without BOOT markers, this check
/// returns without inspecting AZLA.
///
/// This is read-only and limited to BOOT/AZLA, not AZLB or numbered installations.
/// It checks filesystem structure, not runtime hypervisor identity.
pub(crate) fn validate_mshv_bootstrap_components(esp: &Path) -> Result<(), Error> {
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
