use std::{
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error, Result};

/// UKI Addon directory suffix.
pub const UKI_ADDON_DIR_SUFFIX: &str = ".extra.d";
/// UKI Addon file suffix.
pub const UKI_ADDON_FILE_SUFFIX: &str = ".addon.efi";

/// Returns the path to the addon directory associated with the given UKI file,
/// which is expected to be named `<UKI_filename>.extra.d/`. For example, if the
/// UKI file is `vmlinuz-1-azla1.efi`, the associated addon directory would be
/// `vmlinuz-1-azla1.efi.extra.d/` in the same directory as the UKI file.
pub fn uki_addon_dir(uki_path: &Path) -> PathBuf {
    let mut addon_dir = uki_path.to_path_buf().into_os_string();
    addon_dir.push(UKI_ADDON_DIR_SUFFIX);
    PathBuf::from(addon_dir)
}

/// Determines if the given path corresponds to a UKI addon file, which is
/// expected to be a regular file with a name that ends with `.addon.efi`. For
/// example, `vmlinuz-1-azla1.efi.addon.efi` would be considered a UKI addon
/// file, while `vmlinuz-1-azla1.efi` would not.
pub fn is_uki_addon_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }

    path.as_os_str()
        .as_encoded_bytes()
        .ends_with(UKI_ADDON_FILE_SUFFIX.as_bytes())
}

