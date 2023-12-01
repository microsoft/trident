use std::{fs, process::Command};

use anyhow::{Context, Error};
use log::debug;
use netplan_types::NetworkConfig;

use osutils::exe::OutputChecker;

pub const TRIDENT_NETPLAN_FILE: &str = "/etc/netplan/99-trident.yaml";

pub fn write(data: &str) -> Result<(), Error> {
    debug!("Writing netplan config to {}", TRIDENT_NETPLAN_FILE);
    fs::write(TRIDENT_NETPLAN_FILE, data).context(format!(
        "Failed to write netplan config to {}",
        TRIDENT_NETPLAN_FILE
    ))
}

pub fn apply() -> Result<(), Error> {
    debug!("Applying netplan config");
    Command::new("/usr/sbin/netplan")
        .arg("apply")
        .output()
        .context("Failed to start netplan")?
        .check()
        .context("Executing `netplan apply` failed")
}

pub fn render_netplan_yaml(value: &NetworkConfig) -> Result<String, Error> {
    #[derive(serde::Serialize)]
    struct NetplanConfig<'a> {
        network: &'a NetworkConfig,
    }

    serde_yaml::to_string(&NetplanConfig { network: value })
        .context("Failed to render netplan yaml")
}
