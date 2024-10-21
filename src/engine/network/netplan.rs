use std::fs;

use anyhow::{Context, Error};
use log::debug;
use netplan_types::NetworkConfig;

use osutils::dependencies::Dependency;

pub const TRIDENT_NETPLAN_FILE: &str = "/etc/netplan/99-trident.yaml";

pub fn write(data: &str) -> Result<(), Error> {
    debug!("Writing netplan config to {}", TRIDENT_NETPLAN_FILE);
    fs::write(TRIDENT_NETPLAN_FILE, data)
        .with_context(|| format!("Failed to write netplan config to {}", TRIDENT_NETPLAN_FILE))
}

pub fn generate() -> Result<(), Error> {
    debug!("Generating netplan config");
    Dependency::Netplan.cmd().arg("generate").run_and_check()?;
    Ok(())
}

pub fn apply() -> Result<(), Error> {
    debug!("Applying netplan config");
    Dependency::Netplan.cmd().arg("apply").run_and_check()?;
    Ok(())
}

pub fn render_netplan_yaml(value: &NetworkConfig) -> Result<String, Error> {
    #[derive(serde::Serialize)]
    struct NetplanConfig<'a> {
        network: &'a NetworkConfig,
    }

    serde_yaml::to_string(&NetplanConfig { network: value })
        .context("Failed to render netplan yaml")
}
