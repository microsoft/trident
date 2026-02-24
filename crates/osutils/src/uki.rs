use std::path::{Path, PathBuf};

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
    path.is_file()
        && path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(UKI_ADDON_FILE_SUFFIX))
}

pub fn get_uki_name_from_addon_file(addon_file: &Path) -> Result<&str, Error> {
    addon_file
        .file_name()
        .with_context(|| format!(
            "Expected a file but found a directory at path '{}' in UKI addons directory.",
            addon_file.display()
        ))?
        .to_str()
        .with_context(|| format!(
            "Failed to get file name for path '{}' in UKI addons directory.",
            addon_file.display()
        ))?
        .strip_suffix(UKI_ADDON_FILE_SUFFIX)
        .with_context(|| format!(
            "File '{}' in UKI addons directory does not end with expected suffix '{UKI_ADDON_FILE_SUFFIX}'.",
            addon_file.display(),
        ))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Tests for get_uki_name_from_addon_file function
    mod get_uki_name_from_addon_file_tests {
        use super::*;
        use tempfile::TempDir;

        /// Test successful name extraction from valid addon files
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

            let error_msg = result.unwrap_err().to_string();
            // Directory will fail because it doesn't end with .addon.efi suffix
            assert!(
                error_msg.contains("does not end with expected suffix")
                    || error_msg.contains(".addon.efi"),
                "Error should mention suffix issue, got: {}",
                error_msg
            );
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

            let result = get_uki_name_from_addon_file(&multiple_suffix_file);
            assert!(
                result.is_ok(),
                "Should handle file with multiple suffix patterns"
            );
            assert_eq!(
                result.unwrap(),
                "driver.addon.efi.backup",
                "Should strip only the final suffix"
            );
        }

        /// Test non-existent files (should still work for name extraction if path has correct suffix)
        #[test]
        fn test_get_uki_name_from_addon_file_nonexistent() {
            let temp_dir = TempDir::new().unwrap();
            let nonexistent_file = temp_dir.path().join("nonexistent.addon.efi");
            // Don't create the file

            let result = get_uki_name_from_addon_file(&nonexistent_file);
            // This should work because the function only checks the path string, not file existence
            assert!(
                result.is_ok(),
                "Should extract name from path even if file doesn't exist"
            );
            assert_eq!(result.unwrap(), "nonexistent");
        }

        /// Test integration with is_uki_addon_file function
        #[test]
        fn test_integration_with_is_uki_addon_file() {
            let temp_dir = TempDir::new().unwrap();

            let test_files = [
                ("valid.addon.efi", true, Some("valid")),
                ("invalid.txt", false, None),
                ("wrong.efi", false, None),
            ];

            for (file_name, should_be_addon, expected_name) in test_files {
                let file_path = temp_dir.path().join(file_name);
                std::fs::write(&file_path, "content").unwrap();

                let is_addon = is_uki_addon_file(&file_path);
                assert_eq!(
                    is_addon, should_be_addon,
                    "is_uki_addon_file result for {}",
                    file_name
                );

                if should_be_addon {
                    let name_result = get_uki_name_from_addon_file(&file_path);
                    assert!(
                        name_result.is_ok(),
                        "get_uki_name_from_addon_file should succeed for valid addon file"
                    );
                    assert_eq!(name_result.unwrap(), expected_name.unwrap());
                } else {
                    let name_result = get_uki_name_from_addon_file(&file_path);
                    assert!(
                        name_result.is_err(),
                        "get_uki_name_from_addon_file should fail for non-addon file"
                    );
                }
            }
        }
    }

    /// Additional comprehensive tests for uki_addon_dir function
    mod uki_addon_dir_tests {
        use super::*;

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

        /// Test that the function works correctly with different file extensions
        #[test]
        fn test_uki_addon_dir_various_extensions() {
            let extensions = vec![".efi", ".EFI", "", ".bin", ".img", ".kernel"];

            for ext in extensions {
                let filename = format!("kernel{}", ext);
                let expected = format!("{}{}", filename, UKI_ADDON_DIR_SUFFIX);

                let uki_path = PathBuf::from(&filename);
                let result = uki_addon_dir(&uki_path);

                assert_eq!(
                    result.to_string_lossy(),
                    expected,
                    "Failed for extension: '{}'",
                    ext
                );
            }
        }

        /// Test that the function preserves the original path structure
        #[test]
        fn test_uki_addon_dir_path_preservation() {
            let base_path = PathBuf::from("/complex/path/with/multiple/components");
            let uki_filename = "kernel-5.15.0-azl.efi";
            let uki_path = base_path.join(uki_filename);

            let result = uki_addon_dir(&uki_path);

            // Should preserve the directory structure
            assert_eq!(result.parent().unwrap(), base_path);

            // Should have correct filename with suffix
            let result_filename = result.file_name().unwrap().to_string_lossy();
            let expected_filename = format!("{}{}", uki_filename, UKI_ADDON_DIR_SUFFIX);
            assert_eq!(result_filename, expected_filename);
        }

        /// Test with Unicode and special characters in paths
        #[test]
        fn test_uki_addon_dir_unicode_and_special_chars() {
            let test_paths = vec![
                "kernel-ñ-ü-ä.efi",
                "kernel with spaces.efi",
                "kernel-123_456.efi",
                "kernel@host.efi",
                "kernel#hash.efi",
                "kernel$dollar.efi",
                "kernel&ampersand.efi",
            ];

            for path_str in test_paths {
                let uki_path = PathBuf::from(path_str);
                let result = uki_addon_dir(&uki_path);

                // Should always end with the correct suffix
                assert!(
                    result.to_string_lossy().ends_with(UKI_ADDON_DIR_SUFFIX),
                    "Result should end with suffix for path: '{}'",
                    path_str
                );

                // Should contain the original filename
                assert!(
                    result.to_string_lossy().contains(path_str),
                    "Result should contain original path: '{}'",
                    path_str
                );
            }
        }
    }

    /// Additional comprehensive tests for is_uki_addon_file function
    mod is_uki_addon_file_tests {
        use super::*;
        use tempfile::TempDir;

        /// Test file permission scenarios
        #[test]
        #[cfg(unix)]
        fn test_is_uki_addon_file_permissions() {
            use std::os::unix::fs::PermissionsExt;

            let temp_dir = TempDir::new().unwrap();

            // Test with different file permissions
            let test_cases = vec![
                (0o644, true, "read-write for owner, read for others"),
                (0o755, true, "executable file"),
                (0o400, true, "read-only for owner"),
                (0o000, true, "no permissions (file still exists)"),
            ];

            for (mode, should_succeed, description) in test_cases {
                let file_path = temp_dir.path().join(format!("test_{:o}.addon.efi", mode));
                std::fs::write(&file_path, "content").unwrap();

                let mut perms = std::fs::metadata(&file_path).unwrap().permissions();
                perms.set_mode(mode);
                std::fs::set_permissions(&file_path, perms).unwrap();

                let result = is_uki_addon_file(&file_path);
                assert_eq!(
                    result, should_succeed,
                    "Failed for {}: mode {:o}",
                    description, mode
                );
            }
        }

        /// Test with very long filenames
        #[test]
        fn test_is_uki_addon_file_long_names() {
            let temp_dir = TempDir::new().unwrap();

            // Create a very long but valid addon filename
            let long_name = format!("{}.addon.efi", "a".repeat(200));
            let file_path = temp_dir.path().join(&long_name);

            // Only test if the filesystem supports this length
            if std::fs::write(&file_path, "content").is_ok() {
                assert!(
                    is_uki_addon_file(&file_path),
                    "Should handle long addon filenames"
                );
            }
        }

        /// Test case sensitivity
        #[test]
        fn test_is_uki_addon_file_case_sensitivity() {
            let temp_dir = TempDir::new().unwrap();

            let test_cases = vec![
                ("file.addon.efi", true, "lowercase (correct)"),
                ("file.ADDON.EFI", false, "uppercase addon and efi"),
                ("file.Addon.Efi", false, "mixed case"),
                ("file.addon.EFI", false, "uppercase efi only"),
                ("file.ADDON.efi", false, "uppercase addon only"),
            ];

            for (filename, should_be_addon, description) in test_cases {
                let file_path = temp_dir.path().join(filename);
                std::fs::write(&file_path, "content").unwrap();

                let result = is_uki_addon_file(&file_path);
                assert_eq!(
                    result, should_be_addon,
                    "Failed for {}: '{}'",
                    description, filename
                );
            }
        }

        /// Test behavior with broken symlinks
        #[test]
        #[cfg(unix)]
        fn test_is_uki_addon_file_broken_symlinks() {
            let temp_dir = TempDir::new().unwrap();

            // Create a symlink to a non-existent file
            let target_path = temp_dir.path().join("nonexistent.addon.efi");
            let symlink_path = temp_dir.path().join("broken_symlink.addon.efi");

            if std::os::unix::fs::symlink(&target_path, &symlink_path).is_ok() {
                // Broken symlink should not be considered a valid addon file
                assert!(
                    !is_uki_addon_file(&symlink_path),
                    "Broken symlink should not be identified as addon file"
                );
            }
        }

        /// Test with empty files and zero-byte files
        #[test]
        fn test_is_uki_addon_file_empty_files() {
            let temp_dir = TempDir::new().unwrap();

            // Empty file should still be valid addon file if named correctly
            let empty_file = temp_dir.path().join("empty.addon.efi");
            std::fs::write(&empty_file, "").unwrap();

            assert!(
                is_uki_addon_file(&empty_file),
                "Empty file with correct name should be valid addon file"
            );

            // File with just whitespace
            let whitespace_file = temp_dir.path().join("whitespace.addon.efi");
            std::fs::write(&whitespace_file, "   \n  \t  ").unwrap();

            assert!(
                is_uki_addon_file(&whitespace_file),
                "File with whitespace should be valid addon file"
            );
        }
    }

    /// Integration and cross-function tests
    mod integration_tests {
        use super::*;
        use tempfile::TempDir;

        /// Test the workflow of creating addon directory and checking addon files
        #[test]
        fn test_uki_workflow_integration() {
            let temp_dir = TempDir::new().unwrap();
            let uki_path = temp_dir.path().join("kernel.efi");

            // Create the UKI file
            std::fs::write(&uki_path, "mock uki content").unwrap();

            // Get the expected addon directory
            let addon_dir = uki_addon_dir(&uki_path);
            assert!(addon_dir.to_string_lossy().ends_with(".extra.d"));

            // Create the addon directory
            std::fs::create_dir_all(&addon_dir).unwrap();

            // Create some addon files in the directory
            let addon_files = [
                "driver1.addon.efi",
                "driver2.addon.efi",
                "firmware.addon.efi",
            ];

            for addon_file in &addon_files {
                let addon_path = addon_dir.join(addon_file);
                std::fs::write(&addon_path, "mock addon content").unwrap();

                // Verify each file is correctly identified as addon file
                assert!(
                    is_uki_addon_file(&addon_path),
                    "File {} should be identified as addon file",
                    addon_file
                );

                // Verify name extraction works
                let extracted_name = get_uki_name_from_addon_file(&addon_path).unwrap();
                let expected_name = addon_file.strip_suffix(UKI_ADDON_FILE_SUFFIX).unwrap();
                assert_eq!(
                    extracted_name, expected_name,
                    "Name extraction should work for {}",
                    addon_file
                );
            }

            // Create some non-addon files that should be ignored
            let non_addon_files = ["readme.txt", "config.ini", "driver.efi"];
            for non_addon_file in &non_addon_files {
                let non_addon_path = addon_dir.join(non_addon_file);
                std::fs::write(&non_addon_path, "not an addon").unwrap();

                assert!(
                    !is_uki_addon_file(&non_addon_path),
                    "File {} should NOT be identified as addon file",
                    non_addon_file
                );
            }
        }

        /// Test consistency between all functions with various inputs
        #[test]
        fn test_function_consistency() {
            let temp_dir = TempDir::new().unwrap();

            let test_kernels = [
                "vmlinuz-5.15.0.efi",
                "kernel.efi",
                "boot.efi",
                "complex-name_123.efi",
            ];

            for kernel_name in &test_kernels {
                let uki_path = temp_dir.path().join(kernel_name);
                std::fs::write(&uki_path, "uki content").unwrap();

                // Test addon directory generation
                let addon_dir = uki_addon_dir(&uki_path);
                let expected_addon_name = format!("{}.extra.d", kernel_name);
                assert!(
                    addon_dir.to_string_lossy().ends_with(&expected_addon_name),
                    "Addon directory should have correct suffix for {}",
                    kernel_name
                );

                // Create addon directory and test addon files
                std::fs::create_dir_all(&addon_dir).unwrap();

                let addon_name = kernel_name.strip_suffix(".efi").unwrap_or(kernel_name);
                let addon_filename = format!("{}-driver.addon.efi", addon_name);
                let addon_path = addon_dir.join(&addon_filename);
                std::fs::write(&addon_path, "addon content").unwrap();

                // All functions should work consistently
                assert!(
                    is_uki_addon_file(&addon_path),
                    "Addon file should be correctly identified for {}",
                    kernel_name
                );

                let extracted_name = get_uki_name_from_addon_file(&addon_path).unwrap();
                let expected_name = format!("{}-driver", addon_name);
                assert_eq!(
                    extracted_name, expected_name,
                    "Name extraction should be consistent for {}",
                    kernel_name
                );
            }
        }
    }
}
