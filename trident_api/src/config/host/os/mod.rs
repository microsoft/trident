use std::collections::HashSet;

use netplan_types::NetworkConfig;
use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use super::error::HostConfigurationStaticValidationError;

pub mod additional_files;
mod network;
pub mod users;

use additional_files::AdditionalFile;
use users::User;

/// Configuration for the host OS.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Os {
    /// Netplan network configuration for the runtime OS.
    ///
    /// See [Netplan YAML Configuration](https://netplan.readthedocs.io/en/stable/netplan-yaml/) for more information.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "network::schema_helpers::make_placeholder_netplan_schema")
    )]
    pub network: Option<NetworkConfig>,

    /// SELinux configuration for the host.
    #[serde(default)]
    pub selinux: Selinux,

    /// Users to configure on the host.
    #[serde(default)]
    pub users: Vec<User>,

    /// Additional Files to add to the image.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub additional_files: Vec<AdditionalFile>,

    /// Hostname of the system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

/// Configuration for selinux mode
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Selinux {
    /// Override the SELinux mode. When not provided, no changes will be made to
    /// the existing configuration.
    pub mode: Option<SelinuxMode>,
}

/// SELinux mode
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum SelinuxMode {
    /// # Disabled
    ///
    /// Set SELinux to disabled. The mode is set by appending `selinux=0` to the
    /// kernel command line.
    Disabled,

    /// # Permissive
    ///
    /// Set SELinux to permissive. The mode is set by appending `selinux=1
    /// enforcing=0` to the kernel command line.
    Permissive,

    /// # Enforcing
    ///
    /// Set SELinux to enforcing. The mode is set by appending `selinux=1
    /// enforcing=1` to the kernel command line.
    Enforcing,
}

/// Configuration for the management OS.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct ManagementOs {
    /// Netplan network configuration for the management OS.
    ///
    /// See [Netplan YAML Configuration](https://netplan.readthedocs.io/en/stable/netplan-yaml/) for more information.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "network::schema_helpers::make_placeholder_netplan_schema")
    )]
    pub network: Option<NetworkConfig>,
}

impl Os {
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        let mut usernames = HashSet::new();
        for user in &self.users {
            if !usernames.insert(&user.name) {
                return Err(HostConfigurationStaticValidationError::DuplicateUsernames(
                    user.name.clone(),
                ));
            }
        }

        for file in &self.additional_files {
            file.validate()?;
        }

        if let Some(network) = self.network.as_ref() {
            network::validate_netplan(network)?;
        }

        Ok(())
    }
}

impl ManagementOs {
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        if let Some(network) = self.network.as_ref() {
            network::validate_netplan(network)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use users::Password;

    use super::*;

    #[test]
    fn test_validate_os_users() {
        let mut config = Os::default();
        assert!(config.validate().is_ok());

        config.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_os_users_fail_duplicate_usernames() {
        let mut config = Os::default();
        config.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        config.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });

        assert_eq!(
            config.validate(),
            Err(HostConfigurationStaticValidationError::DuplicateUsernames(
                "test".to_string()
            ))
        );
    }
}
