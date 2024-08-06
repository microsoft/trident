use log::info;
use std::path::Path;

use trident_api::{
    error::{ManagementError, ReportError, TridentError},
    status::HostStatus,
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

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        _exec_root: &Path,
    ) -> Result<(), TridentError> {
        match host_status.spec.os.network.as_ref() {
            Some(config) => {
                let config = netplan::render_netplan_yaml(config)
                    .structured(ManagementError::RenderNetworkNetplanYaml)?;
                netplan::write(&config).structured(ManagementError::WriteNetplanConfig)?;
                netplan::apply().structured(ManagementError::ApplyNetplanConfig)?;
            }
            None => {
                info!("Network config not provided");
            }
        }
        Ok(())
    }
}
