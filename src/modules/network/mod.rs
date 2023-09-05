use anyhow::{Context, Error};
use log::info;

use trident_api::{
    config::HostConfiguration,
    status::{HostStatus, UpdateKind},
};

use crate::modules::Module;

mod netplan;
pub mod provisioning;

#[derive(Default, Debug)]
pub struct NetworkModule;
impl Module for NetworkModule {
    fn name(&self) -> &'static str {
        "network"
    }

    fn refresh_host_status(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
        Ok(())
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        Some(UpdateKind::HotPatch)
    }

    fn reconcile(
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
