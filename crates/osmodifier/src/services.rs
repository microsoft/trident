// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Service management — enable and disable systemd services.

use std::process::Command;

use anyhow::{Context, Error};
use log::{debug, warn};

use trident_api::config::Services;

use crate::OsModifierContext;

/// Enable and disable the requested systemd services.
pub fn configure(ctx: &OsModifierContext, services: &Services) -> Result<(), Error> {
    for service in &services.enable {
        enable_service(ctx, service)?;
    }

    for service in &services.disable {
        disable_service(ctx, service)?;
    }

    Ok(())
}

fn enable_service(ctx: &OsModifierContext, service: &str) -> Result<(), Error> {
    debug!("Enabling service '{service}'");
    let root = ctx.root.to_str().unwrap_or("/");

    let output = Command::new("systemctl")
        .args(["--root", root, "enable", service])
        .output()
        .with_context(|| format!("Failed to execute systemctl enable {service}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to enable service '{service}': {stderr}");
    }

    Ok(())
}

fn disable_service(ctx: &OsModifierContext, service: &str) -> Result<(), Error> {
    // Check if the service is enabled first
    let root = ctx.root.to_str().unwrap_or("/");

    let check = Command::new("systemctl")
        .args(["--root", root, "is-enabled", service])
        .output()
        .with_context(|| format!("Failed to check if service '{service}' is enabled"))?;

    if !check.status.success() {
        warn!("Service '{service}' is not enabled, skipping disable");
        return Ok(());
    }

    debug!("Disabling service '{service}'");
    let output = Command::new("systemctl")
        .args(["--root", root, "disable", service])
        .output()
        .with_context(|| format!("Failed to execute systemctl disable {service}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to disable service '{service}': {stderr}");
    }

    Ok(())
}
