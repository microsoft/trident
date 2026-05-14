// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Kernel module configuration — write modules-load.d and modprobe.d configs.

use std::fs;

use anyhow::{bail, Context, Error};
use log::debug;

use trident_api::config::{LoadMode, Module};

use crate::OsModifierContext;

const MODULES_LOAD_DIR: &str = "/etc/modules-load.d";
const MODULES_LOAD_CONF: &str = "/etc/modules-load.d/modules-load.conf";
const MODPROBE_DIR: &str = "/etc/modprobe.d";
const MODPROBE_DISABLED_CONF: &str = "/etc/modprobe.d/modules-disabled.conf";
const MODPROBE_OPTIONS_CONF: &str = "/etc/modprobe.d/module-options.conf";

/// Configure kernel modules by writing modules-load.d and modprobe.d files.
pub fn configure(ctx: &OsModifierContext, modules: &[Module]) -> Result<(), Error> {
    // Read existing configs (or start fresh)
    let load_path = ctx.path(MODULES_LOAD_CONF);
    let disabled_path = ctx.path(MODPROBE_DISABLED_CONF);
    let options_path = ctx.path(MODPROBE_OPTIONS_CONF);

    let mut load_lines = read_config_lines(ctx, MODULES_LOAD_CONF);
    let mut disabled_lines = read_config_lines(ctx, MODPROBE_DISABLED_CONF);
    let mut options_lines = read_config_lines(ctx, MODPROBE_OPTIONS_CONF);

    for module in modules {
        match module.load_mode {
            LoadMode::Always => {
                debug!("Module '{}': set to always load", module.name);
                // Remove from blacklist if present
                remove_blacklist(&mut disabled_lines, &module.name);
                // Add to auto-load list if not present
                if !load_lines.iter().any(|l| l.trim() == module.name) {
                    load_lines.push(module.name.clone());
                }
                // Set options if provided
                if !module.options.is_empty() {
                    update_options(&mut options_lines, &module.name, &module.options);
                }
            }
            LoadMode::Auto => {
                debug!("Module '{}': set to auto", module.name);
                // Remove from blacklist if present
                remove_blacklist(&mut disabled_lines, &module.name);
                // Set options if provided
                if !module.options.is_empty() {
                    update_options(&mut options_lines, &module.name, &module.options);
                }
            }
            LoadMode::Disable => {
                debug!("Module '{}': set to disabled", module.name);
                if !module.options.is_empty() {
                    bail!(
                        "Module '{}' is disabled but has options set — this is not allowed",
                        module.name
                    );
                }
                // Remove from auto-load list
                load_lines.retain(|l| l.trim() != module.name);
                // Add to blacklist if not present
                let blacklist_entry = format!("blacklist {}", module.name);
                if !disabled_lines.iter().any(|l| l.trim() == blacklist_entry) {
                    disabled_lines.push(blacklist_entry);
                }
            }
            LoadMode::Inherit => {
                debug!("Module '{}': inherit (update options only)", module.name);
                // Only update options if module is not disabled
                let is_disabled = disabled_lines
                    .iter()
                    .any(|l| l.trim() == format!("blacklist {}", module.name));
                if !is_disabled && !module.options.is_empty() {
                    update_options(&mut options_lines, &module.name, &module.options);
                }
            }
        }
    }

    // Write out the config files
    ensure_dir(ctx, MODULES_LOAD_DIR)?;
    ensure_dir(ctx, MODPROBE_DIR)?;

    write_config(&load_path, &load_lines)?;
    write_config(&disabled_path, &disabled_lines)?;
    write_config(&options_path, &options_lines)?;

    Ok(())
}

fn read_config_lines(ctx: &OsModifierContext, path: &str) -> Vec<String> {
    let full = ctx.path(path);
    fs::read_to_string(&full)
        .map(|s| s.lines().map(String::from).collect())
        .unwrap_or_default()
}

fn remove_blacklist(lines: &mut Vec<String>, module_name: &str) {
    let entry = format!("blacklist {module_name}");
    lines.retain(|l| l.trim() != entry);
}

fn update_options(
    lines: &mut Vec<String>,
    module_name: &str,
    options: &std::collections::HashMap<String, String>,
) {
    // Remove any existing options line for this module
    let prefix = format!("options {module_name} ");
    lines.retain(|l| !l.starts_with(&prefix) && l.trim() != format!("options {module_name}"));

    // Build new options line
    if !options.is_empty() {
        let opts_str: Vec<String> = options.iter().map(|(k, v)| format!("{k}={v}")).collect();
        lines.push(format!("options {module_name} {}", opts_str.join(" ")));
    }
}

fn ensure_dir(ctx: &OsModifierContext, path: &str) -> Result<(), Error> {
    let full = ctx.path(path);
    fs::create_dir_all(&full)
        .with_context(|| format!("Failed to create directory '{}'", full.display()))
}

