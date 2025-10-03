use std::path::PathBuf;

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
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Extension {
    /// The path to the extension image file.
    ///
    /// URLs may have one of the following four schemes: `http://`, `https://`, `file://`, or
    /// `oci://`. Extension image files stored in OCI registries must allow for
    /// anonymous pulls.
    pub url: Url,

    /// The Sha384 of the entire extension image file.
    pub sha384: Sha384Hash,

    /// The absolute path of the extension image in the target OS.
    ///
    /// By default, sysexts are placed in /var/lib/extensions/. Trident also
    /// supports placing sysexts in:
    /// - /etc/extensions/
    /// - /var/lib/extensions/
    /// - /.extra/sysext
    ///
    /// By default, confexts are placed in /var/lib/confexts/. Trident also
    /// supports placing confexts in:
    /// - /var/lib/confexts/
    /// - /usr/lib/confexts/
    /// - /usr/local/lib/confexts/
    pub location: Option<PathBuf>,
}

impl Extension {
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        // Ensure that the location, if given, is a valid location for the
        // extension image to be placed
        let Some(location) = &self.location else {
            return Ok(());
        };

        // 'location' is a directory, not a file
        if location.as_os_str().to_string_lossy().ends_with('/') {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidPath {
                    location: location.display().to_string(),
                    message: "Location cannot be a directory".to_string(),
                },
            );
        }

        // File must end in *.raw
        let Some(filename) = location.file_name().and_then(|f| f.to_str()) else {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidPath {
                    location: location.display().to_string(),
                    message: "Could not retrieve file name".to_string(),
                },
            );
        };
        if !filename.ends_with(".raw") {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidFileExtension {
                    location: location.display().to_string(),
                },
            );
        }

        // Check that the directory is valid
        let Some(provided_dir) = location.parent() else {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidPath {
                    location: location.display().to_string(),
                    message: "Could not retrieve directory path".to_string(),
                },
            );
        };
        if !VALID_CONFEXT_DIRECTORIES
            .iter()
            .chain(VALID_SYSEXT_DIRECTORIES.iter())
            .any(|valid_dir| provided_dir == PathBuf::from(valid_dir))
        {
            return Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                    location: location.display().to_string(),
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

    fn create_test_extension(location: Option<PathBuf>) -> Extension {
        Extension {
            url: Url::parse("http://example.com/test.raw").unwrap(),
            sha384: Sha384Hash::from("a".repeat(96)),
            location,
        }
    }

    #[test]
    fn test_validate_no_location_succeeds() {
        let ext = create_test_extension(None);
        assert!(ext.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_sysext_location_succeeds() {
        let ext = create_test_extension(Some(PathBuf::from("/var/lib/extensions/test.raw")));
        assert!(ext.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_confext_location_succeeds() {
        let ext = create_test_extension(Some(PathBuf::from("/var/lib/confexts/test.raw")));
        assert!(ext.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_directory_fails() {
        let location = PathBuf::from("/opt/invalid/test.raw");
        let ext = create_test_extension(Some(location.clone()));
        assert_eq!(
            ext.validate(),
            Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                    location: location.display().to_string(),
                }
            )
        );
    }

    #[test]
    fn test_validate_no_parent_directory_fails() {
        let location = PathBuf::from("test.raw");
        let ext = create_test_extension(Some(location.clone()));
        assert_eq!(
            ext.validate(),
            Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidDirectory {
                    location: location.display().to_string(),
                }
            )
        );
    }

    #[test]
    fn test_validate_invalid_extension_fails() {
        let location = PathBuf::from("/var/lib/extensions/test.img");
        let ext = create_test_extension(Some(location.clone()));
        assert_eq!(
            ext.validate(),
            Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidFileExtension {
                    location: location.display().to_string(),
                }
            )
        );
    }

    #[test]
    fn test_validate_no_filename_fails() {
        let location = PathBuf::from("/var/lib/extensions/");
        let ext = create_test_extension(Some(location.clone()));
        assert_eq!(
            ext.validate(),
            Err(
                HostConfigurationStaticValidationError::ExtensionImageInvalidPath {
                    location: location.display().to_string(),
                    message: "Location cannot be a directory".to_string(),
                }
            )
        );
    }
}
