// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Kernel module configuration — write modules-load.d and modprobe.d configs.

use std::fs;

use anyhow::{bail, Context, Error};
use log::debug;

use trident_api::config::{Module, LoadMode};

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

fn update_options(lines: &mut Vec<String>, module_name: &str, options: &std::collections::HashMap<String, String>) {
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
