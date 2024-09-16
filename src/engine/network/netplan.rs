use std::{fs, process::Command};

use anyhow::{Context, Error};
use log::debug;
use netplan_types::NetworkConfig;

use osutils::exe::RunAndCheck;

pub const TRIDENT_NETPLAN_FILE: &str = "/etc/netplan/99-trident.yaml";

pub fn write(data: &str) -> Result<(), Error> {
    debug!("Writing netplan config to {}", TRIDENT_NETPLAN_FILE);
    fs::write(TRIDENT_NETPLAN_FILE, data)
        .with_context(|| format!("Failed to write netplan config to {}", TRIDENT_NETPLAN_FILE))
}

pub fn generate() -> Result<(), Error> {
    debug!("Generating netplan config");
    Command::new("/usr/sbin/netplan")
        .arg("generate")
        .run_and_check()
}

pub fn apply() -> Result<(), Error> {
    debug!("Applying netplan config");
    Command::new("/usr/sbin/netplan")
        .arg("apply")
        .run_and_check()
}

pub fn render_netplan_yaml(value: &NetworkConfig) -> Result<String, Error> {
    #[derive(serde::Serialize)]
    struct NetplanConfig<'a> {
        network: &'a NetworkConfig,
    }

    serde_yaml::to_string(&NetplanConfig { network: value })
        .context("Failed to render netplan yaml")
}
