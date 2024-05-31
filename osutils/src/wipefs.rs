use std::{path::Path, process::Command};

use anyhow::{Context, Error};

use crate::exe::RunAndCheck;

pub fn all(device: impl AsRef<Path>) -> Result<(), Error> {
    Command::new("wipefs")
        .arg("--all")
        .arg(device.as_ref())
        .run_and_check()
        .with_context(|| format!("Failed to wipe device '{}'", device.as_ref().display()))
}
