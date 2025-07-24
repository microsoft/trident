use std::{io::Write, path::Path, process::Command};

use anyhow::{Context, Error};
use log::{debug, warn};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use trident_api::config::{KernelCommandLine, Module, Selinux, Services};

use crate::{exe::RunAndCheck, osmodifier};

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct OSModifierConfig {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<MICUser>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<Module>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub services: Option<Services>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_command_line: Option<KernelCommandLine>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub selinux: Option<Selinux>,
}

impl OSModifierConfig {
    pub fn call_os_modifier(&self, os_modifier_path: &Path) -> Result<(), Error> {
        let os_modifier_config_yaml =
            serde_yaml::to_string(&self).context("Failed to serialize OS modifier config")?;

        if os_modifier_config_yaml.is_empty() {
            // Should never get here, but in case the OS modifier config is empty, return early
            // without calling binary
            warn!("OS modifier config is empty. OS modifier will not be called.");
            return Ok(());
        }

        debug!("Calling OS modifier");
        debug!(
            "Calling OS modifier with the following config:\n{}",
            os_modifier_config_yaml
        );
        let mut config_file = NamedTempFile::new().context("Failed to create a temporary file")?;
        config_file
            .write_all(os_modifier_config_yaml.as_bytes())
            .and_then(|_| config_file.flush())
            .context("Failed to write OS modifier config to temporary file and flush")?;
        osmodifier::run(os_modifier_path, config_file.path())
            .context("Failed to run OS modifier")?;
        Ok(())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MICServices {
    pub services: Services,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PasswordType {
    Locked,
    PlainText,
    Hashed,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MICPassword {
    #[serde(rename = "type")]
    pub password_type: PasswordType,
    pub value: String,
}

/// A helper struct to convert user into MIC's user format
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MICUser {
    pub name: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub uid: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<MICPassword>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_expires_days: Option<u64>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ssh_public_keys: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_group: Option<String>,

    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub secondary_groups: Vec<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_command: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub home_directory: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Overlay {
    pub lower_dir: String,
    pub upper_dir: String,
    pub work_dir: String,
    pub partition: IdentifiedPartition,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentifiedPartition {
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Verity {
    pub id: String,
    pub name: String,
    pub data_device: String,
    pub hash_device: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub corruption_option: Option<CorruptionOption>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Specifies the behavior in case of detected corruption.
pub enum CorruptionOption {
    /// Default setting. Fails the I/O operation with an I/O error.
    IoError,

    /// Ignores the corruption and continues operation.
    Ignore,

    /// Causes the system to panic. This will print errors and try restarting the system
    /// upon detecting corruption.
    Panic,

    /// Attempts to restart the system upon detecting corruption.
    Restart,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selinux: Option<Selinux>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<Overlay>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verity: Option<Verity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root_device: Option<String>,
}

pub fn run(os_modifier_path: &Path, config_file: &Path) -> Result<(), Error> {
    // Run the OS modifier with the configuration file
    Command::new(os_modifier_path)
        .arg("--config-file")
        .arg(config_file)
        .arg("--log-level=debug")
        .run_and_check()
        .context(format!(
            "Failed to run OS modifier with config file {}",
            config_file.display()
        ))?;

    Ok(())
}

pub fn update_grub(os_modifier_path: &Path) -> Result<(), Error> {
    Command::new(os_modifier_path.to_str().unwrap())
        .arg("--update-grub")
        .arg("--log-level=debug")
        .run_and_check()
        .context("Failed to run OS modifier to update GRUB config")
}
