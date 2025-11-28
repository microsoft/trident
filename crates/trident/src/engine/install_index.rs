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

    use std::{fs, path::PathBuf};

    use tempfile::TempDir;

    use trident_api::error::ErrorKind;

    use crate::engine::boot::make_esp_dir_name_candidates;

    // Helper that constructs the ESP EFI path, creates the directories, and returns the path.
    fn setup_esp_efi_path(mount_point: &Path) -> PathBuf {
        let esp_efi_path = mount_point
            .join(ESP_RELATIVE_MOUNT_POINT_PATH)
            .join(ESP_EFI_DIRECTORY);
        fs::create_dir_all(&esp_efi_path).unwrap();
        esp_efi_path
    }

    #[test]
    fn test_install_index_variants() {
        // Test case #0: No existing install directories.
        let test_dir = TempDir::new().unwrap();
        let esp_efi_path = setup_esp_efi_path(test_dir.path());
        assert_eq!(next_install_index(test_dir.path()).unwrap(), 0);
        assert_eq!(
            find_first_available_install_index(&esp_efi_path).unwrap(),
            0
        );

        // Test case #1: Install directories 0-9 exist.
        let test_dir = TempDir::new().unwrap();
        let esp_efi_path = setup_esp_efi_path(test_dir.path());
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                for dir_name in dir_names {
                    fs::create_dir(esp_efi_path.join(dir_name)).unwrap();
                }
            });
        assert_eq!(next_install_index(test_dir.path()).unwrap(), 10);
        assert_eq!(
            find_first_available_install_index(&esp_efi_path).unwrap(),
            10
        );

        // Test case #2: Install directories 0-9 exist. Func will skip unavailable indices, even
        // when only the A volume IDs are present.
        let test_dir = TempDir::new().unwrap();
        let esp_efi_path = setup_esp_efi_path(test_dir.path());
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(esp_efi_path.join(&dir_names[0])).unwrap();
            });
        assert_eq!(next_install_index(test_dir.path()).unwrap(), 10);
        assert_eq!(
            find_first_available_install_index(&esp_efi_path).unwrap(),
            10
        );

        // Test case #3: Install directories 0-9 exist. Func will skip unavailable indices, even
        // when only the A volume IDs are present.
        let test_dir = TempDir::new().unwrap();
        let esp_efi_path = setup_esp_efi_path(test_dir.path());
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(esp_efi_path.join(&dir_names[1])).unwrap();
            });
        assert_eq!(next_install_index(test_dir.path()).unwrap(), 10);
        assert_eq!(
            find_first_available_install_index(&esp_efi_path).unwrap(),
            10
        );

        // Test case #4: Install directories 0-9 exist. Func will skip unavailable indices, even
        // when only one ID is present per install.
        let test_dir = TempDir::new().unwrap();
        let esp_efi_path = setup_esp_efi_path(test_dir.path());
        let mut volume_selector = (0..=1).cycle();
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(esp_efi_path.join(&dir_names[volume_selector.next().unwrap()]))
                    .unwrap();
            });
        assert_eq!(next_install_index(test_dir.path()).unwrap(), 10);
        assert_eq!(
            find_first_available_install_index(&esp_efi_path).unwrap(),
            10
        );

        let test_dir = TempDir::new().unwrap();
        let esp_efi_path = setup_esp_efi_path(test_dir.path());
        let mut volume_selector = (0..=1).cycle();
        volume_selector.next(); // Advance to start with B
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(esp_efi_path.join(&dir_names[volume_selector.next().unwrap()]))
                    .unwrap();
            });
        assert_eq!(next_install_index(test_dir.path()).unwrap(), 10);
        assert_eq!(
            find_first_available_install_index(&esp_efi_path).unwrap(),
            10
        );
    }

    #[test]
    fn test_no_available_install_index() {
        let test_dir = tempfile::TempDir::new().unwrap();
        let esp_efi_path = setup_esp_efi_path(test_dir.path());

        // Exhaust all possible indices (up to 1000)
        crate::engine::boot::make_esp_dir_name_candidates()
            .take(1000)
            .for_each(|(_, dir_names)| {
                for dir_name in dir_names {
                    std::fs::create_dir(esp_efi_path.join(dir_name)).unwrap();
                }
            });

        assert_eq!(
            find_first_available_install_index(&esp_efi_path)
                .unwrap_err()
                .kind(),
            &ErrorKind::UnsupportedConfiguration(
                UnsupportedConfigurationError::NoAvailableInstallIndex
            )
        );

        assert_eq!(
            next_install_index(test_dir.path()).unwrap_err().kind(),
            &ErrorKind::UnsupportedConfiguration(
                UnsupportedConfigurationError::NoAvailableInstallIndex
            )
        );
    }
}
