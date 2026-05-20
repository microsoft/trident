// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! grub.cfg parsing and grub2-mkconfig execution.
//!
//! Used by the `update_default_grub` flow to extract boot args from the
//! generated grub.cfg and sync them back to /etc/default/grub.
//!
//! The parsing logic mirrors the Go implementation's `FindNonRecoveryLinuxLine`
//! and `ParseCommandLineArgs` from imagecustomizerlib/grubcfgutils.go.
//! The Go code uses a full grub tokenizer; this port uses simpler string-based
//! parsing that matches the behavior for grub2-mkconfig-generated output.

use std::fs;

use anyhow::{bail, Context, Error};
use log::{debug, info, trace};
use osutils::dependencies::Dependency;

use crate::OsModifierContext;

/// Possible grub.cfg locations, tried in order.
const GRUB_CFG_PATHS: &[&str] = &["/boot/grub2/grub.cfg", "/boot/grub/grub.cfg"];

/// The grub.cfg args we want to extract for syncing to /etc/default/grub.
const SYNC_ARG_NAMES: &[&str] = &["rd.overlayfs", "roothash", "root", "selinux", "enforcing"];

/// Extract boot arguments from the generated grub.cfg.
///
/// Returns a tuple of (args_to_sync, optional_root_device).
/// `args_to_sync` contains entries like `["selinux=1", "rd.overlayfs=..."]`.
/// `root_device` is extracted separately because it maps to GRUB_DEVICE
/// rather than GRUB_CMDLINE_LINUX.
///
/// Mirrors Go `extractValuesFromGrubConfig` in modifydefaultgrub.go.
pub fn extract_boot_args_from_grub_cfg(
    ctx: &OsModifierContext,
) -> Result<(Vec<String>, Option<String>), Error> {
    let grub_cfg_path = find_grub_cfg(ctx)?;
    let content = fs::read_to_string(&grub_cfg_path)
        .with_context(|| format!("Failed to read '{}'", grub_cfg_path.display()))?;

    trace!("grub.cfg content:\n{content}");

    // Find the non-recovery linux command lines.
    // Go expects exactly one; error otherwise.
    let linux_lines = find_non_recovery_linux_lines(&content)?;
    if linux_lines.len() != 1 {
        bail!(
            "expected 1 non-recovery linux line, found {}",
            linux_lines.len()
        );
    }
    let linux_line = &linux_lines[0];
    debug!("Found linux line: {linux_line}");

    // Parse args from the linux line (skip first token which is the kernel path).
    let args_str = linux_line
        .split_whitespace()
        .skip(1) // skip kernel path (e.g., /boot/vmlinuz)
        .collect::<Vec<_>>();

    let mut values = Vec::new();
    let mut root_device = None;

    for token in &args_str {
        let (name, value) = match token.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (*token, None),
        };

        if SYNC_ARG_NAMES.contains(&name) {
            if let Some(v) = value {
                if name == "root" {
                    root_device = Some(v.to_string());
                } else {
                    values.push(format!("{name}={v}"));
                }
            }
        }
    }

    Ok((values, root_device))
}

/// Find the grub.cfg file on the filesystem.
fn find_grub_cfg(ctx: &OsModifierContext) -> Result<std::path::PathBuf, Error> {
    for path in GRUB_CFG_PATHS {
        let full = ctx.path(path);
        if full.exists() {
            return Ok(full);
        }
    }
    bail!("Could not find grub.cfg at any of: {:?}", GRUB_CFG_PATHS)
}

