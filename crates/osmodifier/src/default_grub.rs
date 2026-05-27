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

    /// Set a variable value. If the variable exists, update it in place,
    /// preserving the original line's leading whitespace and quote style.
    /// If not, append it with double quotes (the GRUB default).
    pub fn set_variable(&mut self, name: &str, value: &str) {
        let prefix = format!("{name}=");

        for line in &mut self.lines {
            if line.trim().starts_with(&prefix) {
                let leading_ws = &line[..line.len() - line.trim_start().len()];
                let after_eq = line.trim().strip_prefix(&prefix).unwrap_or("");
                let quote = detect_quote_char(after_eq);
                *line = format!("{leading_ws}{name}={quote}{value}{quote}");
                return;
            }
        }

        // Not found — append with double quotes (GRUB convention)
        self.lines.push(format!("{name}=\"{value}\""));
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

    /// Add extra command line arguments to GRUB_CMDLINE_LINUX_DEFAULT.
    ///
    /// Appends args to the `_DEFAULT` variable (not `GRUB_CMDLINE_LINUX`) so
    /// they apply only to the default boot entry, not recovery entries.
    /// This matches Go's `addExtraCommandLineToDefaultGrubFile` behavior.
    ///
    /// Args are appended without dedup, matching Go's text-insert approach.
    /// GRUB evaluates later args after earlier ones, so intentional overrides
    /// (e.g., an extra `foo=new` overriding an existing `foo=old`) work.
    pub fn add_extra_cmdline(&mut self, extra: &[String]) {
        if extra.is_empty() {
            return;
        }

        let current = self
            .get_variable("GRUB_CMDLINE_LINUX_DEFAULT")
            .unwrap_or_default();

        let mut args: Vec<String> = if current.is_empty() {
            Vec::new()
        } else {
            current.split_whitespace().map(String::from).collect()
        };

        args.extend(extra.iter().cloned());

        let new_value = args.join(" ");
        self.set_variable("GRUB_CMDLINE_LINUX_DEFAULT", &new_value);
    }
}

