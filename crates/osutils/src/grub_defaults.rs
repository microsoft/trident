// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Direct manipulation of `/etc/default/grub` and `grub2-mkconfig`.
//!
//! This module replaces the external `os-modifier` tool for GRUB configuration
//! updates, enabling Trident to manage boot configuration natively without
//! depending on `azurelinux-image-tools`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Error};
use log::{debug, info, trace};

use crate::exe::RunAndCheck;

/// Default path to the GRUB defaults file.
pub const DEFAULT_GRUB_PATH: &str = "/etc/default/grub";

/// Default path for grub2-mkconfig output.
pub const GRUB_CFG_PATH: &str = "/boot/grub2/grub.cfg";

/// Represents parsed content of `/etc/default/grub`.
///
/// Preserves the original file structure (comments, ordering, unknown keys)
/// while allowing targeted modifications to specific variables.
#[derive(Debug)]
pub struct GrubDefaults {
    /// Raw lines from the file, preserving comments and ordering.
    lines: Vec<GrubLine>,
    /// Path to the defaults file.
    path: PathBuf,
}

#[derive(Debug, Clone)]
enum GrubLine {
    /// A key=value assignment (possibly quoted).
    Assignment { key: String, value: String },
    /// A comment or blank line, preserved as-is.
    Other(String),
}

impl GrubDefaults {
    /// Read and parse `/etc/default/grub` (or a custom path).
    pub fn read(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read '{}'", path.display()))?;

        let lines = content
            .lines()
            .map(|line| {
                let trimmed = line.trim();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    GrubLine::Other(line.to_string())
                } else if let Some((key, value)) = trimmed.split_once('=') {
                    GrubLine::Assignment {
                        key: key.trim().to_string(),
                        value: value.trim().to_string(),
                    }
                } else {
                    GrubLine::Other(line.to_string())
                }
            })
            .collect();

