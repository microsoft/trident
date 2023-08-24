use anyhow::Context;
use log::info;

use super::netplan;

use anyhow::Error;

pub fn start(
    override_network: Option<serde_yaml::Value>,
    network_provision: Option<serde_yaml::Value>,
    network: Option<serde_yaml::Value>,
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