/// Detect the quote character used around a value string.
/// Checks only the leading character so that trailing content (e.g., an
/// inline `# comment`) does not prevent single-quote detection.
/// Returns `'` for single-quoted, `"` for everything else (the GRUB default).
fn detect_quote_char(s: &str) -> char {
    match s.trim_start().chars().next() {
        Some('\'') => '\'',
        _ => '"',
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

    #[test]
    fn test_update_cmdline_args_empty_initial() {
        let mut grub = DefaultGrub {
            lines: vec![r#"GRUB_CMDLINE_LINUX="""#.to_string()],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.update_cmdline_args(&[], &["selinux=1".to_string()])
            .unwrap();

        let result = grub.get_variable("GRUB_CMDLINE_LINUX").unwrap();
        assert_eq!(result, "selinux=1");
    }

    #[test]
    fn test_update_cmdline_args_missing_variable() {
        // If GRUB_CMDLINE_LINUX doesn't exist, it should be created
        let mut grub = DefaultGrub {
            lines: vec!["GRUB_TIMEOUT=0".to_string()],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.update_cmdline_args(&[], &["selinux=1".to_string()])
            .unwrap();

        let result = grub.get_variable("GRUB_CMDLINE_LINUX").unwrap();
        assert_eq!(result, "selinux=1");
        // Original variable should be preserved
        assert_eq!(grub.get_variable("GRUB_TIMEOUT"), Some("0".to_string()));
    }

    #[test]
    fn test_add_extra_cmdline_basic() {
        let mut grub = DefaultGrub {
            lines: vec![r#"GRUB_CMDLINE_LINUX="quiet""#.to_string()],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.add_extra_cmdline(&["console=tty0".to_string(), "loglevel=3".to_string()]);

        // Should write to GRUB_CMDLINE_LINUX_DEFAULT, not GRUB_CMDLINE_LINUX
        let result = grub.get_variable("GRUB_CMDLINE_LINUX_DEFAULT").unwrap();
        assert!(result.contains("console=tty0"));
        assert!(result.contains("loglevel=3"));
        // GRUB_CMDLINE_LINUX should be unchanged
        assert_eq!(
            grub.get_variable("GRUB_CMDLINE_LINUX"),
            Some("quiet".to_string())
        );
    }

    #[test]
    fn test_add_extra_cmdline_appends_to_existing_default() {
        let mut grub = DefaultGrub {
            lines: vec![
                r#"GRUB_CMDLINE_LINUX="quiet""#.to_string(),
                r#"GRUB_CMDLINE_LINUX_DEFAULT="rd.auto=1""#.to_string(),
            ],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.add_extra_cmdline(&["console=tty0".to_string()]);

        let result = grub.get_variable("GRUB_CMDLINE_LINUX_DEFAULT").unwrap();
        assert!(result.contains("rd.auto=1"), "Existing args preserved");
        assert!(result.contains("console=tty0"), "New arg appended");
    }

    #[test]
    fn test_add_extra_cmdline_no_dedup() {
        // Go does not dedup — intentional overrides must be allowed
        let mut grub = DefaultGrub {
            lines: vec![r#"GRUB_CMDLINE_LINUX_DEFAULT="selinux=1""#.to_string()],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.add_extra_cmdline(&["selinux=0".to_string()]);

        let result = grub.get_variable("GRUB_CMDLINE_LINUX_DEFAULT").unwrap();
        assert!(
            result.contains("selinux=0"),
            "Override arg should be appended"
        );
    }

    #[test]
    fn test_add_extra_cmdline_empty_initial() {
        let mut grub = DefaultGrub {
            lines: vec![],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.add_extra_cmdline(&["console=tty0".to_string()]);

        let result = grub.get_variable("GRUB_CMDLINE_LINUX_DEFAULT").unwrap();
        assert_eq!(result, "console=tty0");
    }

    #[test]
    fn test_comments_preserved() {
        let mut grub = DefaultGrub {
            lines: vec![
                "# This is a comment".to_string(),
                r#"GRUB_TIMEOUT=5"#.to_string(),
                "# Another comment".to_string(),
                r#"GRUB_CMDLINE_LINUX="quiet""#.to_string(),
            ],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.set_variable("GRUB_TIMEOUT", "0");

        assert_eq!(grub.lines[0], "# This is a comment");
        assert_eq!(grub.lines[2], "# Another comment");
        assert_eq!(grub.get_variable("GRUB_TIMEOUT"), Some("0".to_string()));
    }

    #[test]
    fn test_write_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let grub_path = tmp.path().join("etc/default");
        fs::create_dir_all(&grub_path).unwrap();
        fs::write(
            grub_path.join("grub"),
            "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX=\"quiet selinux=1\"\n",
        )
        .unwrap();

        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        // Read, modify, write
        let mut grub = DefaultGrub::read(&ctx).unwrap();
        grub.set_variable("GRUB_TIMEOUT", "0");
        grub.write().unwrap();

        // Read again and verify
        let grub2 = DefaultGrub::read(&ctx).unwrap();
        assert_eq!(grub2.get_variable("GRUB_TIMEOUT"), Some("0".to_string()));
        assert_eq!(
            grub2.get_variable("GRUB_CMDLINE_LINUX"),
            Some("quiet selinux=1".to_string())
        );
    }

    #[test]
    fn test_single_quoted_value() {
        let grub = DefaultGrub {
            lines: vec!["GRUB_DEVICE='/dev/sda1'".to_string()],
            path: PathBuf::from("/etc/default/grub"),
        };

        assert_eq!(
            grub.get_variable("GRUB_DEVICE"),
            Some("/dev/sda1".to_string())
        );
    }

    #[test]
    fn test_detect_quote_char() {
        assert_eq!(detect_quote_char(r#""hello""#), '"');
        assert_eq!(detect_quote_char("'hello'"), '\'');
        assert_eq!(detect_quote_char("noquotes"), '"');
        assert_eq!(detect_quote_char(""), '"');
    }

    #[test]
    fn test_set_variable_preserves_single_quotes() {
        let mut grub = DefaultGrub {
            lines: vec!["GRUB_DEVICE='/dev/sda1'".to_string()],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.set_variable("GRUB_DEVICE", "/dev/sdb1");
        assert_eq!(grub.lines[0], "GRUB_DEVICE='/dev/sdb1'");
        assert_eq!(
            grub.get_variable("GRUB_DEVICE"),
            Some("/dev/sdb1".to_string())
        );
    }

    #[test]
    fn test_set_variable_preserves_leading_whitespace() {
        let mut grub = DefaultGrub {
            lines: vec![
                "  GRUB_TIMEOUT=5".to_string(),
                "\tGRUB_DEVICE=\"/dev/sda1\"".to_string(),
            ],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.set_variable("GRUB_TIMEOUT", "0");
        assert_eq!(grub.lines[0], "  GRUB_TIMEOUT=\"0\"");

        grub.set_variable("GRUB_DEVICE", "/dev/sdb1");
        assert_eq!(grub.lines[1], "\tGRUB_DEVICE=\"/dev/sdb1\"");
    }

    #[test]
    fn test_set_variable_preserves_both_indent_and_quotes() {
        let mut grub = DefaultGrub {
            lines: vec!["    GRUB_DEVICE='/dev/sda1'".to_string()],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.set_variable("GRUB_DEVICE", "/dev/sdb1");
        assert_eq!(grub.lines[0], "    GRUB_DEVICE='/dev/sdb1'");
    }

    #[test]
    fn test_detect_quote_char_with_trailing_content() {
        // Inline comment after single-quoted value should still detect single quote
        assert_eq!(detect_quote_char("'value' # comment"), '\'');
        assert_eq!(detect_quote_char("\"value\" # comment"), '"');
    }

    #[test]
    fn test_set_variable_append_uses_double_quotes() {
        let mut grub = DefaultGrub {
            lines: vec!["# empty config".to_string()],
            path: PathBuf::from("/etc/default/grub"),
        };

        grub.set_variable("NEW_VAR", "new_value");
        assert_eq!(grub.lines[1], r#"NEW_VAR="new_value""#);
    }

    #[test]
    fn test_real_world_azl3_default_grub() {
        // Modeled after the AZL 3.0 /etc/default/grub
        let grub = DefaultGrub {
            lines: vec![
                r#"GRUB_TIMEOUT=0"#.to_string(),
                r#"GRUB_DISTRIBUTOR="AzureLinux""#.to_string(),
                r#"GRUB_DISABLE_SUBMENU=y"#.to_string(),
                r#"GRUB_TERMINAL_OUTPUT="console""#.to_string(),
                r#"GRUB_CMDLINE_LINUX="      rd.auto=1 net.ifnames=0 lockdown=integrity ""#
                    .to_string(),
                r#"GRUB_CMDLINE_LINUX_DEFAULT=" $kernelopts""#.to_string(),
            ],
            path: PathBuf::from("/etc/default/grub"),
        };

        let cmdline = grub.get_variable("GRUB_CMDLINE_LINUX").unwrap();
        assert!(cmdline.contains("rd.auto=1"));
        assert!(cmdline.contains("lockdown=integrity"));

        assert_eq!(
            grub.get_variable("GRUB_DISTRIBUTOR"),
            Some("AzureLinux".to_string())
        );
    }
}
