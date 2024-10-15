use std::{io::Write, path::Path};

use anyhow::{Context, Error};
use log::debug;
use osutils::osmodifier;
use osutils::osmodifier::MICHostname;
use tempfile::NamedTempFile;

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
    osmodifier::run(os_modifier_path, tmpfile.path())
        .context("Failed to run OS modifier to set up hostname")?;

    Ok(())
}
