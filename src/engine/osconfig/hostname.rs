use std::{io::Write, path::Path, process::Command};

use anyhow::{Context, Error};
use log::debug;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use osutils::exe::RunAndCheck;

#[derive(Serialize, Deserialize)]
struct MICHostname {
    hostname: String,
}

pub(super) fn set_up_hostname(hostname: &str, os_modifier_path: &Path) -> Result<(), Error> {
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
    Command::new(os_modifier_path)
        .arg("--config-file")
        .arg(tmpfile.path())
        .arg("--log-level=debug")
        .run_and_check()
        .context("Failed to run OS modifier")?;

    Ok(())
}
