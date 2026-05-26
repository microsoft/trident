// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! grub.cfg parsing and grub2-mkconfig execution.
//!
//! Used by the `update_default_grub` flow to extract boot args from the
//! generated grub.cfg and sync them back to /etc/default/grub.
//!
//! The non-recovery linux line extraction is delegated to
//! [`osutils::grub::find_non_recovery_linux_lines`], which is shared with
//! other consumers in the trident codebase.

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
    let linux_lines = osutils::grub::find_non_recovery_linux_lines(&content)?;
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
                // Skip variable references (e.g., root=$rootdevice). Go's
                // ParseCommandLineArgs detects VAR_EXPANSION tokens and clears
                // the value; we match by skipping the token entirely.
                if v.starts_with('$') {
                    trace!("Skipping variable reference: {token}");
                    continue;
                }
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
    use tempfile::tempdir;

    // ---------------------------------------------------------------
    // Helper: write a grub.cfg in a temp dir and call the public API
    // ---------------------------------------------------------------
    fn extract_from_grub_cfg_str(content: &str) -> Result<(Vec<String>, Option<String>), Error> {
        let tmp = tempdir().unwrap();
        let grub_dir = tmp.path().join("boot/grub2");
        std::fs::create_dir_all(&grub_dir).unwrap();
        std::fs::write(grub_dir.join("grub.cfg"), content).unwrap();
        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };
        extract_boot_args_from_grub_cfg(&ctx)
    }

    // ======================= extract_boot_args_from_grub_cfg =======================

    #[test]
    fn test_extract_args_basic() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Azure Linux' {
                linux /boot/vmlinuz root=/dev/sda2 selinux=1 enforcing=1 rd.overlayfs=/a,/b,/c,/dev/sda3
            }
        "#};

        let (args, root_device) = extract_from_grub_cfg_str(grub_cfg).unwrap();

        assert_eq!(root_device, Some("/dev/sda2".to_string()));
        assert!(args.contains(&"selinux=1".to_string()));
        assert!(args.contains(&"enforcing=1".to_string()));
        assert!(args.iter().any(|a| a.starts_with("rd.overlayfs=")));
        // root should NOT be in args (it goes to root_device)
        assert!(!args.iter().any(|a| a.starts_with("root=")));
    }

    #[test]
    fn test_extract_args_no_root() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz selinux=1
            }
        "#};

        let (args, root_device) = extract_from_grub_cfg_str(grub_cfg).unwrap();

        assert_eq!(root_device, None);
        assert!(args.contains(&"selinux=1".to_string()));
    }

    #[test]
    fn test_extract_args_ignores_unknown_args() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz quiet root=/dev/sda1 loglevel=3 selinux=1 splash
            }
        "#};

        let (args, root_device) = extract_from_grub_cfg_str(grub_cfg).unwrap();

        assert_eq!(root_device, Some("/dev/sda1".to_string()));
        assert_eq!(args, vec!["selinux=1"]);
        // quiet, loglevel, splash should NOT appear
    }

    #[test]
    fn test_extract_errors_on_multiple_non_recovery_entries() {
        // Go expects exactly 1 non-recovery linux line
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux A' {
                linux /boot/vmlinuz root=/dev/sda1
            }
            menuentry 'Linux B' {
                linux /boot/vmlinuz root=/dev/sda2
            }
        "#};

        let result = extract_from_grub_cfg_str(grub_cfg);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("expected 1"),
            "Error should mention expecting 1 line, got: {err_msg}"
        );
    }

    #[test]
    fn test_extract_skips_variable_references() {
        // root=$rootdevice should NOT produce a GRUB_DEVICE write
        let grub_cfg = indoc::indoc! {r#"
            menuentry "CBL-Mariner" {
            	linux /boot/vmlinuz root=$rootdevice selinux=0
            }
        "#};

        let (args, root_device) = extract_from_grub_cfg_str(grub_cfg).unwrap();
        assert_eq!(
            root_device, None,
            "Variable reference root=$rootdevice should be skipped"
        );
        assert!(
            args.contains(&"selinux=0".to_string()),
            "Non-variable args should still be captured"
        );
    }

    #[test]
    fn test_extract_args_with_roothash() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/mapper/root roothash=abc123 selinux=1 enforcing=1
            }
        "#};

        let (args, root_device) = extract_from_grub_cfg_str(grub_cfg).unwrap();

        assert_eq!(root_device, Some("/dev/mapper/root".to_string()));
        assert!(args.contains(&"roothash=abc123".to_string()));
        assert!(args.contains(&"selinux=1".to_string()));
        assert!(args.contains(&"enforcing=1".to_string()));
    }

    #[test]
    fn test_extract_args_empty_result() {
        // No sync-worthy args
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz quiet loglevel=3
            }
        "#};

        let (args, root_device) = extract_from_grub_cfg_str(grub_cfg).unwrap();
        assert!(args.is_empty());
        assert_eq!(root_device, None);
    }

    #[test]
    fn test_extract_missing_grub_cfg_errors() {
        let tmp = tempdir().unwrap();
        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };
        assert!(extract_boot_args_from_grub_cfg(&ctx).is_err());
    }

    #[test]
    fn test_extract_finds_grub2_path() {
        let tmp = tempdir().unwrap();
        let grub_dir = tmp.path().join("boot/grub2");
        std::fs::create_dir_all(&grub_dir).unwrap();
        std::fs::write(
            grub_dir.join("grub.cfg"),
            "menuentry 'L' {\n\tlinux /vmlinuz root=/dev/sda1\n}\n",
        )
        .unwrap();
        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        let (_, root) = extract_boot_args_from_grub_cfg(&ctx).unwrap();
        assert_eq!(root, Some("/dev/sda1".to_string()));
    }

    #[test]
    fn test_extract_finds_grub_fallback_path() {
        let tmp = tempdir().unwrap();
        // Only /boot/grub/ (not grub2/)
        let grub_dir = tmp.path().join("boot/grub");
        std::fs::create_dir_all(&grub_dir).unwrap();
        std::fs::write(
            grub_dir.join("grub.cfg"),
            "menuentry 'L' {\n\tlinux /vmlinuz root=/dev/sdb1\n}\n",
        )
        .unwrap();
        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        let (_, root) = extract_boot_args_from_grub_cfg(&ctx).unwrap();
        assert_eq!(root, Some("/dev/sdb1".to_string()));
    }
}