fn write_config(path: &std::path::Path, lines: &[String]) -> Result<(), Error> {
    let content = if lines.is_empty() {
        String::new()
    } else {
        let mut s = lines.join("\n");
        s.push('\n');
        s
    };
    fs::write(path, &content)
        .with_context(|| format!("Failed to write config to '{}'", path.display()))
}

#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use std::collections::HashMap;
    use tempfile::tempdir;

    use pytest_gen::functional_test;
    use trident_api::config::LoadMode;

    use crate::OsModifierContext;

    fn make_ctx(tmp: &tempfile::TempDir) -> OsModifierContext {
        OsModifierContext {
            root: tmp.path().to_path_buf(),
        }
    }

    #[functional_test(feature = "core")]
    fn test_configure_modules_always_load() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(&tmp);

        let modules = vec![Module {
            name: "br_netfilter".to_string(),
            load_mode: LoadMode::Always,
            options: HashMap::new(),
        }];

        configure(&ctx, &modules).unwrap();

        let load_conf =
            fs::read_to_string(tmp.path().join("etc/modules-load.d/modules-load.conf")).unwrap();
        assert!(
            load_conf.contains("br_netfilter"),
            "Expected br_netfilter in modules-load.conf, got: {load_conf}"
        );
    }

    #[functional_test(feature = "core")]
    fn test_configure_modules_disable() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(&tmp);

        let modules = vec![Module {
            name: "floppy".to_string(),
            load_mode: LoadMode::Disable,
            options: HashMap::new(),
        }];

        configure(&ctx, &modules).unwrap();

        let disabled_conf =
            fs::read_to_string(tmp.path().join("etc/modprobe.d/modules-disabled.conf")).unwrap();
        assert!(
            disabled_conf.contains("blacklist floppy"),
            "Expected 'blacklist floppy' in modules-disabled.conf, got: {disabled_conf}"
        );

        // Should NOT appear in modules-load.conf
        let load_conf =
            fs::read_to_string(tmp.path().join("etc/modules-load.d/modules-load.conf")).unwrap();
        assert!(
            !load_conf.contains("floppy"),
            "floppy should not be in modules-load.conf"
        );
    }

    #[functional_test(feature = "core")]
    fn test_configure_modules_with_options() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(&tmp);

        let mut opts = HashMap::new();
        opts.insert("num_vfs".to_string(), "4".to_string());

        let modules = vec![Module {
            name: "ixgbevf".to_string(),
            load_mode: LoadMode::Always,
            options: opts,
        }];

        configure(&ctx, &modules).unwrap();

        let options_conf =
            fs::read_to_string(tmp.path().join("etc/modprobe.d/module-options.conf")).unwrap();
        assert!(
            options_conf.contains("options ixgbevf num_vfs=4"),
            "Expected module options line, got: {options_conf}"
        );
    }

    #[functional_test(feature = "core", negative = true)]
    fn test_configure_modules_disable_with_options_fails() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(&tmp);

        let mut opts = HashMap::new();
        opts.insert("bad".to_string(), "option".to_string());

        let modules = vec![Module {
            name: "floppy".to_string(),
            load_mode: LoadMode::Disable,
            options: opts,
        }];

        let result = configure(&ctx, &modules);
        assert!(
            result.is_err(),
            "Disabling a module with options should fail"
        );
    }

    #[functional_test(feature = "core")]
    fn test_configure_modules_idempotent() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(&tmp);

        let modules = vec![Module {
            name: "br_netfilter".to_string(),
            load_mode: LoadMode::Always,
            options: HashMap::new(),
        }];

        // Apply twice
        configure(&ctx, &modules).unwrap();
        configure(&ctx, &modules).unwrap();

        let load_conf =
            fs::read_to_string(tmp.path().join("etc/modules-load.d/modules-load.conf")).unwrap();
        let count = load_conf.matches("br_netfilter").count();
        assert_eq!(count, 1, "Module should appear exactly once, got {count}");
    }

    #[functional_test(feature = "core")]
    fn test_configure_modules_disable_removes_from_load() {
        let tmp = tempdir().unwrap();
        let ctx = make_ctx(&tmp);

        // First enable
        let enable = vec![Module {
            name: "br_netfilter".to_string(),
            load_mode: LoadMode::Always,
            options: HashMap::new(),
        }];
        configure(&ctx, &enable).unwrap();

        // Then disable
        let disable = vec![Module {
            name: "br_netfilter".to_string(),
            load_mode: LoadMode::Disable,
            options: HashMap::new(),
        }];
        configure(&ctx, &disable).unwrap();

        let load_conf =
            fs::read_to_string(tmp.path().join("etc/modules-load.d/modules-load.conf")).unwrap();
        assert!(
            !load_conf.contains("br_netfilter"),
            "Disabled module should be removed from modules-load.conf"
        );

        let disabled_conf =
            fs::read_to_string(tmp.path().join("etc/modprobe.d/modules-disabled.conf")).unwrap();
        assert!(
            disabled_conf.contains("blacklist br_netfilter"),
            "Disabled module should appear in blacklist"
        );
    }
}
