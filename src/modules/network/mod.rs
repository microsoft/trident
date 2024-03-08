use anyhow::{Context, Error};
use log::info;

use trident_api::{config::HostConfiguration, status::HostStatus};

use crate::modules::Module;

mod netplan;
pub mod provisioning;

#[derive(Default, Debug)]
pub struct NetworkModule;
impl Module for NetworkModule {
    fn name(&self) -> &'static str {
        "network"
    }

    fn configure(
        &mut self,
        _host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        match host_config.network.as_ref() {
            Some(config) => {
                let config = netplan::render_netplan_yaml(config)
                    .context("failed to render runtime network netplan yaml")?;
                netplan::write(&config).context("failed to write netplan config")?;
                netplan::apply().context("failed to apply netplan config")?;
            }
            None => {
                info!("Network config not provided");
            }
        }
        Ok(())
    }
}
