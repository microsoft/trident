// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! SELinux management — update /etc/selinux/config and GRUB cmdline args.

use std::fs;

use anyhow::{bail, Context, Error};
use log::debug;
use trident_api::config::SelinuxMode;

use crate::{default_grub::DefaultGrub, OsModifierContext};

const SELINUX_CONFIG_PATH: &str = "/etc/selinux/config";

/// Update the SELinux mode in /etc/selinux/config.
pub fn update_config_file(ctx: &OsModifierContext, mode: &SelinuxMode) -> Result<(), Error> {
    let path = ctx.path(SELINUX_CONFIG_PATH);

    if !path.exists() {
        bail!(
            "SELinux config file not found at '{}'. \
             Ensure the selinux-policy package is installed.",
            path.display()
        );
    }

    let content = fs::read_to_string(&path)
        .with_context(|| format!("Failed to read '{}'", path.display()))?;

    let selinux_value = match mode {
        SelinuxMode::Enforcing => "enforcing",
        SelinuxMode::Permissive => "permissive",
        SelinuxMode::Disabled => "disabled",
    };

    // Replace the SELINUX= line
    let re = regex::Regex::new(r"(?m)^SELINUX=.*$")
        .context("Failed to compile SELinux regex")?;

    let new_content = if re.is_match(&content) {
        re.replace(&content, &format!("SELINUX={selinux_value}"))
            .to_string()
    } else {
        // Append if not present
        format!("{content}\nSELINUX={selinux_value}\n")
    };

    debug!(
        "Updating SELinux config at '{}' to '{selinux_value}'",
        path.display()
    );
    fs::write(&path, new_content)
        .with_context(|| format!("Failed to write '{}'", path.display()))
}

/// Update SELinux kernel command line args in the default GRUB config.
///
/// This sets the `selinux` and `enforcing` args in GRUB_CMDLINE_LINUX,
/// matching the Go `UpdateSELinuxCommandLineForEMU` behavior.
pub fn update_grub_cmdline(
    _ctx: &OsModifierContext,
    default_grub: &mut DefaultGrub,
    mode: &SelinuxMode,
) -> Result<(), Error> {
    let new_args = match mode {
        SelinuxMode::Enforcing => vec!["selinux=1".to_string(), "enforcing=1".to_string()],
        SelinuxMode::Permissive => vec!["selinux=1".to_string(), "enforcing=0".to_string()],
        SelinuxMode::Disabled => vec!["selinux=0".to_string()],
    };

    default_grub.update_cmdline_args(&["selinux", "enforcing"], &new_args)
}