/// Extracts the UKI name from a given addon file path by removing the `.addon.efi` suffix.
/// If the file name does not end with the expected suffix, an error is returned. For example,
/// given the addon file path `vmlinuz-1-azla1.efi.addon.efi`, this function would return
/// `vmlinuz-1-azla1.efi`.
pub fn get_uki_name_from_addon_file(addon_file: &Path) -> Result<OsString, Error> {
    let addon_name = addon_file
        .file_name()
        .with_context(|| format!(
            "Expected a file but found a directory at path '{}' in UKI addons directory.",
            addon_file.display()
        ))?
        .as_encoded_bytes()
        .strip_suffix(UKI_ADDON_FILE_SUFFIX.as_bytes())
        .with_context(|| format!(
            "File '{}' in UKI addons directory does not end with expected suffix '{UKI_ADDON_FILE_SUFFIX}'.",
            addon_file.display(),
        ))?
        .to_vec();

    Ok(OsString::from_vec(addon_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Validates that `uki_addon_dir` appends the `.extra.d` suffix to the
    /// UKI file path to form the addon directory path.
    #[test]
    fn test_uki_addon_dir() {
        let test_cases = vec![
            (
                "/some/path/vmlinuz-1-azla1.efi",
                "/some/path/vmlinuz-1-azla1.efi.extra.d",
            ),
            (
                "/different/path/kernel.efi",
                "/different/path/kernel.efi.extra.d",
            ),
            ("relative/path/uki.efi", "relative/path/uki.efi.extra.d"),
            ("simple.efi", "simple.efi.extra.d"),
            (
                "/root/complex-name_123.efi",
                "/root/complex-name_123.efi.extra.d",
            ),
            (
                "/path/with spaces/file name.efi",
                "/path/with spaces/file name.efi.extra.d",
            ),
            (
                "/path/with.dots.in.name.efi",
                "/path/with.dots.in.name.efi.extra.d",
            ),
            ("", ".extra.d"),
        ];

        for (input_path, expected_path) in test_cases {
            let uki_path = PathBuf::from(input_path);
            let expected_addon_dir = PathBuf::from(expected_path);
            let actual_addon_dir = uki_addon_dir(&uki_path);

            assert_eq!(
                actual_addon_dir, expected_addon_dir,
                "Failed for input path: '{}'",
                input_path
            );

            // Verify the result always ends with the correct suffix
            assert!(
                actual_addon_dir
                    .to_string_lossy()
                    .ends_with(UKI_ADDON_DIR_SUFFIX),
                "Result should always end with UKI_ADDON_DIR_SUFFIX for path: '{}'",
                input_path
            );
        }
    }

    /// Test path handling edge cases
    #[test]
    fn test_uki_addon_dir_edge_cases() {
        // Test with various path formats
        let test_cases = vec![
            // Absolute paths
            ("/usr/lib/kernel.efi", "/usr/lib/kernel.efi.extra.d"),
            ("/", "/.extra.d"),
            // Relative paths
            ("./relative.efi", "./relative.efi.extra.d"),
            ("../parent.efi", "../parent.efi.extra.d"),
            // Paths with multiple extensions
            ("file.tar.gz.efi", "file.tar.gz.efi.extra.d"),
            ("file.backup.old.efi", "file.backup.old.efi.extra.d"),
            // Paths ending with directories
            ("/path/to/dir/", "/path/to/dir/.extra.d"),
        ];

        for (input, expected) in test_cases {
            let uki_path = PathBuf::from(input);
            let result = uki_addon_dir(&uki_path);
            let expected_path = PathBuf::from(expected);

            assert_eq!(result, expected_path, "Failed for input: '{}'", input);
        }
    }

    /// Validates that `is_uki_addon_file` correctly identifies addon files.
    #[test]
    fn test_is_uki_addon_file() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();

        // Test cases: (filename, should_be_addon_file, description)
        let test_cases = vec![
            ("vmlinuz-1-azla1.addon.efi", true, "standard addon file"),
            ("driver.addon.efi", true, "simple addon file"),
            (
                "complex-name_123.addon.efi",
                true,
                "complex addon file name",
            ),
            (
                "with.dots.in.name.addon.efi",
                true,
                "addon file with dots in name",
            ),
            (
                "very-long-name-with-many-components-123_456.addon.efi",
                true,
                "long addon file name",
            ),
            (
                "unicode-ä-ü-ñ.addon.efi",
                true,
                "unicode characters in name",
            ),
            ("123456.addon.efi", true, "numeric addon file name"),
            (".addon.efi", true, "minimal addon file name"),
            (
                "file.addon.efi.backup.addon.efi",
                true,
                "multiple suffix patterns",
            ),
            // Invalid cases
            ("vmlinuz-1-azla1.efi", false, "regular EFI file"),
            ("driver.efi", false, "simple EFI file without addon"),
            ("file.txt", false, "text file"),
            ("no_extension", false, "file without extension"),
            ("wrong.suffix", false, "wrong file suffix"),
            ("file.addon", false, "addon without efi extension"),
            ("file.addonx.efi", false, "similar but wrong addon suffix"),
            ("file.addon.efix", false, "similar but wrong efi suffix"),
            ("", false, "empty filename"),
        ];

        for (filename, should_be_addon, description) in test_cases {
            let file_path = temp_dir.path().join(filename);

            // Only create file if filename is not empty
            if !filename.is_empty() {
                std::fs::write(&file_path, "mock content").unwrap();
            }

            let result = is_uki_addon_file(&file_path);
            assert_eq!(
                result, should_be_addon,
                "Failed for {}: '{}'",
                description, filename
            );
        }

        // Test directory handling
        let addon_dir = temp_dir.path().join("directory.addon.efi");
        std::fs::create_dir_all(&addon_dir).unwrap();
        assert!(
            !is_uki_addon_file(&addon_dir),
            "Directory should not be identified as addon file even with correct suffix"
        );

        // Test non-existent file
        let nonexistent = temp_dir.path().join("nonexistent.addon.efi");
        assert!(
            !is_uki_addon_file(&nonexistent),
            "Non-existent file should not be identified as addon file"
        );

        // Test symlink behavior (if supported on platform)
        #[cfg(unix)]
        {
            let real_file = temp_dir.path().join("real.addon.efi");
            std::fs::write(&real_file, "content").unwrap();
            let symlink_file = temp_dir.path().join("symlink.addon.efi");

            if std::os::unix::fs::symlink(&real_file, &symlink_file).is_ok() {
                assert!(
                    is_uki_addon_file(&symlink_file),
                    "Symlink to addon file should be identified as addon file"
                );
            }
        }
    }

    /// Test successful get_uki_name_from_addon_file name extraction
    /// from valid addon files
    #[test]
    fn test_get_uki_name_from_addon_file_valid() {
        let temp_dir = TempDir::new().unwrap();

        let test_cases = vec![
            ("simple.addon.efi", "simple"),
            ("complex-name_123.addon.efi", "complex-name_123"),
            ("driver.addon.efi", "driver"),
            ("with.dots.in.name.addon.efi", "with.dots.in.name"),
            (
                "with-dashes-and_underscores.addon.efi",
                "with-dashes-and_underscores",
            ),
            ("a.addon.efi", "a"),
            (
                "very-long-name-with-many-components-123_456.addon.efi",
                "very-long-name-with-many-components-123_456",
            ),
        ];

        for (file_name, expected_name) in test_cases {
            let file_path = temp_dir.path().join(file_name);
            std::fs::write(&file_path, "mock addon content").unwrap();

            let result = get_uki_name_from_addon_file(&file_path);
            assert!(
                result.is_ok(),
                "Should successfully extract name from {}",
                file_name
            );

            let extracted_name = result.unwrap();
            assert_eq!(
                extracted_name, expected_name,
                "Extracted name should match expected for {}",
                file_name
            );
        }
    }

    /// Test error cases for get_uki_name_from_addon_file
    #[test]
    fn test_get_uki_name_from_addon_file_errors() {
        let temp_dir = TempDir::new().unwrap();

        // Test with file that doesn't have correct suffix
        let wrong_suffix_file = temp_dir.path().join("wrong.suffix.efi");
        std::fs::write(&wrong_suffix_file, "content").unwrap();

        let result = get_uki_name_from_addon_file(&wrong_suffix_file);
        assert!(
            result.is_err(),
            "Should fail for file without correct suffix"
        );
        let error_msg = result.unwrap_err().to_string();
        assert!(
            error_msg.contains("does not end with expected suffix"),
            "Error should mention suffix mismatch"
        );
        assert!(
            error_msg.contains(".addon.efi"),
            "Error should mention expected suffix"
        );

        // Test with file that has no extension at all
        let no_extension_file = temp_dir.path().join("no_extension");
        std::fs::write(&no_extension_file, "content").unwrap();

        let result = get_uki_name_from_addon_file(&no_extension_file);
        assert!(result.is_err(), "Should fail for file without extension");

        // Test with file that ends with .efi but not .addon.efi
        let wrong_efi_file = temp_dir.path().join("wrong.efi");
        std::fs::write(&wrong_efi_file, "content").unwrap();

        let result = get_uki_name_from_addon_file(&wrong_efi_file);
        assert!(
            result.is_err(),
            "Should fail for .efi file without .addon prefix"
        );
    }

    /// Test with directory paths (should fail)
    #[test]
    fn test_get_uki_name_from_addon_file_directory() {
        let temp_dir = TempDir::new().unwrap();
        let subdir = temp_dir.path().join("some_directory");
        std::fs::create_dir_all(&subdir).unwrap();

        let result = get_uki_name_from_addon_file(&subdir);
        assert!(result.is_err(), "Should fail when path is a directory");
    }

    /// Test edge cases with special characters and empty names
    #[test]
    fn test_get_uki_name_from_addon_file_edge_cases() {
        let temp_dir = TempDir::new().unwrap();

        // Test with Unicode characters (should work)
        let unicode_file = temp_dir.path().join("test-ä-ü-ñ.addon.efi");
        std::fs::write(&unicode_file, "content").unwrap();

        let result = get_uki_name_from_addon_file(&unicode_file);
        assert!(
            result.is_ok(),
            "Should handle Unicode characters in filename"
        );
        assert_eq!(result.unwrap(), "test-ä-ü-ñ");

        // Test with numbers only
        let numbers_file = temp_dir.path().join("123456.addon.efi");
        std::fs::write(&numbers_file, "content").unwrap();

        let result = get_uki_name_from_addon_file(&numbers_file);
        assert!(result.is_ok(), "Should handle numeric-only names");
        assert_eq!(result.unwrap(), "123456");

        // Test minimum valid case - just the suffix
        let minimal_file = temp_dir.path().join(".addon.efi");
        std::fs::write(&minimal_file, "content").unwrap();

        let result = get_uki_name_from_addon_file(&minimal_file);
        assert!(result.is_ok(), "Should handle minimal case with empty name");
        assert_eq!(result.unwrap(), "");
    }

    /// Test with files containing the suffix multiple times
    #[test]
    fn test_get_uki_name_from_addon_file_multiple_suffixes() {
        let temp_dir = TempDir::new().unwrap();

        // Test with name that contains the suffix pattern
        let multiple_suffix_file = temp_dir.path().join("driver.addon.efi.backup.addon.efi");
        std::fs::write(&multiple_suffix_file, "content").unwrap();

        let result = get_uki_name_from_addon_file(&multiple_suffix_file).unwrap();
        assert_eq!(
            result, "driver.addon.efi.backup",
            "Should strip only the final suffix"
        );
    }
}
