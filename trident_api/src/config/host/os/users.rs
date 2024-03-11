use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::is_default;

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

    /// List of SSH keys to add to the user's authorized keys.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_public_keys: Vec<String>,

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
    tag = "type",
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
