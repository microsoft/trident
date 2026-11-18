//! Validates MSHV boot components independently of the surrounding bootloader chain.

use std::{fs, io::ErrorKind, path::Path};

use anyhow::{ensure, Context, Error};

use crate::subsystems::esp::boot_tree;

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
            boot_tree::filepaths(&windows)?;
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("Failed to inspect MSHV boot component directory"),
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
