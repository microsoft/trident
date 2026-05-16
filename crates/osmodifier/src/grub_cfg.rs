// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! grub.cfg parsing and grub2-mkconfig execution.
//!
//! Used by the `update_default_grub` flow to extract boot args from the
//! generated grub.cfg and sync them back to /etc/default/grub.

use std::fs;

use anyhow::{bail, Context, Error};
use log::{debug, info, trace};
use osutils::dependencies::Dependency;
use regex::Regex;

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
pub fn extract_boot_args_from_grub_cfg(
    ctx: &OsModifierContext,
) -> Result<(Vec<String>, Option<String>), Error> {
    let grub_cfg_path = find_grub_cfg(ctx)?;
    let content = fs::read_to_string(&grub_cfg_path)
        .with_context(|| format!("Failed to read '{}'", grub_cfg_path.display()))?;

    trace!("grub.cfg content:\n{content}");

    // Find the non-recovery linux command line
    let linux_line = find_non_recovery_linux_line(&content)?;
    debug!("Found linux line: {linux_line}");

    // Parse args from the linux line
    let mut args = Vec::new();
    let mut root_device = None;

    for token in linux_line.split_whitespace() {
        let (name, value) = match token.split_once('=') {
            Some((n, v)) => (n, Some(v)),
            None => (token, None),
        };

        if SYNC_ARG_NAMES.contains(&name) {
            if name == "root" {
                if let Some(v) = value {
                    root_device = Some(v.to_string());
                }
            } else if let Some(v) = value {
                args.push(format!("{name}={v}"));
            }
        }
    }

    Ok((args, root_device))
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

/// Find the linux command line from a non-recovery menuentry in grub.cfg.
///
/// This matches the Go `FindNonRecoveryLinuxLine` behavior:
/// - Scans for `menuentry` blocks
/// - Skips entries whose title contains "recovery"
/// - Returns the `linux` line from the first non-recovery entry
/// - Expects exactly one match
fn find_non_recovery_linux_line(content: &str) -> Result<String, Error> {
    // Simple state-machine approach: track whether we're inside a menuentry
    // block, skip recovery entries, find the linux line.
    let menuentry_re = Regex::new(r#"^\s*menuentry\s+['"](.*?)['"]\s"#)
        .context("Failed to compile menuentry regex")?;
    let linux_re = Regex::new(r"^\s*linux\s+(.+)$").context("Failed to compile linux regex")?;

    let mut in_menuentry = false;
    let mut is_recovery = false;
    let mut brace_depth: i32 = 0;
    let mut linux_lines = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Track menuentry blocks
        if let Some(caps) = menuentry_re.captures(line) {
            let title = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            is_recovery = title.to_lowercase().contains("recovery");
            in_menuentry = true;
            brace_depth = 0;
        }

        // Track braces
        for ch in trimmed.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if brace_depth <= 0 {
                        in_menuentry = false;
                        is_recovery = false;
                    }
                }
                _ => {}
            }
        }

        // Look for linux lines in non-recovery menuentries
        if in_menuentry && !is_recovery {
            if let Some(caps) = linux_re.captures(line) {
                if let Some(args) = caps.get(1) {
                    linux_lines.push(args.as_str().to_string());
                }
            }
        }
    }

    if linux_lines.is_empty() {
        bail!("No non-recovery linux command line found in grub.cfg");
    }

    if linux_lines.len() > 1 {
        // The Go code expects exactly one. We use the first one with a warning.
        debug!(
            "Found {} non-recovery linux lines, using the first one",
            linux_lines.len()
        );
    }

    Ok(linux_lines.into_iter().next().unwrap())
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
    fn test_find_non_recovery_linux_line() {
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

        let result = find_non_recovery_linux_line(grub_cfg).unwrap();
        assert!(result.contains("root=/dev/sda2"));
        assert!(result.contains("selinux=1"));
        assert!(result.contains("rd.overlayfs="));
    }

    #[test]
    fn test_find_non_recovery_linux_line_no_recovery() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let result = find_non_recovery_linux_line(grub_cfg).unwrap();
        assert!(result.contains("root=/dev/sda1"));
    }

    #[test]
    fn test_no_linux_line() {
        let grub_cfg = "set timeout=5\n";
        assert!(find_non_recovery_linux_line(grub_cfg).is_err());
    }
}




