use std::{fs, io, process::Command};

use anyhow::{Context, Error};
use netplan_types::NetworkConfig;

use osutils::exe::OutputChecker;

pub const TRIDENT_NETPLAN_FILE: &str = "/etc/netplan/99-trident.yaml";

pub fn write(data: &str) -> io::Result<()> {
    fs::write(TRIDENT_NETPLAN_FILE, data)
}

pub fn apply() -> Result<(), Error> {
    Command::new("/usr/sbin/netplan")
        .args(["apply"])
        .output()
        .context("Failed to invoke `netplan apply`")?
        .check()
        .context("Executing `netplan apply` failed")
}

pub fn render_netplan_yaml(value: &NetworkConfig) -> Result<String, Error> {
    #[derive(serde::Serialize)]
    struct NetplanConfig<'a> {
        network: &'a NetworkConfig,
    }

    serde_yaml::to_string(&NetplanConfig { network: value })
        .context("failed to render netplan yaml")
}
