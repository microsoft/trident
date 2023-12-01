use std::process::Command;

use anyhow::{Context, Error};
use log::{info, warn};
use netplan_types::NetworkConfig;

use osutils::exe::OutputChecker;
use trident_api::config::HostConfiguration;

use super::netplan;

pub fn start(
    override_network: Option<NetworkConfig>,
    host_config: Option<&HostConfiguration>,
    wait_on_network: bool,
) -> Result<(), Error> {
    let netconf = override_network
        .as_ref()
        .or(host_config.and_then(|hc| hc.network_provision.as_ref().or(hc.network.as_ref())));

    match netconf {
        Some(config) => {
            start_provisioning_network(config, wait_on_network)
                .context("Failed to start provisioning network")?;
            info!("Setup of provisioning network complete!");
        }
        None => {
            // TODO: implement
            // Today mariner ships with a decent default to do DHCP on all
            // interfaces, and that seems ok for now.
            info!("Network config not provided");
        }
    };

    Ok(())
}

fn start_provisioning_network(config: &NetworkConfig, wait_on_network: bool) -> Result<(), Error> {
    let config = netplan::render_netplan_yaml(config)
        .context("Failed to render provisioning network netplan yaml")?;
    netplan::write(&config).context("Failed to write provisioning netplan config")?;

    if wait_on_network {
        // We want to be sure we're only waiting on the interfaces we care about, so
        // we have to remove any defaults:
        osutils::files::clean_directory("/etc/systemd/network")
            .context("failed to clean /etc/systemd/network")?;
    }

    // Apply netplan config
    netplan::apply().context("Failed to apply provisioning netplan config")?;

    if wait_on_network {
        warn!("Enabling systemd-networkd-wait-online");
        Command::new("systemctl")
            .arg("start")
            .arg("systemd-networkd-wait-online")
            .arg("--no-block")
            .output()
            .context("Failed to start systemd-networkd-wait-online")?
            .check()
            .context("Failed to enable systemd-networkd-wait-online")?;
    }

    Ok(())
}
