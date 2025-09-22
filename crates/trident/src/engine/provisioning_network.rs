use anyhow::{Context, Error};
use log::{info, warn};
use netplan_types::NetworkConfig;

use osutils::{dependencies::Dependency, netplan};
use trident_api::config::HostConfiguration;

pub fn start(host_config: &HostConfiguration) -> Result<(), Error> {
    let netconf = host_config
        .management_os
        .netplan
        .as_ref()
        .or(host_config.os.netplan.as_ref());

    match netconf {
        Some(config) => {
            start_provisioning_network(config, false)
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
    netplan::write(config).context("Failed to write provisioning netplan config")?;

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
        Dependency::Systemctl
            .cmd()
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
