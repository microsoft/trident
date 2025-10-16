use std::{fs, path::Path};

use anyhow::Context;
use log::debug;

use osutils::netplan;
use trident_api::error::{ReportError, ServicingError, TridentError};

use crate::engine::{EngineContext, Subsystem};

const CLOUD_INIT_CONFIG_DIR: &str = "/etc/cloud/cloud.cfg.d";
const CLOUD_INIT_DISABLE_FILE: &str = "99-use-trident-networking.cfg";
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
    let cloud_init_disable_path = Path::new(CLOUD_INIT_CONFIG_DIR).join(CLOUD_INIT_DISABLE_FILE);
    if !Path::new(CLOUD_INIT_CONFIG_DIR).exists() {
        debug!(
            "Cloud-init config dir {} does not exist, skipping disabling cloud-init networking",
            cloud_init_disable_path.display()
        );
        return Ok(());
    }

    debug!("Disabling cloud-init networking");
    fs::write(&cloud_init_disable_path, CLOUD_INIT_DISABLE_CONTENT)
        .with_context(|| {
            format!(
                "Failed to write to cloud-init disable file at {}",
                cloud_init_disable_path.display()
            )
        })
        .structured(ServicingError::DisableCloudInitNetworking)
}
