use std::path::Path;

use log::{debug, trace};

use trident_api::{
    constants::{ESP_EFI_DIRECTORY, ESP_RELATIVE_MOUNT_POINT_PATH},
    error::{ReportError, TridentError, TridentResultExt, UnsupportedConfigurationError},
};

use crate::engine::boot;

/// Returns the next available install index for the current install.
pub fn next_install_index(mount_point: &Path) -> Result<usize, TridentError> {
    let esp_efi_path = mount_point
        .join(ESP_RELATIVE_MOUNT_POINT_PATH)
        .join(ESP_EFI_DIRECTORY);

    debug!(
        "Looking for next available install index in '{}'",
        esp_efi_path.display()
    );
    let first_available_install_index = find_first_available_install_index(&esp_efi_path)
        .message("Failed to find the first available install index")?;

    debug!("Selected first available install index: '{first_available_install_index}'",);
    Ok(first_available_install_index)
}

/// Tries to find the next available AzL install index by looking at the
/// ESP directory names present in the specified ESP EFI path.
fn find_first_available_install_index(esp_efi_path: &Path) -> Result<usize, TridentError> {
    Ok(boot::make_esp_dir_name_candidates()
        // Take a limited number of candidates to avoid an infinite loop.
        .take(1000)
        // Go over all the candidates and find the first one that doesn't exist.
        .find(|(idx, dir_names)| {
            trace!("Checking if an install with index '{}' exists", idx);
            // Returns true if all possible ESP directory names for this index
            // do NOT exist.
            dir_names.iter().all(|dir_names| {
                let path = esp_efi_path.join(dir_names);
                trace!("Checking if path '{}' exists", path.display());
                !path.exists()
            })
        })
        .structured(UnsupportedConfigurationError::NoAvailableInstallIndex)?
        .0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::TempDir;

    use crate::engine::boot::make_esp_dir_name_candidates;

    /// Simple case for find_first_available_install_index
    #[test]
    fn test_find_first_available_install_index_simple() {
        let test_dir = TempDir::new().unwrap();
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 0, "First available index should be 0");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices
    #[test]
    fn test_find_first_available_install_index_existing_all() {
        let test_dir = TempDir::new().unwrap();

        // Create all ESP directories for indices 0-9
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                for dir_name in dir_names {
                    fs::create_dir(test_dir.path().join(dir_name)).unwrap();
                }
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices, even when only the A volume IDs are present
    #[test]
    fn test_find_first_available_install_index_existing_a() {
        let test_dir = TempDir::new().unwrap();

        // Create Volume A ESP directories for indices 0-9
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(test_dir.path().join(&dir_names[0])).unwrap();
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices, even when only the B volume IDs are present
    #[test]
    fn test_find_first_available_install_index_existing_b() {
        let test_dir = TempDir::new().unwrap();

        // Create Volume B ESP directories for indices 0-9
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(test_dir.path().join(&dir_names[1])).unwrap();
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices, even when only ONE ID is present per install.
    #[test]
    fn test_find_first_available_install_index_existing_mixed_1() {
        let test_dir = TempDir::new().unwrap();

        // Iterator to cycle between 0 and 1
        let mut volume_selector = (0..=1).cycle();

        // Create alternating A/B Volume ESP directories for indices 0-9, starting with A
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(
                    test_dir
                        .path()
                        .join(&dir_names[volume_selector.next().unwrap()]),
                )
                .unwrap();
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices, even when only ONE ID is present per install.
    #[test]
    fn test_find_first_available_install_index_existing_mixed_2() {
        let test_dir = TempDir::new().unwrap();

        // Iterator to cycle between 0 and 1
        let mut volume_selector = (0..=1).cycle();

        // Advance the volume selector to start with B
        volume_selector.next();

        // Create alternating A/B Volume ESP directories for indices 0-9, starting with B
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(
                    test_dir
                        .path()
                        .join(&dir_names[volume_selector.next().unwrap()]),
                )
                .unwrap();
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }
}
