// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Configuration types for OS modifier operations.
//!
//! These types were originally in `osutils::osmodifier` and are the Rust
//! equivalents of the Go `osmodifierapi` types.

use std::collections::HashSet;
use std::net::IpAddr;

use anyhow::bail;
use serde::{Deserialize, Serialize};
use trident_api::config::{KernelCommandLine, Module, Selinux, Services};

/// OS modification configuration.
///
/// Covers users, hostname, modules, services, kernel command line, and SELinux.
#[derive(Serialize, Deserialize, Default, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

impl OSModifierConfig {
    /// Validate the configuration, matching the Go `osmodifierapi.OS.IsValid()`
    /// checks for hostname format.
    ///
    /// Hostname validation mirrors `govalidator.IsDNSName()` with an additional
    /// underscore rejection (Go: `!IsDNSName(h) || strings.Contains(h, "_")`).
    pub fn validate(&self) -> Result<(), anyhow::Error> {
        if let Some(ref hostname) = self.hostname {
            if !hostname.is_empty() {
                validate_hostname(hostname)?;
            }
        }
        Ok(())
    }
}

/// Password type for user configuration.
///
/// Go has 5 variants (PlainText, Hashed, PlainTextFile, HashedFile, plus
/// locked-via-empty). This crate only needs 3 because trident passes
/// passwords via the API config, never as file paths. File-path variants
/// are not supported.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PasswordType {
    Locked,
    PlainText,
    Hashed,
}

/// User password configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MICPassword {
    #[serde(rename = "type")]
    pub password_type: PasswordType,
    pub value: String,
}

/// User configuration in the MIC (Microsoft Image Customizer) format.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

impl BootConfig {
    /// Validate the boot configuration, matching the Go
    /// `osmodifierapi.OS.IsValid()` checks for duplicate overlay paths.
    ///
    /// Each overlay's `upper_dir` and `work_dir` must be unique across all
    /// overlays to avoid filesystem conflicts at mount time.
    pub fn validate(&self) -> Result<(), anyhow::Error> {
        let mut upper_dirs = HashSet::new();
        let mut work_dirs = HashSet::new();

        for (i, overlay) in self.overlays.iter().enumerate() {
            if !upper_dirs.insert(&overlay.upper_dir) {
                bail!(
                    "duplicate upperDir ({}) found in overlay at index {}",
                    overlay.upper_dir,
                    i
                );
            }
            if !work_dirs.insert(&overlay.work_dir) {
                bail!(
                    "duplicate workDir ({}) found in overlay at index {}",
                    overlay.work_dir,
                    i
                );
            }
        }

        Ok(())
    }
}

