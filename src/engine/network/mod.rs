use log::info;
use std::path::Path;

use trident_api::{
    error::{ReportError, ServicingError, TridentError},
    status::HostStatus,
};

use crate::engine::Module;

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
        host_status: &HostStatus,
        _exec_root: &Path,
    ) -> Result<(), TridentError> {
        match host_status.spec.os.network.as_ref() {
            Some(config) => {
                let config = netplan::render_netplan_yaml(config)
                    .structured(ServicingError::RenderNetworkNetplanYaml)?;
                netplan::write(&config).structured(ServicingError::WriteNetplanConfig)?;
                netplan::apply().structured(ServicingError::ApplyNetplanConfig)?;
            }
            None => {
                info!("Network config not provided");
            }
        }
        Ok(())
    }
}
