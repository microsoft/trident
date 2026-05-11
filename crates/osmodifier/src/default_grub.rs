// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! /etc/default/grub parser and writer.
//!
//! Parses the shell-variable format used by GRUB's default configuration file.
//! Supports reading, modifying, and writing back while preserving comments and
//! ordering.

use std::{fs, path::PathBuf};

use anyhow::{Context, Error};
use log::{debug, trace};

use crate::OsModifierContext;

const DEFAULT_GRUB_PATH: &str = "/etc/default/grub";

/// Represents a parsed /etc/default/grub file.
pub struct DefaultGrub {
    /// Original lines of the file, with modifications applied in-place.
    lines: Vec<String>,
    /// Path to the file on disk.
    path: PathBuf,
}

impl DefaultGrub {
    /// Read and parse /etc/default/grub.
    pub fn read(ctx: &OsModifierContext) -> Result<Self, Error> {
        let path = ctx.path(DEFAULT_GRUB_PATH);
        debug!("Reading default grub from '{}'", path.display());

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read '{}'", path.display()))?;

        trace!("Default grub content:\n{content}");

        let lines = content.lines().map(String::from).collect();
        Ok(Self { lines, path })
    }

    /// Write the (possibly modified) config back to disk.
    pub fn write(&self) -> Result<(), Error> {
        let mut content = self.lines.join("\n");
        content.push('\n');

        debug!("Writing default grub to '{}'", self.path.display());
        trace!("Default grub content to write:\n{content}");

        fs::write(&self.path, &content)
            .with_context(|| format!("Failed to write '{}'", self.path.display()))
    }

    /// Get the value of a variable (e.g., "GRUB_CMDLINE_LINUX").
    /// Returns the unquoted value.
    pub fn get_variable(&self, name: &str) -> Option<String> {
        let prefix = format!("{name}=");
        for line in &self.lines {
            let trimmed = line.trim();
            if trimmed.starts_with(&prefix) {
                let value = &trimmed[prefix.len()..];
                return Some(unquote(value));
            }
        }
        None
    }

    /// Set a variable value. If the variable exists, update it in place.
    /// If not, append it.
    pub fn set_variable(&mut self, name: &str, value: &str) {
        let prefix = format!("{name}=");
        let new_line = format!("{name}=\"{value}\"");

        for line in &mut self.lines {
            if line.trim().starts_with(&prefix) {
                *line = new_line;
                return;
            }
        }

        // Not found — append
        self.lines.push(new_line);
    }

    /// Update kernel command line args in GRUB_CMDLINE_LINUX.
    ///
    /// `old_keys` specifies which arg names to remove (matched by prefix
    /// before `=`). `new_args` are the replacement args to insert.
    ///
    /// This matches the Go `UpdateKernelCommandLineArgs` behavior.
    pub fn update_cmdline_args(
        &mut self,
        old_keys: &[&str],
        new_args: &[String],
    ) -> Result<(), Error> {
        let current = self.get_variable("GRUB_CMDLINE_LINUX").unwrap_or_default();

        let mut args: Vec<String> = current
            .split_whitespace()
            .filter(|arg| {
                let arg_name = arg.split('=').next().unwrap_or(arg);
                !old_keys.contains(&arg_name)
            })
            .map(String::from)
            .collect();

        args.extend(new_args.iter().cloned());

        let new_value = args.join(" ");
        self.set_variable("GRUB_CMDLINE_LINUX", &new_value);

        Ok(())
    }

    /// Add extra command line arguments to GRUB_CMDLINE_LINUX without
    /// removing any existing ones.
    pub fn add_extra_cmdline(&mut self, extra: &[String]) {
        let current = self.get_variable("GRUB_CMDLINE_LINUX").unwrap_or_default();

        let mut args: Vec<String> = if current.is_empty() {
            Vec::new()
        } else {
            current.split_whitespace().map(String::from).collect()
        };

        args.extend(extra.iter().cloned());

        let new_value = args.join(" ");
        self.set_variable("GRUB_CMDLINE_LINUX", &new_value);
    }
}

/// Remove surrounding quotes (single or double) from a value string.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Add extra kernel command line args to /etc/default/grub.
pub fn add_extra_cmdline(ctx: &OsModifierContext, extra: &[String]) -> Result<(), Error> {
    let mut grub = DefaultGrub::read(ctx)?;
    grub.add_extra_cmdline(extra);
    grub.write()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_unquote() {
        assert_eq!(unquote(r#""hello world""#), "hello world");
        assert_eq!(unquote("'hello'"), "hello");
        assert_eq!(unquote("noquotes"), "noquotes");
        assert_eq!(unquote(""), "");
    }

    #[test]
    fn test_get_set_variable() {
        let mut grub = DefaultGrub {
            lines: vec![
                "# Comment".to_string(),
                r#"GRUB_CMDLINE_LINUX="selinux=1 enforcing=1""#.to_string(),
                r#"GRUB_DEVICE="/dev/sda2""#.to_string(),
            ],
            path: PathBuf::from("/etc/default/grub"),
        };

        assert_eq!(
            grub.get_variable("GRUB_CMDLINE_LINUX"),
            Some("selinux=1 enforcing=1".to_string())
        );
        assert_eq!(
            grub.get_variable("GRUB_DEVICE"),
            Some("/dev/sda2".to_string())
        );
        assert_eq!(grub.get_variable("NONEXISTENT"), None);

        grub.set_variable("GRUB_DEVICE", "/dev/sdb1");
        assert_eq!(
            grub.get_variable("GRUB_DEVICE"),
            Some("/dev/sdb1".to_string())
        );

        grub.set_variable("NEW_VAR", "new_value");
        assert_eq!(grub.get_variable("NEW_VAR"), Some("new_value".to_string()));
    }

    #[test]
    fn test_update_cmdline_args() {
        let mut grub = DefaultGrub {
            lines: vec![
                r#"GRUB_CMDLINE_LINUX="quiet selinux=1 enforcing=1 rd.overlayfs=old""#.to_string(),
            ],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.update_cmdline_args(&["selinux", "enforcing"], &["selinux=0".to_string()])
            .unwrap();

        let result = grub.get_variable("GRUB_CMDLINE_LINUX").unwrap();
        assert!(result.contains("quiet"));
        assert!(result.contains("rd.overlayfs=old"));
        assert!(result.contains("selinux=0"));
        assert!(!result.contains("enforcing=1"));
        assert!(!result.contains("selinux=1"));
    }
}
