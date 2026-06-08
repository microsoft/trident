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

use crate::constants::SYNC_ARG_NAMES;
use crate::OsModifierContext;

/// Possible grub.cfg locations, tried in order.
const GRUB_CFG_PATHS: &[&str] = &["/boot/grub2/grub.cfg", "/boot/grub/grub.cfg"];

/// BLS (Boot Loader Spec) entry directory. Fedora-based distros (including
/// AZL4) store kernel boot entries here instead of inline in grub.cfg.
const BLS_ENTRIES_DIR: &str = "/boot/loader/entries";

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
    let linux_lines = match find_non_recovery_linux_lines(&content) {
        Ok(lines) => lines,
        Err(_) if content.contains("blscfg") => {
            debug!("grub.cfg uses BLS (blscfg); reading boot args from BLS entries");
            extract_options_from_bls_entries(ctx)?
        }
        Err(e) => return Err(e),
    };
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

/// Read boot arguments from BLS (Boot Loader Spec) entries.
///
/// Scans `{root}/boot/loader/entries/*.conf`, skips entries whose title
/// contains "rescue" or "recovery" (case-insensitive), and returns the
/// `options` line from the first valid entry (sorted lexically, matching
/// grub's ordering).
fn extract_options_from_bls_entries(ctx: &OsModifierContext) -> Result<Vec<String>, Error> {
    let entries_dir = ctx.path(BLS_ENTRIES_DIR);
    let mut conf_files: Vec<std::path::PathBuf> = fs::read_dir(&entries_dir)
        .with_context(|| format!("Failed to read BLS entries dir '{}'", entries_dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |ext| ext == "conf"))
        .collect();

    conf_files.sort();

    for conf_path in &conf_files {
        let content = fs::read_to_string(conf_path)
            .with_context(|| format!("Failed to read BLS entry '{}'", conf_path.display()))?;

        let mut title = None;
        let mut options = None;

        for line in content.lines() {
            if let Some(value) = line.strip_prefix("title") {
                title = Some(value.trim().to_string());
            } else if let Some(value) = line.strip_prefix("options") {
                options = Some(value.trim().to_string());
            }
        }

        // Skip recovery/rescue entries.
        if let Some(ref t) = title {
            let lower = t.to_lowercase();
            if lower.contains("rescue") || lower.contains("recovery") {
                trace!(
                    "Skipping BLS rescue/recovery entry: {}",
                    conf_path.display()
                );
                continue;
            }
        }

        if let Some(opts) = options {
            debug!(
                "Using BLS entry '{}': options = {opts}",
                conf_path.display()
            );
            // Return as a synthetic "linux" line: prepend a dummy kernel path
            // so the downstream parser (which skips the first token) works.
            return Ok(vec![format!("/boot/vmlinuz {opts}")]);
        }
    }

    bail!(
        "no non-recovery BLS entry found in '{}'",
        entries_dir.display()
    )
}

/// Return the first whitespace-delimited word from a line, or None if the
/// line is empty / whitespace-only.
fn first_word(line: &str) -> Option<&str> {
    line.split_whitespace().next()
}

/// Extract the quoted title from the text after the `menuentry` keyword.
/// Handles both single and double quotes. Returns the content between the
/// first pair of matching quotes, or None if no quoted string is found.
fn extract_quoted_title(after_menuentry: &str) -> Option<&str> {
    let s = after_menuentry.trim();
    let quote = s.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let inner = &s[1..];
    let end = inner.find(quote)?;
    Some(&inner[..end])
}

/// Count unquoted `{` and `}` characters in a line, skipping characters
/// inside single or double quotes. Returns `(open_count, close_count)`.
fn count_braces(line: &str) -> (usize, usize) {
    let mut opens = 0usize;
    let mut closes = 0usize;
    let mut in_quote: Option<char> = None;

    for ch in line.chars() {
        match in_quote {
            Some(q) if ch == q => in_quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => in_quote = Some(ch),
            None if ch == '{' => opens += 1,
            None if ch == '}' => closes += 1,
            _ => {}
        }
    }
    (opens, closes)
}

/// Subtract `closes` from `depth`, bailing on underflow which indicates
/// malformed grub.cfg with unbalanced braces.
fn checked_depth(depth: usize, closes: usize) -> Result<usize, Error> {
    depth
        .checked_sub(closes)
        .context("malformed grub.cfg: unbalanced braces (more '}' than '{')")
}

