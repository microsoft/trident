use anyhow::{Context, Error};
use log::info;
use netplan_types::NetworkConfig;

use trident_api::config::HostConfiguration;

use super::netplan;

pub fn start(
    override_network: Option<NetworkConfig>,
    host_config: Option<&HostConfiguration>,
) -> Result<(), Error> {
    let netconf = override_network
        .as_ref()
        .or(host_config.and_then(|hc| hc.network_provision.as_ref().or(hc.network.as_ref())));

    match netconf {
        Some(config) => {
            let config = netplan::render_netplan_yaml(config)
                .context("failed to render provisioning network netplan yaml")?;
            netplan::write(&config).context("failed to write provisioning netplan config")?;
            netplan::apply().context("failed to apply provisioning netplan config")?;
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
