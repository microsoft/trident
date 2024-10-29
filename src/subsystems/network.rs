use std::path::Path;

use log::debug;

use osutils::netplan;
use trident_api::error::{ReportError, ServicingError, TridentError};

use crate::engine::{EngineContext, Subsystem};

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
                debug!("Configuring network");
                netplan::write(config).structured(ServicingError::WriteNetplanConfig)?;
                netplan::generate().structured(ServicingError::GenerateNetplanConfig)?;
            }
            None => {
                debug!("Network config not provided");
            }
        }
        Ok(())
    }
}
