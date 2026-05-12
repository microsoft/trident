// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Hostname management — writes /etc/hostname.

use std::fs;

use anyhow::{Context, Error};
use log::debug;

use crate::OsModifierContext;

const HOSTNAME_PATH: &str = "/etc/hostname";

/// Write the hostname to /etc/hostname.
pub fn update(ctx: &OsModifierContext, hostname: &str) -> Result<(), Error> {
    let path = ctx.path(HOSTNAME_PATH);
    debug!("Writing hostname '{}' to '{}'", hostname, path.display());
    fs::write(&path, hostname)
        .with_context(|| format!("Failed to write hostname to '{}'", path.display()))
}
