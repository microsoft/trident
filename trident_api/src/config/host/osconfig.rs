use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::is_default;

/// Configuration for the host OS.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct OsConfig {
    /// # Users
    ///
    /// Map of users to configure on the host. The key is the username.
    #[serde(default)]
    pub users: HashMap<String, User>,
}

/// Configuration for a specific user.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct User {
    /// Password configuration.
    #[serde(default, skip_serializing_if = "is_default")]
    pub password: Password,

    /// List of groups to add the user to. **(IN DEVELOPMENT)**
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,

    /// List of SSH keys to add to the user's authorized keys. **(IN DEVELOPMENT)**
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ssh_keys: Vec<String>,

    /// SSH configuration for the user. **(IN DEVELOPMENT)**
    #[serde(default)]
    #[serde(skip_serializing_if = "is_default")]
    pub ssh_mode: SshMode,
}

/// Password configuration for a user.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(
    rename_all = "kebab-case",
    deny_unknown_fields,
    tag = "mode",
    content = "value"
)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum Password {
    /// # [DEFAULT] Locked Password
    ///
    /// Lock the user's password. (equivalent to `passwd -l`)
    #[default]
    Locked,

    /// # Plaintext Password
    ///
    /// Set the user's password to a plaintext value.
    DangerousPlainText(String),

    /// # Hashed Password
    ///
    /// Set the user's password to a hashed value.
    DangerousHashed(String),
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum SshMode {
    /// # [DEFAULT] Blocked
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
    DangerousAllowPassword,
}
