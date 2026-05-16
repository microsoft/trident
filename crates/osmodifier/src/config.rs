// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configuration types for OS modifier operations.
//!
//! These types were originally in `osutils::osmodifier` and are the Rust
//! equivalents of the Go `osmodifierapi` types.

use serde::{Deserialize, Serialize};
use trident_api::config::{KernelCommandLine, Module, Selinux, Services};

/// OS modification configuration.
///
/// Covers users, hostname, modules, services, kernel command line, and SELinux.
#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OSModifierConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub users: Vec<MICUser>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<Module>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub services: Option<Services>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kernel_command_line: Option<KernelCommandLine>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selinux: Option<Selinux>,
}

/// Password type for user configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PasswordType {
    Locked,
    PlainText,
    Hashed,
}

/// User password configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MICPassword {
    #[serde(rename = "type")]
    pub password_type: PasswordType,
    pub value: String,
}

/// User configuration in the MIC (Microsoft Image Customizer) format.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MICUser {
    pub name: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<i32>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<MICPassword>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_expires_days: Option<u64>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ssh_public_keys: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_group: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secondary_groups: Vec<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub startup_command: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_directory: Option<String>,
}

/// Overlay filesystem configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Overlay {
    pub lower_dir: String,
    pub upper_dir: String,
    pub work_dir: String,
    pub partition: IdentifiedPartition,
}

/// A partition identified by an ID string.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IdentifiedPartition {
    pub id: String,
}

/// dm-verity configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Verity {
    pub id: String,
    pub name: String,
    pub data_device: String,
    pub hash_device: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub corruption_option: Option<CorruptionOption>,
}

/// Corruption handling behavior for dm-verity.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub enum CorruptionOption {
    IoError,
    Ignore,
    Panic,
    Restart,
}

/// Boot-specific configuration (overlays, verity, SELinux, root device).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BootConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selinux: Option<Selinux>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overlays: Vec<Overlay>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verity: Option<Verity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_device: Option<String>,
}
