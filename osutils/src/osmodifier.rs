use std::{path::Path, process::Command};

use anyhow::{Context, Error};
use serde::{Deserialize, Serialize};

use crate::exe::RunAndCheck;

#[derive(Serialize, Deserialize)]
pub struct MICHostname {
    pub hostname: String,
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
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MICUsers {
    pub users: Vec<MICUser>,
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