/// Validate a hostname, matching Go's `govalidator.IsDNSName()` plus the
/// additional underscore rejection from `osmodifierapi.OS.IsValid()`.
///
/// Rules (ported from Go `govalidator` v11):
/// - Must not be empty
/// - Total length excluding dots must not exceed 255
/// - Must not parse as an IP address (v4 or v6)
/// - Each dot-separated label: 1–63 characters, `[a-zA-Z0-9-]` only
/// - Labels must not start or end with a hyphen
/// - No underscores (Go: `strings.Contains(hostname, "_")`)
/// - A single trailing dot is permitted (Go regex allows it)
fn validate_hostname(hostname: &str) -> Result<(), anyhow::Error> {
    if hostname.is_empty() {
        bail!("invalid hostname (empty)");
    }

    // Go: `strings.Contains(s.Hostname, "_")`
    if hostname.contains('_') {
        bail!("invalid hostname ({hostname})");
    }

    // Go: `!IsIP(str)` — reject bare IP addresses.
    if hostname.parse::<IpAddr>().is_ok() {
        bail!("invalid hostname ({hostname})");
    }

    // Go: `len(strings.Replace(str, ".", "", -1)) > 255`
    let len_without_dots = hostname.chars().filter(|&c| c != '.').count();
    if len_without_dots > 255 {
        bail!("invalid hostname ({hostname})");
    }

    // Strip a single trailing dot (Go regex: `[\._]?$`).
    let trimmed = hostname.strip_suffix('.').unwrap_or(hostname);

    if trimmed.is_empty() {
        bail!("invalid hostname ({hostname})");
    }

    // Validate each label.
    for label in trimmed.split('.') {
        if label.is_empty() || label.len() > 63 {
            bail!("invalid hostname ({hostname})");
        }
        if label.starts_with('-') || label.ends_with('-') {
            bail!("invalid hostname ({hostname})");
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            bail!("invalid hostname ({hostname})");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------
    // Hostname validation
    // ---------------------------------------------------------------

    #[test]
    fn hostname_valid_simple() {
        validate_hostname("my-host").unwrap();
    }

    #[test]
    fn hostname_valid_fqdn() {
        validate_hostname("host.example.com").unwrap();
    }

    #[test]
    fn hostname_valid_trailing_dot() {
        validate_hostname("host.example.com.").unwrap();
    }

    #[test]
    fn hostname_valid_single_char() {
        validate_hostname("a").unwrap();
    }

    #[test]
    fn hostname_valid_numeric_label() {
        validate_hostname("host1.2nd.example").unwrap();
    }

    #[test]
    fn hostname_invalid_underscore() {
        assert!(validate_hostname("host_name").is_err());
    }

    #[test]
    fn hostname_invalid_leading_hyphen() {
        assert!(validate_hostname("-host").is_err());
    }

    #[test]
    fn hostname_invalid_trailing_hyphen() {
        assert!(validate_hostname("host-").is_err());
    }

    #[test]
    fn hostname_invalid_empty_label() {
        assert!(validate_hostname("host..example").is_err());
    }

    #[test]
    fn hostname_invalid_ipv4() {
        assert!(validate_hostname("192.168.1.1").is_err());
    }

    #[test]
    fn hostname_invalid_ipv6() {
        assert!(validate_hostname("::1").is_err());
    }

    #[test]
    fn hostname_invalid_empty() {
        assert!(validate_hostname("").is_err());
    }

    #[test]
    fn hostname_invalid_just_dot() {
        assert!(validate_hostname(".").is_err());
    }

    #[test]
    fn hostname_invalid_special_chars() {
        assert!(validate_hostname("host!name").is_err());
    }

    #[test]
    fn hostname_invalid_label_too_long() {
        let long_label = "a".repeat(64);
        assert!(validate_hostname(&long_label).is_err());
    }

    #[test]
    fn hostname_valid_label_max_length() {
        let label = "a".repeat(63);
        validate_hostname(&label).unwrap();
    }

    #[test]
    fn hostname_invalid_total_too_long() {
        // 256 chars without dots exceeds the 255 limit
        let name = format!("{}.{}", "a".repeat(63), "b".repeat(193));
        assert!(validate_hostname(&name).is_err());
    }

    // ---------------------------------------------------------------
    // OSModifierConfig.validate()
    // ---------------------------------------------------------------

    #[test]
    fn os_config_validate_no_hostname() {
        let config = OSModifierConfig::default();
        config.validate().unwrap();
    }

    #[test]
    fn os_config_validate_empty_hostname() {
        let config = OSModifierConfig {
            hostname: Some(String::new()),
            ..Default::default()
        };
        config.validate().unwrap();
    }

    #[test]
    fn os_config_validate_valid_hostname() {
        let config = OSModifierConfig {
            hostname: Some("my-host".to_string()),
            ..Default::default()
        };
        config.validate().unwrap();
    }

    #[test]
    fn os_config_validate_invalid_hostname() {
        let config = OSModifierConfig {
            hostname: Some("host_name".to_string()),
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    // ---------------------------------------------------------------
    // BootConfig.validate() — overlay duplicate checks
    // ---------------------------------------------------------------

    fn make_overlay(lower: &str, upper: &str, work: &str) -> Overlay {
        Overlay {
            lower_dir: lower.to_string(),
            upper_dir: upper.to_string(),
            work_dir: work.to_string(),
            partition: IdentifiedPartition {
                id: "part-1".to_string(),
            },
        }
    }

    #[test]
    fn boot_config_validate_no_overlays() {
        let config = BootConfig {
            selinux: None,
            overlays: vec![],
            verity: None,
            root_device: None,
        };
        config.validate().unwrap();
    }

    #[test]
    fn boot_config_validate_unique_overlays() {
        let config = BootConfig {
            selinux: None,
            overlays: vec![
                make_overlay("/etc", "/upper/a", "/work/a"),
                make_overlay("/var", "/upper/b", "/work/b"),
            ],
            verity: None,
            root_device: None,
        };
        config.validate().unwrap();
    }

    #[test]
    fn boot_config_validate_duplicate_upper_dir() {
        let config = BootConfig {
            selinux: None,
            overlays: vec![
                make_overlay("/etc", "/upper/dup", "/work/a"),
                make_overlay("/var", "/upper/dup", "/work/b"),
            ],
            verity: None,
            root_device: None,
        };
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate upperDir"), "got: {err}");
        assert!(err.contains("/upper/dup"), "got: {err}");
        assert!(err.contains("index 1"), "got: {err}");
    }

    #[test]
    fn boot_config_validate_duplicate_work_dir() {
        let config = BootConfig {
            selinux: None,
            overlays: vec![
                make_overlay("/etc", "/upper/a", "/work/dup"),
                make_overlay("/var", "/upper/b", "/work/dup"),
            ],
            verity: None,
            root_device: None,
        };
        let err = config.validate().unwrap_err().to_string();
        assert!(err.contains("duplicate workDir"), "got: {err}");
        assert!(err.contains("/work/dup"), "got: {err}");
    }

    #[test]
    fn boot_config_validate_upper_and_work_can_match_across_fields() {
        // Go does NOT check upper_dir vs work_dir cross-field — only within
        // their own sets. So upper_dir == work_dir of a different overlay is OK.
        let config = BootConfig {
            selinux: None,
            overlays: vec![
                make_overlay("/etc", "/shared/path", "/work/a"),
                make_overlay("/var", "/upper/b", "/shared/path"),
            ],
            verity: None,
            root_device: None,
        };
        config.validate().unwrap();
    }
}
