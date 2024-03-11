use std::{collections::HashSet, path::PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::is_default;

use super::error::InvalidHostConfigurationError;

/// Configuration for the host OS.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct OsConfig {
    /// # Users
    ///
    /// Map of users to configure on the host. The key is the username.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<User>,

    /// Additional Files to add to the image.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_files: Vec<AdditionalFile>,
}

/// Configuration for a specific user.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct User {
    /// Username
    pub name: String,

    /// Password configuration.
    #[serde(default, skip_serializing_if = "is_default")]
    pub password: Password,

    /// Specifies the desired User ID. If not provided, the system will automatically assign a UID.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<i32>,

    /// Primary group to add the user to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_group: Option<String>,

    /// List of secondary groups to add the user to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary_groups: Vec<String>,

    /// List of SSH keys to add to the user's authorized keys. **(IN DEVELOPMENT)**
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_keys: Vec<String>,

    /// SSH configuration for the user. **(IN DEVELOPMENT)**
    #[serde(default, skip_serializing_if = "is_default")]
    pub ssh_mode: SshMode,

    /// Number of days until the password expires, used for setting up password expiry policy.
    #[cfg(feature = "dangerous-options")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dangerous_password_expires_days: Option<u64>,

    /// Command to be executed at startup, providing a way to run custom scripts or applications on user login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_command: Option<String>,
}

/// Password configuration for a user.
///
/// **NOTICE:**
///
/// As a security measure, **Trident does NOT support passwords** for
/// Trident-created users. The only allowed value for this field is a locked
/// password, which is the default when this field is skipped. A locked password
/// means that the user account does not allow logging in using password
/// authentication. It is recommended to use SSH keys for authentication
/// instead.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(
    rename_all = "kebab-case",
    deny_unknown_fields,
    tag = "mode",
    content = "value"
)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum Password {
    /// # \[DEFAULT\] Locked Password
    ///
    /// Lock the user's password. (equivalent to `passwd -l`)
    #[default]
    Locked,

    /// # Plaintext Password
    ///
    /// Set the user's password to a plaintext value.
    #[cfg(feature = "dangerous-options")]
    DangerousPlainText(String),

    /// # Hashed Password
    ///
    /// Set the user's password to a hashed value.
    #[cfg(feature = "dangerous-options")]
    DangerousHashed(String),
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum SshMode {
    /// # \[DEFAULT\] Blocked
    ///
    /// Disable SSH for this entity.
    #[default]
    Block,

    /// # Key Only
    ///
    /// Enable SSH for this entity with KEY only.
    KeyOnly,

    /// # Key and Password
    ///
    /// Enable SSH for this entity with KEY and PASSWORD.
    #[cfg(feature = "dangerous-options")]
    DangerousAllowPassword,
}

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
    pub fn validate(&self) -> Result<(), InvalidHostConfigurationError> {
        if let Some(permissions) = &self.permissions {
            // This catches a fun gotcha: If the permissions field is an octal *integer* value, some
            // YAML tooling will convert it to a decimal integer. Subsquent parsing assumpting it
            // was an octal value would fail.
            if !permissions.starts_with('0') {
                return Err(
                    InvalidHostConfigurationError::AdditionalFileInvalidPermissions(
                        permissions.to_string(),
                        self.destination.display().to_string(),
                    ),
                );
            }
            match u32::from_str_radix(permissions, 8) {
                Ok(v) if v <= 0o777 => (),
                _ => {
                    return Err(
                        InvalidHostConfigurationError::AdditionalFileInvalidPermissions(
                            permissions.to_string(),
                            self.destination.display().to_string(),
                        ),
                    )
                }
            }
        }

        match (&self.content, &self.path) {
            (Some(_), Some(_)) => Err(
                InvalidHostConfigurationError::AdditionalFileHasBothContentAndPath(
                    self.destination.display().to_string(),
                ),
            ),
            (None, None) => Err(
                InvalidHostConfigurationError::AdditionalFileHasNoContentOrPath(
                    self.destination.display().to_string(),
                ),
            ),
            _ => Ok(()),
        }
    }
}

impl OsConfig {
    pub fn validate(&self) -> Result<(), InvalidHostConfigurationError> {
        let mut usernames = HashSet::new();
        for user in &self.users {
            if !usernames.insert(&user.name) {
                return Err(InvalidHostConfigurationError::DuplicateUsernames(
                    user.name.clone(),
                ));
            }
        }

        for file in &self.additional_files {
            file.validate()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_user() {
        let mut config = OsConfig::default();
        assert!(config.validate().is_ok());

        config.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        assert!(config.validate().is_ok());

        config.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_permissions() {
        let mut file = AdditionalFile {
            destination: PathBuf::from("/test"),
            content: Some("...".to_string()),
            path: None,
            permissions: Some("0777".to_string()),
        };
        assert!(file.validate().is_ok());

        file.permissions = Some("invalid".to_string());
        assert_eq!(
            file.validate().unwrap_err(),
            InvalidHostConfigurationError::AdditionalFileInvalidPermissions(
                "invalid".to_string(),
                "/test".to_string()
            )
        );

        file.permissions = Some("0999".to_string());
        assert_eq!(
            file.validate().unwrap_err(),
            InvalidHostConfigurationError::AdditionalFileInvalidPermissions(
                "0999".to_string(),
                "/test".to_string()
            )
        );

        file.permissions = Some("1555".to_string());
        assert_eq!(
            file.validate().unwrap_err(),
            InvalidHostConfigurationError::AdditionalFileInvalidPermissions(
                "1555".to_string(),
                "/test".to_string()
            )
        );

        file.permissions = None;
        assert!(file.validate().is_ok());
    }

    #[test]
    fn test_validate_additional_files() {
        let mut config = OsConfig::default();
        assert!(config.validate().is_ok());

        config.additional_files.push(AdditionalFile {
            destination: PathBuf::from("/test"),
            content: Some("test".to_string()),
            path: None,
            permissions: None,
        });
        assert!(config.validate().is_ok());

        config.additional_files.clear();
        config.additional_files.push(AdditionalFile {
            destination: PathBuf::from("/test"),
            content: None,
            path: Some(PathBuf::from("/test")),
            permissions: None,
        });
        assert!(config.validate().is_ok());

        config.additional_files.clear();
        config.additional_files.push(AdditionalFile {
            destination: PathBuf::from("/test"),
            content: None,
            path: None,
            permissions: None,
        });
        assert_eq!(
            config.validate().unwrap_err(),
            InvalidHostConfigurationError::AdditionalFileHasNoContentOrPath("/test".to_string())
        );

        config.additional_files.clear();
        config.additional_files.push(AdditionalFile {
            destination: PathBuf::from("/test"),
            content: Some("test".to_string()),
            path: Some(PathBuf::from("/test")),
            permissions: None,
        });
        assert_eq!(
            config.validate().unwrap_err(),
            InvalidHostConfigurationError::AdditionalFileHasBothContentAndPath("/test".to_string())
        );
    }
}