/// Find `linux` directive lines from top-level non-recovery menuentries in
/// grub.cfg content.
///
/// Walks the raw grub.cfg text line-by-line looking for `menuentry`
/// keywords, checks the quoted title for the substring `"recovery"`
/// (case-sensitive, matching the Go implementation), and collects the
/// arguments portion of each `linux` line inside non-recovery blocks.
///
/// Returns all matches; callers decide whether to require exactly one.
///
/// # Submenu handling
///
/// On systems with multiple kernels, `grub2-mkconfig` produces a
/// top-level menuentry for the default kernel plus a
/// `submenu 'Advanced options ...'` block containing additional
/// menuentries (including recovery variants). This parser tracks brace
/// depth and skips `submenu { ... }` blocks entirely, so only top-level
/// menuentries contribute `linux` lines.
///
/// This goes beyond the Go implementation's `FindNonRecoveryLinuxLine`,
/// which does not track submenus and would return >1 line on multi-kernel
/// systems.
fn find_non_recovery_linux_lines(content: &str) -> Result<Vec<String>, Error> {
    let mut depth: usize = 0;
    let mut in_top_level_menuentry = false;
    let mut in_submenu = false;
    let mut submenu_start_depth: usize = 0;
    let mut linux_lines = Vec::new();

    for line in content.lines() {
        let keyword = first_word(line);
        let (opens, closes) = count_braces(line);

        // If inside a submenu block, just track depth until we exit.
        if in_submenu {
            depth = checked_depth(depth + opens, closes)?;
            if depth <= submenu_start_depth {
                in_submenu = false;
            }
            continue;
        }

        if let Some(kw) = keyword {
            if kw == "submenu" {
                // Enter submenu — skip everything inside it.
                in_submenu = true;
                submenu_start_depth = depth;
                depth = checked_depth(depth + opens, closes)?;
                // Edge case: opening and closing brace on same line
                if depth <= submenu_start_depth {
                    in_submenu = false;
                }
                continue;
            }

            if kw == "menuentry" && depth == 0 {
                in_top_level_menuentry = true;
                let after_keyword =
                    line[line.find("menuentry").unwrap() + "menuentry".len()..].trim();
                if let Some(title) = extract_quoted_title(after_keyword) {
                    if title.contains("recovery") {
                        in_top_level_menuentry = false;
                    }
                }
            } else if in_top_level_menuentry && kw == "linux" {
                let after_linux = line[line.find("linux").unwrap() + "linux".len()..].trim();
                if !after_linux.is_empty() {
                    linux_lines.push(after_linux.to_string());
                }
            }
        }

        // Update depth after processing the line's keywords.
        depth = checked_depth(depth + opens, closes)?;

        // If we just closed the top-level menuentry block, reset state.
        if depth == 0 {
            in_top_level_menuentry = false;
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

    // ======================= find_non_recovery_linux_lines =======================

    #[test]
    fn test_non_recovery_with_recovery_entry() {
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
        assert!(!result.contains("single"));
    }

    #[test]
    fn test_single_non_recovery_entry() {
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
    fn test_no_linux_line_errors() {
        let grub_cfg = "set timeout=5\n";
        assert!(find_non_recovery_linux_lines(grub_cfg).is_err());
    }

    #[test]
    fn test_only_recovery_entries_errors() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux (recovery)' {
                linux /boot/vmlinuz root=/dev/sda1 single
            }
        "#};
        assert!(find_non_recovery_linux_lines(grub_cfg).is_err());
    }

    #[test]
    fn test_recovery_detection_is_case_sensitive() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux Recovery Mode' {
                linux /boot/vmlinuz root=/dev/sda1 single
            }
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda2
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(
            lines.len(),
            2,
            "uppercase 'Recovery' should not be filtered"
        );
    }

    #[test]
    fn test_multiple_non_recovery_entries() {
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
    fn test_linux_line_captures_full_args() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda2 selinux=1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert!(lines[0].starts_with("/boot/vmlinuz"));
        assert!(lines[0].contains("selinux=1"));
    }

    #[test]
    fn test_tab_indented_grub_cfg() {
        let grub_cfg = "menuentry 'Linux' {\n\tlinux /boot/vmlinuz root=/dev/sda2 selinux=1\n\tinitrd /boot/initrd.img\n}\n";

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda2"));
    }

    #[test]
    fn test_double_quoted_menuentry_title() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry "Azure Linux" {
                linux /boot/vmlinuz root=/dev/sda1
            }
            menuentry "Azure Linux (recovery)" {
                linux /boot/vmlinuz root=/dev/sda1 single
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(!lines[0].contains("single"));
    }

    #[test]
    fn test_recovery_in_class_not_in_title() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Azure Linux' --class recovery-icon {
                linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(
            lines.len(),
            1,
            "recovery in class name should not filter the entry"
        );
    }

    #[test]
    fn test_real_world_azl2_grub_cfg() {
        let grub_cfg = indoc::indoc! {r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 33beac00-b378-4b0c-b0cb-d5dcebf2cf57 -s

            load_env -f $bootprefix/mariner.cfg

            set rootdevice=PARTUUID=c17c558b-068b-459c-92cb-f218d14b44a1

            menuentry "CBL-Mariner" {
            	linux $bootprefix/$mariner_linux       rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity selinux=0 $systemd_cmdline   $kernelopts
            	if [ -f $bootprefix/$mariner_initrd ]; then
            		initrd $bootprefix/$mariner_initrd
            	fi
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("selinux=0"));
        assert!(lines[0].contains("root=$rootdevice"));
    }

    #[test]
    fn test_menuentry_without_linux_line() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Empty Entry' {
                set gfxpayload=keep
            }
            menuentry 'Real Entry' {
                linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda1"));
    }

    #[test]
    fn test_linux_outside_menuentry_ignored() {
        let grub_cfg = indoc::indoc! {r#"
            linux /boot/stray-vmlinuz root=/dev/stray
            menuentry 'Linux' {
                linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda1"));
    }

    // ======================= submenu handling =======================

    #[test]
    fn test_submenu_entries_are_skipped() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Azure Linux' --class azurelinux {
            	linux /boot/vmlinuz-6.6.60 root=UUID=abc selinux=1
            	initrd /boot/initramfs-6.6.60.img
            }
            submenu 'Advanced options for Azure Linux' --class azurelinux {
            	menuentry 'Azure Linux, with Linux 6.6.60' --class azurelinux {
            		linux /boot/vmlinuz-6.6.60 root=UUID=abc selinux=1
            		initrd /boot/initramfs-6.6.60.img
            	}
            	menuentry 'Azure Linux, with Linux 6.6.60 (recovery mode)' --class azurelinux {
            		linux /boot/vmlinuz-6.6.60 root=UUID=abc single
            		initrd /boot/initramfs-6.6.60.img
            	}
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(
            lines.len(),
            1,
            "submenu entries should be skipped, only top-level entry counts"
        );
        assert!(lines[0].contains("root=UUID=abc"));
        assert!(lines[0].contains("selinux=1"));
    }

    #[test]
    fn test_multi_kernel_submenu() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Azure Linux' --class azurelinux {
            	linux /boot/vmlinuz-6.6.60 root=UUID=abc selinux=1
            	initrd /boot/initramfs-6.6.60.img
            }
            submenu 'Advanced options for Azure Linux' --class azurelinux {
            	menuentry 'Azure Linux, with Linux 6.6.60' --class azurelinux {
            		linux /boot/vmlinuz-6.6.60 root=UUID=abc selinux=1
            		initrd /boot/initramfs-6.6.60.img
            	}
            	menuentry 'Azure Linux, with Linux 6.6.60 (recovery mode)' --class azurelinux {
            		linux /boot/vmlinuz-6.6.60 root=UUID=abc single
            		initrd /boot/initramfs-6.6.60.img
            	}
            	menuentry 'Azure Linux, with Linux 6.6.51' --class azurelinux {
            		linux /boot/vmlinuz-6.6.51 root=UUID=abc selinux=1
            		initrd /boot/initramfs-6.6.51.img
            	}
            	menuentry 'Azure Linux, with Linux 6.6.51 (recovery mode)' --class azurelinux {
            		linux /boot/vmlinuz-6.6.51 root=UUID=abc single
            		initrd /boot/initramfs-6.6.51.img
            	}
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(
            lines.len(),
            1,
            "only the top-level menuentry's linux line should be returned"
        );
        assert!(
            lines[0].contains("vmlinuz-6.6.60"),
            "should capture the newest (top-level) kernel"
        );
    }

    #[test]
    fn test_submenu_before_top_level_entry() {
        let grub_cfg = indoc::indoc! {r#"
            submenu 'Advanced options' {
            	menuentry 'Linux old' {
            		linux /boot/vmlinuz-old root=/dev/sda1
            	}
            }
            menuentry 'Linux' {
            	linux /boot/vmlinuz root=/dev/sda2 selinux=1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda2"));
    }

    #[test]
    fn test_submenu_only_errors() {
        let grub_cfg = indoc::indoc! {r#"
            submenu 'Advanced options' {
            	menuentry 'Linux' {
            		linux /boot/vmlinuz root=/dev/sda1
            	}
            }
        "#};

        assert!(find_non_recovery_linux_lines(grub_cfg).is_err());
    }

    #[test]
    fn test_braces_in_quoted_title_not_counted() {
        let grub_cfg = indoc::indoc! {r#"
            menuentry 'Azure Linux {debug}' {
            	linux /boot/vmlinuz root=/dev/sda1
            }
        "#};

        let lines = find_non_recovery_linux_lines(grub_cfg).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("root=/dev/sda1"));
    }

    // ======================= count_braces =======================

    #[test]
    fn test_count_braces_basic() {
        assert_eq!(count_braces("menuentry 'Linux' {"), (1, 0));
        assert_eq!(count_braces("}"), (0, 1));
        assert_eq!(count_braces("no braces here"), (0, 0));
    }

    #[test]
    fn test_count_braces_skips_quoted() {
        assert_eq!(count_braces("menuentry 'title {x}' {"), (1, 0));
        assert_eq!(count_braces(r#"menuentry "title {x}" {"#), (1, 0));
    }

    // ======================= BLS entry support =======================

    #[test]
    fn test_extract_bls_fallback() {
        let tmp = tempdir().unwrap();

        // Write a BLS-style grub.cfg (contains blscfg, no inline linux lines)
        let grub_dir = tmp.path().join("boot/grub2");
        std::fs::create_dir_all(&grub_dir).unwrap();
        std::fs::write(
            grub_dir.join("grub.cfg"),
            indoc::indoc! {r#"
                set timeout=5
                load_env -f /boot/grub2/grubenv
                blscfg
            "#},
        )
        .unwrap();

        // Write a BLS entry
        let bls_dir = tmp.path().join("boot/loader/entries");
        std::fs::create_dir_all(&bls_dir).unwrap();
        std::fs::write(
            bls_dir.join("azl4.conf"),
            indoc::indoc! {r#"
                title Azure Linux 4.0 (6.6.60)
                version 6.6.60
                linux /boot/vmlinuz-6.6.60
                initrd /boot/initramfs-6.6.60.img
                options root=/dev/sda2 ro selinux=1 rd.overlayfs=lower,upper,work,/dev/sda5
            "#},
        )
        .unwrap();

        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        let (args, root_device) = extract_boot_args_from_grub_cfg(&ctx).unwrap();
        assert_eq!(root_device, Some("/dev/sda2".to_string()));
        assert!(args.contains(&"selinux=1".to_string()));
    }

    #[test]
    fn test_extract_bls_skips_recovery() {
        let tmp = tempdir().unwrap();

        let grub_dir = tmp.path().join("boot/grub2");
        std::fs::create_dir_all(&grub_dir).unwrap();
        std::fs::write(grub_dir.join("grub.cfg"), "set timeout=5\nblscfg\n").unwrap();

        let bls_dir = tmp.path().join("boot/loader/entries");
        std::fs::create_dir_all(&bls_dir).unwrap();

        // Rescue entry (should be skipped)
        std::fs::write(
            bls_dir.join("rescue.conf"),
            indoc::indoc! {r#"
                title Azure Linux 4.0 rescue
                version 6.6.60
                linux /boot/vmlinuz-6.6.60
                initrd /boot/initramfs-6.6.60.img
                options root=/dev/sda2 ro single
            "#},
        )
        .unwrap();

        // Normal entry (should be used)
        std::fs::write(
            bls_dir.join("zzz-normal.conf"),
            indoc::indoc! {r#"
                title Azure Linux 4.0 (6.6.60)
                version 6.6.60
                linux /boot/vmlinuz-6.6.60
                initrd /boot/initramfs-6.6.60.img
                options root=/dev/sda2 ro selinux=1
            "#},
        )
        .unwrap();

        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        let (args, root_device) = extract_boot_args_from_grub_cfg(&ctx).unwrap();
        assert_eq!(root_device, Some("/dev/sda2".to_string()));
        assert!(args.contains(&"selinux=1".to_string()));
        // "single" from rescue entry should NOT appear
        assert!(!args.iter().any(|a| a.contains("single")));
    }

    #[test]
    fn test_extract_bls_no_entries() {
        let tmp = tempdir().unwrap();

        let grub_dir = tmp.path().join("boot/grub2");
        std::fs::create_dir_all(&grub_dir).unwrap();
        std::fs::write(grub_dir.join("grub.cfg"), "set timeout=5\nblscfg\n").unwrap();

        // Empty BLS entries dir
        let bls_dir = tmp.path().join("boot/loader/entries");
        std::fs::create_dir_all(&bls_dir).unwrap();

        let ctx = OsModifierContext {
            root: tmp.path().to_path_buf(),
        };

        let result = extract_boot_args_from_grub_cfg(&ctx);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("no non-recovery BLS entry found"),
            "Error should mention no BLS entries, got: {err_msg}"
        );
    }
}
