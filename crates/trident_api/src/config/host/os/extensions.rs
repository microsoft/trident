use std::{ffi::OsStr, path::PathBuf};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    config::HostConfigurationStaticValidationError,
    constants::{VALID_CONFEXT_DIRECTORIES, VALID_SYSEXT_DIRECTORIES},
    primitives::hash::Sha384Hash,
};

/// Data about an extension image (sysext or confext) to merge onto the target OS.
///
/// Extension image must be a [Discoverable Disk
/// Image](https://uapi-group.org/specifications/specs/discoverable_disk_image/).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Extension {
    /// The path to the extension image file, which must be a [Discoverable Disk
    /// Image](https://uapi-group.org/specifications/specs/discoverable_disk_image/).
    ///
    /// URLs may have one of the following four schemes: `http://`, `https://`, `file://`, or
    /// `oci://`. Extension image files stored in OCI registries must allow for
    /// anonymous pulls.
    pub url: Url,

    /// The Sha384 of the entire extension image file.
    pub sha384: Sha384Hash,

    /// The absolute path of the extension image in the target OS.
    ///
    /// By default, sysexts are placed in /var/lib/extensions/. Trident supports
    /// placing sysexts in:
    /// - /etc/extensions/
    /// - /var/lib/extensions/
    /// - /.extra/sysext
    ///
    /// By default, confexts are placed in /var/lib/confexts/. Trident supports
    /// placing confexts in:
    /// - /var/lib/confexts/
    /// - /usr/lib/confexts/
    /// - /usr/local/lib/confexts/
    ///
    /// /run/sysexts/ and /run/confexts/ are not supported.
    pub path: Option<PathBuf>,
}

impl Extension {
    pub fn validate_sysext(&self) -> Result<(), HostConfigurationStaticValidationError> {
        self.validate(&VALID_SYSEXT_DIRECTORIES)
    }

    pub fn validate_confext(&self) -> Result<(), HostConfigurationStaticValidationError> {
        self.validate(&VALID_CONFEXT_DIRECTORIES)
    }

    fn validate(
        &self,
        valid_directories: &[&str],
    ) -> Result<(), HostConfigurationStaticValidationError> {
        // Ensure that the path, if given, is a valid path for the
        // extension image to be placed.
        let Some(path) = &self.path else {
            return Ok(());
        };

        // 'path' must be a file with file extension ".raw".
        if path.extension() != Some(OsStr::new("raw")) {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidFileExtension {
                    path: path.display().to_string(),
                },
            );
        }

        // Note: parent() only returns None for root paths or prefix-only paths.
        // We will never call parent on such a path as it would be caught by the
        // check for a 'raw' file extension above. For relative paths like
        // "test.raw", it returns Some(""), which will fail the directory check
        // below.
        let provided_dir = path.parent().unwrap_or(path);
        // Check that the directory is valid
        if !valid_directories
            .iter()
            .any(|valid_dir| provided_dir.as_os_str() == OsStr::new(valid_dir))
        {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                    path: path.display().to_string(),
                    valid_directories: valid_directories.join(", "),
                },
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use url::Url;

    use crate::primitives::hash::Sha384Hash;

    fn create_test_extension(path: Option<PathBuf>) -> Extension {
        Extension {
            url: Url::parse("http://example.com/test.raw").unwrap(),
            sha384: Sha384Hash::from("a".repeat(96)),
            path,
        }
    }

    #[test]
    fn test_validate_no_path_succeeds() {
        let ext = create_test_extension(None);
        ext.validate_sysext().unwrap();
        ext.validate_confext().unwrap();
    }

    #[test]
    fn test_validate_valid_sysext_path_succeeds() {
        let path = PathBuf::from("/var/lib/extensions/test.raw");
        let ext = create_test_extension(Some(path.clone()));
        ext.validate_sysext().unwrap();
        assert_eq!(
            ext.validate_confext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                path: path.display().to_string(),
                valid_directories: VALID_CONFEXT_DIRECTORIES.join(", ")
            }
        );
    }

    #[test]
    fn test_validate_valid_confext_path_succeeds() {
        let path = PathBuf::from("/var/lib/confexts/test.raw");
        let ext = create_test_extension(Some(path.clone()));
        ext.validate_confext().unwrap();
        assert_eq!(
            ext.validate_sysext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                path: path.display().to_string(),
                valid_directories: VALID_SYSEXT_DIRECTORIES.join(", ")
            }
        );
    }

    #[test]
    fn test_validate_invalid_directory_fails() {
        let path = PathBuf::from("/opt/invalid/test.raw");
        let ext = create_test_extension(Some(path.clone()));
        assert_eq!(
            ext.validate_sysext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                path: path.display().to_string(),
                valid_directories: VALID_SYSEXT_DIRECTORIES.join(", ")
            }
        );
        assert_eq!(
            ext.validate_confext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                path: path.display().to_string(),
                valid_directories: VALID_CONFEXT_DIRECTORIES.join(", ")
            }
        );
    }

    #[test]
    fn test_validate_no_parent_directory_fails() {
        let path = PathBuf::from("test.raw");
        let ext = create_test_extension(Some(path.clone()));
        assert_eq!(
            ext.validate_sysext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                path: path.display().to_string(),
                valid_directories: VALID_SYSEXT_DIRECTORIES.join(", ")
            }
        );
        assert_eq!(
            ext.validate_confext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                path: path.display().to_string(),
                valid_directories: VALID_CONFEXT_DIRECTORIES.join(", ")
            }
        );
    }

    #[test]
    fn test_validate_relative_path_fails() {
        let path = PathBuf::from("var/lib/extensions/test.raw");
        let ext = create_test_extension(Some(path.clone()));
        assert_eq!(
            ext.validate_sysext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                path: path.display().to_string(),
                valid_directories: VALID_SYSEXT_DIRECTORIES.join(", ")
            }
        );
        assert_eq!(
            ext.validate_confext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                path: path.display().to_string(),
                valid_directories: VALID_CONFEXT_DIRECTORIES.join(", ")
            }
        );
    }

    #[test]
    fn test_validate_invalid_extension_fails() {
        let path = PathBuf::from("/var/lib/extensions/test.img");
        let ext = create_test_extension(Some(path.clone()));
        assert_eq!(
            ext.validate_sysext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidFileExtension {
                path: path.display().to_string(),
            }
        );
        assert_eq!(
            ext.validate_confext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidFileExtension {
                path: path.display().to_string(),
            }
        );
    }

    #[test]
    fn test_validate_no_filename_fails() {
        let path = PathBuf::from("/var/lib/extensions/");
        let ext = create_test_extension(Some(path.clone()));
        assert_eq!(
            ext.validate_sysext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidFileExtension {
                path: path.display().to_string(),
            }
        );
        assert_eq!(
            ext.validate_confext().unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageInvalidFileExtension {
                path: path.display().to_string(),
            }
        );
    }
}
