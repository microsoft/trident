use std::collections::HashSet;
use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    str::FromStr,
};

use log::warn;
use netplan_types::NetworkConfig;
use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::is_default;

use super::error::HostConfigurationStaticValidationError;

pub mod additional_files;
pub mod extensions;
pub mod modules;
mod network;
pub mod services;
pub mod users;

use additional_files::AdditionalFile;
use extensions::Extension;
use modules::Module;
use services::Services;
use users::User;

/// Configuration for the host OS.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Os {
    /// Netplan network configuration for the target OS.
    ///
    /// See [Netplan YAML Configuration](https://netplan.readthedocs.io/en/stable/netplan-yaml/) for more information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

    /// Data about systext images, which should be merged on the target OS.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub sysexts: Vec<Extension>,

    /// Data about confext images, which should be merged on the target OS.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub confexts: Vec<Extension>,

    /// Options for configuring the UEFI fallback.
    #[serde(default, skip_serializing_if = "is_default")]
    pub uefi_fallback: Option<UefiFallbackMode>,
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
    /// Note: Trident only supports SELinux and root-verity together when running in UKI-mode.
    /// Otherwise, when using root-verity, SELinux must not be enabled in the OS image or SELinux
    /// should be explicitly set to `disabled`.
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

impl Display for SelinuxMode {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        let mode_str = match self {
            SelinuxMode::Disabled => "disabled",
            SelinuxMode::Permissive => "permissive",
            SelinuxMode::Enforcing => "enforcing",
        };
        write!(f, "{mode_str}")
    }
}

impl FromStr for SelinuxMode {
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

/// UEFIFallback mode
///
/// UEFI provides a mechanism for booting from an EFI file without
/// a corresponding boot variable existing in NVRAM. This is known
/// as the UEFI fallback mode, and it uses a specific file path
/// (\EFI\BOOT) to locate the fallback bootloader.
///
/// This configuration option allows specifying how Trident should
/// populate the UEFI fallback boot files during OS installation or
/// update.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum UefiFallbackMode {
    /// # Rollback
    ///
    /// In 'rollback' mode, the servicing OS boot files will be used as
    /// the UEFI fallback boot files. This mode is not valid for clean
    /// installs.
    Rollback,

    /// # Rollforward
    ///
    /// In 'rollforward' mode, the newly installed or updated OS (the
    /// target OS) boot files will be used as the UEFI fallback boot files.
    Rollforward,

    /// # None
    ///
    /// This is the default mode, where no UEFI fallback boot files
    /// are installed.
    None,
}

impl Display for UefiFallbackMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode_str = match self {
            UefiFallbackMode::Rollback => "rollback",
            UefiFallbackMode::Rollforward => "rollforward",
            UefiFallbackMode::None => "none",
        };
        write!(f, "{mode_str}")
    }
}

impl FromStr for UefiFallbackMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "rollback" => Ok(UefiFallbackMode::Rollback),
            "rollforward" => Ok(UefiFallbackMode::Rollforward),
            "none" => Ok(UefiFallbackMode::None),
            _ => Err(anyhow::anyhow!("Invalid UEFI fallback mode: {}", s)),
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

        // Warn if SELinux is not disabled and sysexts or confexts are specified.
        if let Some(selinux_mode) = self.selinux.mode {
            if !(self.sysexts.is_empty() && self.confexts.is_empty())
                && selinux_mode != SelinuxMode::Disabled
            {
                warn!(
                    "The use of SELinux with sysexts and confexts is not \
                    supported. SELinux mode should be set to 'disabled'."
                );
            }
        }

        // Validate sysexts
        let mut sysext_hashes = HashSet::new();
        let mut sysext_paths = HashSet::new();
        self.sysexts.iter().try_for_each(|ext| {
            // Ensure there are no duplicate images
            if !sysext_hashes.insert(&ext.sha384) {
                return Err(
                    HostConfigurationStaticValidationError::DuplicateExtensionImage {
                        hash: ext.sha384.to_string(),
                    },
                );
            }
            // Ensure that no images are placed at the same path
            if let Some(path) = &ext.path {
                if !sysext_paths.insert(path) {
                    return Err(
                        HostConfigurationStaticValidationError::DuplicateExtensionImagePath {
                            path: path.display().to_string(),
                        },
                    );
                }
            }

            ext.validate_sysext()?;
            Ok(())
        })?;

        // Validate confexts
        let mut confext_hashes = HashSet::new();
        let mut confext_paths = HashSet::new();
        self.confexts.iter().try_for_each(|ext| {
            if !confext_hashes.insert(&ext.sha384) {
                return Err(
                    HostConfigurationStaticValidationError::DuplicateExtensionImage {
                        hash: ext.sha384.to_string(),
                    },
                );
            }
            if let Some(path) = &ext.path {
                if !confext_paths.insert(path) {
                    return Err(
                        HostConfigurationStaticValidationError::DuplicateExtensionImagePath {
                            path: path.display().to_string(),
                        },
                    );
                }
            }
            ext.validate_confext()?;
            Ok(())
        })?;

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
    use super::*;

    use std::path::PathBuf;

    use url::Url;

    use users::Password;

    use crate::primitives::hash::Sha384Hash;

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

    #[test]
    fn test_validate_extensions_success() {
        let mut config = Os::default();
        config.sysexts.push(Extension {
            url: Url::parse("http://example.com/ext1.raw").unwrap(),
            sha384: Sha384Hash::from("a".repeat(96)),
            path: Some(PathBuf::from("/var/lib/extensions/ext1.raw")),
        });
        config.sysexts.push(Extension {
            url: Url::parse("http://example.com/ext2.raw").unwrap(),
            sha384: Sha384Hash::from("b".repeat(96)),
            path: None,
        });
        config.validate().unwrap();
    }

    #[test]
    fn test_validate_extensions_fail_duplicate_hash() {
        let mut config = Os::default();
        let duplicate_hash = Sha384Hash::from("a".repeat(96));
        config.sysexts.push(Extension {
            url: Url::parse("http://example.com/ext1.raw").unwrap(),
            sha384: duplicate_hash.clone(),
            path: Some(PathBuf::from("/var/lib/extensions/ext1.raw")),
        });
        config.sysexts.push(Extension {
            url: Url::parse("http://example.com/ext2.raw").unwrap(),
            sha384: duplicate_hash.clone(),
            path: Some(PathBuf::from("/var/lib/extensions/ext2.raw")),
        });

        assert_eq!(
            config.validate().unwrap_err(),
            HostConfigurationStaticValidationError::DuplicateExtensionImage {
                hash: duplicate_hash.to_string()
            }
        );
    }

    #[test]
    fn test_validate_extensions_fail_duplicate_path() {
        let mut config = Os::default();
        let duplicate_path = PathBuf::from("/var/lib/extensions/ext.raw");
        config.sysexts.push(Extension {
            url: Url::parse("http://example.com/ext1.raw").unwrap(),
            sha384: Sha384Hash::from("a".repeat(96)),
            path: Some(duplicate_path.clone()),
        });
        config.sysexts.push(Extension {
            url: Url::parse("http://example.com/ext2.raw").unwrap(),
            sha384: Sha384Hash::from("b".repeat(96)),
            path: Some(duplicate_path.clone()),
        });

        assert_eq!(
            config.validate().unwrap_err(),
            HostConfigurationStaticValidationError::DuplicateExtensionImagePath {
                path: duplicate_path.display().to_string()
            }
        );
    }
}