/// Return the first whitespace-delimited word from a line, or None if the
/// line is empty / whitespace-only.
fn first_word(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

/// Find the linux command lines from non-recovery menuentry blocks in grub.cfg.
///
/// Mirrors Go `FindNonRecoveryLinuxLine` in grubcfgutils.go:
/// - Iterates tokenized lines looking for `menuentry` keyword as first token.
/// - Checks the second token (title) for "recovery" (case-sensitive, matching Go).
/// - Collects `linux` lines from non-recovery menuentries.
/// - Returns all matches; caller decides whether to require exactly one.
fn find_non_recovery_linux_lines(content: &str) -> Result<Vec<String>, Error> {
    let mut in_menuentry = false;
    let mut is_recovery = false;
    let mut linux_lines = Vec::new();

    for line in content.lines() {
        let keyword = match first_word(line) {
            Some(w) => w,
            None => continue,
        };

        if keyword == "menuentry" {
            in_menuentry = true;
            // Go checks: strings.Contains(line.Tokens[1].RawContent, "recovery")
            // The second token is the title string (including quotes).
            // We check the rest of the line after "menuentry" for "recovery".
            let after_keyword = line[line.find("menuentry").unwrap() + "menuentry".len()..].trim();
            is_recovery = after_keyword.contains("recovery");

            if is_recovery {
                in_menuentry = false;
            }
        } else if in_menuentry && keyword == "linux" {
            // Capture everything after the "linux" keyword.
            let after_linux = line[line.find("linux").unwrap() + "linux".len()..].trim();
            if !after_linux.is_empty() {
                linux_lines.push(after_linux.to_string());
            }
        }
    }

    if linux_lines.is_empty() {
        bail!("no linux line found in non-recovery menuentry");
    }

    Ok(linux_lines)
}

/// Run grub2-mkconfig to regenerate the GRUB configuration.
pub fn run_grub_mkconfig(ctx: &OsModifierContext) -> Result<(), Error> {
    let grub_cfg_path = find_grub_cfg(ctx)?;

    info!("Running grub2-mkconfig -o '{}'", grub_cfg_path.display());

    Dependency::Grub2Mkconfig
        .cmd()
        .arg("-o")
        .arg(&grub_cfg_path)
        .run_and_check()
        .context("Failed to execute grub2-mkconfig")?;

    debug!("grub2-mkconfig completed successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_non_recovery_linux_lines() {
        let grub_cfg = indoc::indoc! {r#"
            set timeout=5
            menuentry 'Azure Linux' --class azurelinux {
                linux /boot/vmlinuz root=/dev/sda2 selinux=1 enforcing=1 rd.overlayfs=/a,/b,/c,/dev/sda3
                initrd /boot/initrd.img
            }
            menuentry 'Azure Linux (recovery)' --class azurelinux {
                linux /boot/vmlinuz root=/dev/sda2 single
                initrd /boot/initrd.img
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        let result = &lines[0];
        assert!(result.contains("root=/dev/sda2"));
        assert!(result.contains("selinux=1"));
        assert!(result.contains("rd.overlayfs="));
        // Recovery entry should be excluded
        assert!(!result.contains("single"));
    }

    #[test]
    fn test_find_non_recovery_linux_lines_no_recovery() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda1"));
    }

    #[test]
    fn test_no_linux_line() {
        let grub_cfg = "set timeout=5\n";
        assert!(find_non_recovery_linux_lines(grub_cfg).is_err());
    }

    #[test]
    fn test_recovery_detection_is_case_sensitive() {
        // Go uses case-sensitive "recovery" check. "Recovery" should NOT
        // be filtered as recovery.
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux Recovery Mode' {
                linux /boot/vmlinuz root=/dev/sda1 single
            }
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda2
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        // "Recovery" (capital R) should not match — only lowercase "recovery"
        // is filtered by the Go code. However, the title here is "Linux Recovery Mode"
        // which does NOT contain lowercase "recovery", so both entries are kept.
        // Wait — actually "Recovery" does not contain "recovery" (case-sensitive).
        // So we get 2 linux lines.
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_multiple_non_recovery_linux_lines() {
        // Go expects exactly 1 non-recovery linux line.
        // extract_boot_args_from_grub_cfg should error on multiple.
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux A' {
                linux /boot/vmlinuz root=/dev/sda1
            }
            menuentry 'Linux B' {
                linux /boot/vmlinuz root=/dev/sda2
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 2);
    }

    #[test]
    fn test_linux_line_captures_args_after_keyword() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda2 selinux=1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        // Should capture everything after "linux " — including the kernel path
        assert!(lines[0].starts_with("/boot/vmlinuz"));
        assert!(lines[0].contains("selinux=1"));
    }
}
