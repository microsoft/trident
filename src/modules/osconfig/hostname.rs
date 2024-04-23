use std::{io::Write, process::Command};

use crate::OS_MODIFIER_BINARY_PATH;
use anyhow::{Context, Error};
use log::debug;
use osutils::exe::RunAndCheck;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

#[derive(Serialize, Deserialize)]
struct MICHostname {
    hostname: String,
}

pub(super) fn set_up_hostname(hostname: &str) -> Result<(), Error> {
    debug!("Setting up hostname");

    let mic_hostname_config = MICHostname {
        hostname: hostname.to_string(),
    };

    let mic_hostname_yaml = serde_yaml::to_string(&mic_hostname_config)?;
    let mut tmpfile = NamedTempFile::new().context("Failed to create a temporary file")?;
    tmpfile
        .write_all(mic_hostname_yaml.as_bytes())
        .context("Failed to write MIC hostname YAML to temporary file")?;
    tmpfile.flush().context("Failed to flush temporary file")?;

    // Invoke os modifier with the hostname config file
    Command::new(OS_MODIFIER_BINARY_PATH)
        .arg("--config-file")
        .arg(tmpfile.path())
        .arg("--log-level=debug")
        .run_and_check()
        .context("Failed to run OS modifier")?;

    Ok(())
}
