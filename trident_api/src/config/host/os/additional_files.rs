use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::config::HostConfigurationStaticValidationError;

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct AdditionalFile {
    /// Location on the target image to place the file.
    pub destination: PathBuf,

    /// The contents of the script. Conflicts with path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Path to the script file. Conflicts with content.
    ///
    /// The file must be located on the host's filesystem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,

    /// Permissions to set on the file.
    ///
    /// If not specified, this will default to the permissions of the source file when `path` is
    /// used and to 0644 when `content` is used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<String>,
}

impl AdditionalFile {
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        if let Some(permissions) = &self.permissions {
            // This catches a fun gotcha: If the permissions field is an octal *integer* value, some
            // YAML tooling will convert it to a decimal integer. Subsquent parsing assumpting it
            // was an octal value would fail.
            if !permissions.starts_with('0') {
                return Err(
                    HostConfigurationStaticValidationError::AdditionalFileInvalidPermissions {
                        additional_file: self.destination.display().to_string(),
                        permissions: permissions.to_string(),
                    },
                );
            }
            match u32::from_str_radix(permissions, 8) {
                Ok(v) if v <= 0o777 => (),
                _ => {
                    return Err(
                        HostConfigurationStaticValidationError::AdditionalFileInvalidPermissions {
                            additional_file: self.destination.display().to_string(),
                            permissions: permissions.to_string(),
                        },
                    )
                }
            }
        }

        match (&self.content, &self.path) {
            (Some(_), Some(_)) => Err(
                HostConfigurationStaticValidationError::AdditionalFileBothContentAndPath {
                    additional_file: self.destination.display().to_string(),
                },
            ),
            (None, None) => Err(
                HostConfigurationStaticValidationError::AdditionalFileNoContentOrPath {
                    additional_file: self.destination.display().to_string(),
                },
            ),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_additional_file() {
        let mut file = AdditionalFile {
            destination: PathBuf::from("/test"),
            content: Some("...".to_string()),
            path: None,
            permissions: Some("0777".to_string()),
        };
        assert!(file.validate().is_ok());

        file.permissions = None;
        assert!(file.validate().is_ok());

        // Providing path and setting content to None is also valid
        file = AdditionalFile {
            destination: PathBuf::from("/test"),
            content: None,
            path: Some(PathBuf::from("/test")),
            permissions: None,
        };
        assert!(file.validate().is_ok());
    }

    #[test]
    fn test_validate_additional_file_fail_invalid_permissions() {
        let mut file = AdditionalFile {
            destination: PathBuf::from("/test"),
            content: Some("...".to_string()),
            path: None,
            permissions: Some("invalid".to_string()),
        };
        assert_eq!(
            file.validate().unwrap_err(),
            HostConfigurationStaticValidationError::AdditionalFileInvalidPermissions {
                additional_file: "/test".to_string(),
                permissions: "invalid".to_string(),
            }
        );

        file.permissions = Some("0999".to_string());
        assert_eq!(
            file.validate().unwrap_err(),
            HostConfigurationStaticValidationError::AdditionalFileInvalidPermissions {
                additional_file: "/test".to_string(),
                permissions: "0999".to_string(),
            }
        );

        file.permissions = Some("1555".to_string());
        assert_eq!(
            file.validate().unwrap_err(),
            HostConfigurationStaticValidationError::AdditionalFileInvalidPermissions {
                additional_file: "/test".to_string(),
                permissions: "1555".to_string(),
            }
        );
    }

    #[test]
    fn test_validate_fail_both_content_path() {
        let file = AdditionalFile {
            destination: PathBuf::from("/test"),
            content: Some("test".to_string()),
            path: Some(PathBuf::from("/test")),
            permissions: None,
        };
        assert_eq!(
            file.validate().unwrap_err(),
            HostConfigurationStaticValidationError::AdditionalFileBothContentAndPath {
                additional_file: "/test".to_string()
            }
        );
    }

    #[test]
    fn test_validate_fail_no_content_or_path() {
        let file = AdditionalFile {
            destination: PathBuf::from("/test"),
            content: None,
            path: None,
            permissions: None,
        };
        assert_eq!(
            file.validate().unwrap_err(),
            HostConfigurationStaticValidationError::AdditionalFileNoContentOrPath {
                additional_file: "/test".to_string()
            }
        );
    }
}
