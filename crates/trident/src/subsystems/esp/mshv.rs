//! ACL, efitreeloader (ETL), and MSHV-related functionality for ESP provisioning.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use log::{debug, warn};

/// Filename of the second-stage EFI executable for efitreeloader (ETL).
pub(super) const ETL_SECOND_STAGE_EFI_FILENAME: &str = "second.efi";

/// List of top-level MSHV ESP payload binaries that need to be copied.
const MSHV_ESP_PAYLOAD_BINARIES: [&str; 2] = ["efi/HvLoader.efi", "efi/lxhvloader.dll"];

/// Optional ESP-relative directory containing secondary MSHV payload binaries.
/// Contents vary between images, so discover its direct files instead of
/// maintaining a fixed binary list. Subdirectories are ignored.
const MSHV_ESP_PAYLOAD_DIR: &str = "efi/Windows/System32";

/// Copies the MSHV ESP payload files from the source ESP to the destination ESP
/// for a clean install.
///
/// Specifically, it copies the top-level MSHV ESP payload binaries listed in
/// `MSHV_ESP_PAYLOAD_BINARIES` from the source ESP to the destination ESP, and
/// then copies any and all secondary MSHV ESP payload binaries from the
/// `MSHV_ESP_PAYLOAD_DIR` directory directly, preserving ESP-relative paths.
/// Subdirectories are ignored.
/// An absent secondary payload directory is allowed.
pub(super) fn copy_mshv_esp_payload(
    source_esp: &Path,
    destination_esp: &Path,
) -> Result<(), Error> {
    // List of relative paths to copy from the source ESP to the destination ESP.
    let mut files_to_copy: Vec<PathBuf> = Vec::new();

    // Validate and discover the payload before writing, so missing required files
    // or traversal errors do not leave a partially copied payload.
    for relative_path in MSHV_ESP_PAYLOAD_BINARIES {
        let source_path = source_esp.join(relative_path);
        ensure!(
            source_path.is_file(),
            "Required MSHV ESP payload file '{relative_path}' is missing from the source ESP"
        );
        files_to_copy.push(relative_path.into());
    }

    let payload_dir = source_esp.join(MSHV_ESP_PAYLOAD_DIR);
    if payload_dir.try_exists().with_context(|| {
        format!(
            "Failed to inspect MSHV payload directory '{}'",
            payload_dir.display()
        )
    })? {
        for entry in fs::read_dir(&payload_dir).with_context(|| {
            format!(
                "Failed to read MSHV payload directory '{}'",
                payload_dir.display()
            )
        })? {
            let entry = entry
                .with_context(|| format!("Failed to read entry in '{}'", payload_dir.display()))?;
            let file_type = entry.file_type().with_context(|| {
                format!(
                    "Failed to inspect MSHV payload entry '{}'",
                    entry.path().display()
                )
            })?;
            if file_type.is_dir() {
                warn!(
                    "Ignoring unexpected directory '{}' in MSHV ESP payload",
                    entry.path().display()
                );
                continue;
            }
            // Reject symlinks and special files rather than following
            // references outside the payload directory.
            ensure!(
                file_type.is_file(),
                "MSHV payload entry '{}' is not a regular file or directory",
                entry.path().display()
            );
            files_to_copy.push(Path::new(MSHV_ESP_PAYLOAD_DIR).join(entry.file_name()));
        }
    }

    for relative_path in files_to_copy {
        let source_path = source_esp.join(&relative_path);
        let destination_path = destination_esp.join(&relative_path);
        let parent = destination_path.parent().with_context(|| {
            format!(
                "Failed to get parent directory for '{}'",
                destination_path.display()
            )
        })?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create MSHV ESP payload directory '{}'",
                parent.display()
            )
        })?;
        debug!(
            "Copying MSHV ESP payload file '{}' to '{}'",
            source_path.display(),
            destination_path.display()
        );
        fs::copy(&source_path, &destination_path).with_context(|| {
            format!(
                "Failed to copy MSHV ESP payload file '{}' to '{}'",
                source_path.display(),
                destination_path.display()
            )
        })?;
    }

    Ok(())
}
