use std::path::Path;

use anyhow::{Context, Error};

use crate::dependencies::Dependency;

pub fn all(device: impl AsRef<Path>) -> Result<(), Error> {
    Dependency::Wipefs
        .cmd()
        .arg("--all")
        .arg(device.as_ref())
        .run_and_check()
        .with_context(|| format!("Failed to wipe device '{}'", device.as_ref().display()))
}
