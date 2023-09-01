use anyhow::{Context, Error};
use log::info;
use netplan_types::NetworkConfig;

use super::netplan;

pub fn start(
    override_network: Option<NetworkConfig>,
    network_provision: Option<NetworkConfig>,
    network: Option<NetworkConfig>,
) -> Result<(), Error> {
    let custom_config = override_network.or(network_provision).or(network);

    match custom_config {
        Some(config) => {
            let config = netplan::render_netplan_yaml(&config)
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