        Ok(Self {
            lines,
            path: path.to_path_buf(),
        })
    }

    /// Read from the default path.
    pub fn read_default() -> Result<Self, Error> {
        Self::read(DEFAULT_GRUB_PATH)
    }

    /// Get the value of a variable (unquoted).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.lines.iter().find_map(|line| match line {
            GrubLine::Assignment { key: k, value } if k == key => Some(unquote(value)),
            _ => None,
        })
    }

    /// Set a variable's value. If the key exists, update it in place.
    /// If it doesn't exist, append it.
    pub fn set(&mut self, key: &str, value: &str) {
        let quoted_value = format!("\"{}\"", value);
        let mut found = false;
        for line in &mut self.lines {
            if let GrubLine::Assignment { key: k, value: v } = line {
                if k == key {
                    *v = quoted_value.clone();
                    found = true;
                    break;
                }
            }
        }
        if !found {
            self.lines.push(GrubLine::Assignment {
                key: key.to_string(),
                value: quoted_value,
            });
        }
    }

    /// Parse kernel command line args from GRUB_CMDLINE_LINUX.
    /// Returns a map of arg_name -> optional value.
    pub fn get_cmdline_args(&self) -> HashMap<String, Option<String>> {
        let mut args = HashMap::new();
        if let Some(cmdline) = self.get("GRUB_CMDLINE_LINUX") {
            for token in shell_split(cmdline) {
                if let Some((key, value)) = token.split_once('=') {
                    args.insert(key.to_string(), Some(value.to_string()));
                } else {
                    args.insert(token.to_string(), None);
                }
            }
        }
        args
    }

    /// Update specific kernel command line args in GRUB_CMDLINE_LINUX.
    ///
    /// `updates` maps arg names to new values. If the arg exists, its value
    /// is replaced. If it doesn't exist, it's appended. Args not in `updates`
    /// are preserved unchanged.
    pub fn update_cmdline_args(&mut self, updates: &[(&str, &str)]) {
        let current = self.get("GRUB_CMDLINE_LINUX").unwrap_or("").to_string();
        let mut tokens: Vec<String> = shell_split(&current);

        for (name, value) in updates {
            let prefix = format!("{}=", name);
            let new_token = format!("{}={}", name, value);

            let mut found = false;
            for token in &mut tokens {
                if token.starts_with(&prefix) || token == *name {
                    *token = new_token.clone();
                    found = true;
                    break;
                }
            }
            if !found {
                tokens.push(new_token);
            }
        }

        let new_cmdline = tokens.join(" ");
        self.set("GRUB_CMDLINE_LINUX", &new_cmdline);
    }

    /// Remove specific args from GRUB_CMDLINE_LINUX by name prefix.
    pub fn remove_cmdline_args(&mut self, names: &[&str]) {
        let current = self.get("GRUB_CMDLINE_LINUX").unwrap_or("").to_string();
        let tokens: Vec<String> = shell_split(&current)
            .into_iter()
            .filter(|token| {
                !names.iter().any(|name| {
                    token == *name || token.starts_with(&format!("{}=", name))
                })
            })
            .collect();

        let new_cmdline = tokens.join(" ");
        self.set("GRUB_CMDLINE_LINUX", &new_cmdline);
    }

    /// Write the modified defaults back to the file.
    pub fn write(&self) -> Result<(), Error> {
        self.write_to(&self.path)
    }

    /// Write to a specific path.
    pub fn write_to(&self, path: &Path) -> Result<(), Error> {
        let content: String = self
            .lines
            .iter()
            .map(|line| match line {
                GrubLine::Assignment { key, value } => format!("{}={}", key, value),
                GrubLine::Other(s) => s.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Ensure trailing newline
        let content = if content.ends_with('\n') {
            content
        } else {
            format!("{}\n", content)
        };

        fs::write(path, &content)
            .with_context(|| format!("Failed to write '{}'", path.display()))?;

        trace!("Wrote GRUB defaults to '{}'", path.display());
        Ok(())
    }
}

/// Run `grub2-mkconfig` to regenerate the GRUB configuration.
pub fn regenerate_grub_config(output_path: impl AsRef<Path>) -> Result<(), Error> {
    let output_path = output_path.as_ref();
    info!(
        "Regenerating GRUB config at '{}'",
        output_path.display()
    );

    std::process::Command::new("grub2-mkconfig")
        .arg("-o")
        .arg(output_path)
        .run_and_check()
        .with_context(|| {
            format!(
                "Failed to run grub2-mkconfig -o '{}'",
                output_path.display()
            )
        })?;

    debug!("GRUB config regenerated successfully");
    Ok(())
}

/// Parse kernel command line args from an existing grub.cfg file.
///
/// Finds the first non-recovery `linux` line and extracts its arguments.
/// This replicates what os-modifier's `extractValuesFromGrubConfig` does.
pub fn extract_cmdline_from_grub_cfg(grub_cfg_path: &Path) -> Result<HashMap<String, Option<String>>, Error> {
    let content = fs::read_to_string(grub_cfg_path)
        .with_context(|| format!("Failed to read '{}'", grub_cfg_path.display()))?;

    // Find the first `linux` or `linuxefi` line that isn't in a recovery menuentry
    let mut in_recovery = false;
    for line in content.lines() {
        let trimmed = line.trim();

        // Track if we're inside a recovery menuentry
        if trimmed.starts_with("menuentry") && trimmed.contains("recovery") {
            in_recovery = true;
        }
        if in_recovery && trimmed == "}" {
            in_recovery = false;
            continue;
        }
        if in_recovery {
            continue;
        }

        // Look for linux/linuxefi command
        if trimmed.starts_with("linux ") || trimmed.starts_with("linuxefi ") {
            let mut args = HashMap::new();
            // Skip the command and kernel path (first two tokens)
            let tokens: Vec<&str> = trimmed.split_whitespace().collect();
            for token in tokens.iter().skip(2) {
                if let Some((key, value)) = token.split_once('=') {
                    args.insert(key.to_string(), Some(value.to_string()));
                } else {
                    args.insert(token.to_string(), None);
                }
            }
            return Ok(args);
        }
    }

    bail!(
        "No non-recovery linux line found in '{}'",
        grub_cfg_path.display()
    )
}

/// Strip surrounding quotes from a string.
fn unquote(s: &str) -> &str {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

/// Simple space-delimited split (not full shell parsing, but sufficient for
/// GRUB_CMDLINE_LINUX values which don't contain nested quotes).
fn shell_split(s: &str) -> Vec<String> {
    s.split_whitespace().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp_grub(content: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(content.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn test_read_and_get() {
        let f = write_temp_grub(
            r#"# Comment line
GRUB_DEFAULT=0
GRUB_TIMEOUT=5
GRUB_CMDLINE_LINUX="rd.overlayfs=lower,upper,work,/dev/sda3 root=/dev/sda2 selinux=1"
GRUB_DISABLE_RECOVERY=true
"#,
        );

        let grub = GrubDefaults::read(f.path()).unwrap();
        assert_eq!(grub.get("GRUB_DEFAULT"), Some("0"));
        assert_eq!(grub.get("GRUB_TIMEOUT"), Some("5"));
        assert_eq!(
            grub.get("GRUB_CMDLINE_LINUX"),
            Some("rd.overlayfs=lower,upper,work,/dev/sda3 root=/dev/sda2 selinux=1")
        );
        assert_eq!(grub.get("GRUB_DISABLE_RECOVERY"), Some("true"));
        assert_eq!(grub.get("NONEXISTENT"), None);
    }

    #[test]
    fn test_get_cmdline_args() {
        let f = write_temp_grub(
            r#"GRUB_CMDLINE_LINUX="root=/dev/sda2 selinux=1 rd.overlayfs=a,b,c,d quiet"
"#,
        );

        let grub = GrubDefaults::read(f.path()).unwrap();
        let args = grub.get_cmdline_args();
        assert_eq!(args.get("root"), Some(&Some("/dev/sda2".to_string())));
        assert_eq!(args.get("selinux"), Some(&Some("1".to_string())));
        assert_eq!(args.get("rd.overlayfs"), Some(&Some("a,b,c,d".to_string())));
        assert_eq!(args.get("quiet"), Some(&None));
    }

    #[test]
    fn test_update_cmdline_args() {
        let f = write_temp_grub(
            r#"GRUB_CMDLINE_LINUX="root=/dev/sda2 selinux=1 quiet"
"#,
        );

        let mut grub = GrubDefaults::read(f.path()).unwrap();
        grub.update_cmdline_args(&[
            ("root", "/dev/sda5"),
            ("selinux", "0"),
            ("rd.overlayfs", "lower,upper,work,/dev/sda3"),
        ]);

        let args = grub.get_cmdline_args();
        assert_eq!(args.get("root"), Some(&Some("/dev/sda5".to_string())));
        assert_eq!(args.get("selinux"), Some(&Some("0".to_string())));
        assert_eq!(
            args.get("rd.overlayfs"),
            Some(&Some("lower,upper,work,/dev/sda3".to_string()))
        );
        // quiet should be preserved
        assert_eq!(args.get("quiet"), Some(&None));
    }

    #[test]
    fn test_remove_cmdline_args() {
        let f = write_temp_grub(
            r#"GRUB_CMDLINE_LINUX="root=/dev/sda2 selinux=1 quiet rd.overlayfs=a,b,c,d"
"#,
        );

        let mut grub = GrubDefaults::read(f.path()).unwrap();
        grub.remove_cmdline_args(&["selinux", "rd.overlayfs"]);

        let args = grub.get_cmdline_args();
        assert_eq!(args.get("selinux"), None);
        assert_eq!(args.get("rd.overlayfs"), None);
        assert_eq!(args.get("root"), Some(&Some("/dev/sda2".to_string())));
        assert_eq!(args.get("quiet"), Some(&None));
    }

    #[test]
    fn test_write_preserves_structure() {
        let original = r#"# This is a comment
GRUB_DEFAULT=0
GRUB_TIMEOUT=5

# Another comment
GRUB_CMDLINE_LINUX="root=/dev/sda2"
"#;
        let f = write_temp_grub(original);
        let mut grub = GrubDefaults::read(f.path()).unwrap();
        grub.set("GRUB_TIMEOUT", "10");

        let out = NamedTempFile::new().unwrap();
        grub.write_to(out.path()).unwrap();
        let written = fs::read_to_string(out.path()).unwrap();

        assert!(written.contains("# This is a comment"));
        assert!(written.contains("# Another comment"));
        assert!(written.contains("GRUB_TIMEOUT=\"10\""));
        assert!(written.contains("GRUB_DEFAULT=0"));
    }

    #[test]
    fn test_extract_cmdline_from_grub_cfg() {
        let cfg = r#"
menuentry 'Azure Linux' {
    linux /vmlinuz-6.6.51 root=/dev/sda2 selinux=1 rd.overlayfs=lower,upper,work,/dev/sda3 quiet
    initrd /initramfs-6.6.51.img
}
menuentry 'Azure Linux (recovery)' {
    linux /vmlinuz-6.6.51 root=/dev/sda2 single
    initrd /initramfs-6.6.51.img
}
"#;
        let f = write_temp_grub(cfg);
        let args = extract_cmdline_from_grub_cfg(f.path()).unwrap();
        assert_eq!(args.get("root"), Some(&Some("/dev/sda2".to_string())));
        assert_eq!(args.get("selinux"), Some(&Some("1".to_string())));
        assert_eq!(args.get("quiet"), Some(&None));
        // Should NOT pick up the recovery "single" arg
        assert_eq!(args.get("single"), None);
    }

    #[test]
    fn test_set_new_key() {
        let f = write_temp_grub("GRUB_DEFAULT=0\n");
        let mut grub = GrubDefaults::read(f.path()).unwrap();
        grub.set("GRUB_HIDDEN_TIMEOUT", "0");

        let out = NamedTempFile::new().unwrap();
        grub.write_to(out.path()).unwrap();
        let written = fs::read_to_string(out.path()).unwrap();
        assert!(written.contains("GRUB_HIDDEN_TIMEOUT=\"0\""));
    }
}
