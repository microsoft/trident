use std::collections::HashSet;

use netplan_types::NetworkConfig;
use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::is_default;

use super::error::HostConfigurationStaticValidationError;

pub mod additional_files;
pub mod modules;
mod network;
pub mod services;
pub mod users;

use additional_files::AdditionalFile;
use modules::Module;
use services::Services;
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
    pub netplan: Option<NetworkConfig>,

    /// SELinux configuration for the host.
    ///
    /// Note: SELinux cannot be used in conjunction with vfat or NTFS filesystems. When SELinux is
    /// set to permissive or enforcing, the setfiles operation will be skipped for any filesystems
    /// of type vfat or NTFS.
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

    /// Kernel modules to configure.
    #[serde(default, skip_serializing_if = "is_default")]
    pub modules: Vec<Module>,

    /// Options for configuring systemd services.
    #[serde(default, skip_serializing_if = "is_default")]
    pub services: Services,

    /// Options for configuring the kernel.
    #[serde(default, skip_serializing_if = "is_default")]
    pub kernel_command_line: KernelCommandLine,
}

/// Additional kernel command line options to add to the image.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct KernelCommandLine {
    pub extra_command_line: Vec<String>,
}

/// Configuration for SELinux mode
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Selinux {
    /// Override the SELinux mode. When not provided, no changes will be made to the existing
    /// configuration.
    ///
    /// Note: Trident only supports SELinux and root verity together when running in UKI-mode.
    /// Otherwise when using verity, SELinux must not be enabled in the OS image or SELinux should
    /// be explicitly set to `disabled`.
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

impl std::fmt::Display for SelinuxMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode_str = match self {
            SelinuxMode::Disabled => "disabled",
            SelinuxMode::Permissive => "permissive",
            SelinuxMode::Enforcing => "enforcing",
        };
        write!(f, "{mode_str}")
    }
}

impl std::str::FromStr for SelinuxMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "disabled" => Ok(SelinuxMode::Disabled),
            "permissive" => Ok(SelinuxMode::Permissive),
            "enforcing" => Ok(SelinuxMode::Enforcing),
            _ => Err(anyhow::anyhow!("Invalid SELinux mode: {}", s)),
        }
    }
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
    pub netplan: Option<NetworkConfig>,

    /// Users to configure on the management OS.
    #[serde(default)]
    pub users: Vec<User>,
}

impl Os {
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        let mut usernames = HashSet::new();
        for user in &self.users {
            if !usernames.insert(&user.name) {
                return Err(HostConfigurationStaticValidationError::DuplicateUsernames {
                    username: user.name.clone(),
                });
            }
        }

        for file in &self.additional_files {
            file.validate()?;
        }

        if let Some(network) = self.netplan.as_ref() {
            network::validate_netplan(network)?;
        }

        Ok(())
    }
}

impl ManagementOs {
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        if let Some(network) = self.netplan.as_ref() {
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
        config.validate().unwrap();

        config.users.push(User {
            name: "test".to_string(),
            password: Password::Locked,
            ..Default::default()
        });
        config.validate().unwrap();
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
            Err(HostConfigurationStaticValidationError::DuplicateUsernames {
                username: "test".to_string()
            })
        );
    }
}
