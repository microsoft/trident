use std::fs;

use anyhow::Context;
use log::debug;

use osutils::netplan;
use trident_api::error::{ReportError, ServicingError, TridentError};

use crate::engine::{EngineContext, Subsystem};

const CLOUD_INIT_DISABLE_FILE: &str = "/etc/cloud/cloud.cfg.d/99-use-trident-networking.cfg";
const CLOUD_INIT_DISABLE_CONTENT: &str = "network: {config: disabled}";

#[derive(Default, Debug)]
pub struct NetworkSubsystem;
impl Subsystem for NetworkSubsystem {
    fn name(&self) -> &'static str {
        "network"
    }

    #[tracing::instrument(name = "network_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        match ctx.spec.os.netplan.as_ref() {
            Some(config) => {
                debug!("Configuring network");
                netplan::write(config).structured(ServicingError::WriteNetplanConfig)?;
                netplan::generate().structured(ServicingError::GenerateNetplanConfig)?;

                // We need to disable cloud-init's network configuration when
                // Trident is configuring the network, otherwise cloud-init may
                // deploy additional configurations that are undesired and may
                // conflict with or otherwise affect Trident's network setup.
                disable_cloud_init_networking()?;
            }
            None => {
                debug!("Network config not provided");
            }
        }
        Ok(())
    }
}

fn disable_cloud_init_networking() -> Result<(), TridentError> {
    fs::write(CLOUD_INIT_DISABLE_FILE, CLOUD_INIT_DISABLE_CONTENT)
        .with_context(|| {
            format!("Failed to write to cloud-init disable file at {CLOUD_INIT_DISABLE_FILE}")
        })
        .structured(ServicingError::DisableCloudInitNetworking)
}
