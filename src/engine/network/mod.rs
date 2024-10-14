use std::path::Path;

use log::info;

use trident_api::error::{ReportError, ServicingError, TridentError};

use crate::engine::Subsystem;

use super::EngineContext;

mod netplan;
pub mod provisioning;

#[derive(Default, Debug)]
pub struct NetworkSubsystem;
impl Subsystem for NetworkSubsystem {
    fn name(&self) -> &'static str {
        "network"
    }

    #[tracing::instrument(name = "network_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext, _exec_root: &Path) -> Result<(), TridentError> {
        match ctx.spec.os.network.as_ref() {
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
