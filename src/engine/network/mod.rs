use std::path::Path;

use log::info;

use trident_api::{
    error::{ReportError, ServicingError, TridentError},
    status::HostStatus,
};

use crate::engine::Subsystem;

mod netplan;
pub mod provisioning;

#[derive(Default, Debug)]
pub struct NetworkSubsystem;
impl Subsystem for NetworkSubsystem {
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
                info!("Configuring network");
                let config = netplan::render_netplan_yaml(config)
                    .structured(ServicingError::RenderNetworkNetplanYaml)?;
                netplan::write(&config).structured(ServicingError::WriteNetplanConfig)?;
                netplan::generate().structured(ServicingError::GenerateNetplanConfig)?;
            }
            None => {
                info!("Network config not provided");
            }
        }
        Ok(())
    }
}
