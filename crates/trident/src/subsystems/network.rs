use std::{fs, path::Path};

use anyhow::Context;
use log::debug;

use osutils::netplan;
use trident_api::{
    error::{ReportError, ServicingError, TridentError},
    status::ServicingType,
};

use crate::engine::{EngineContext, Subsystem, RUNS_ON_ALL};

const CLOUD_INIT_CONFIG_DIR: &str = "/etc/cloud/cloud.cfg.d";
const CLOUD_INIT_DISABLE_FILE: &str = "99-use-trident-networking.cfg";
const CLOUD_INIT_DISABLE_CONTENT: &str = "network: {config: disabled}";

#[derive(Default, Debug)]
pub struct NetworkSubsystem;
impl Subsystem for NetworkSubsystem {
    fn name(&self) -> &'static str {
        "network"
    }

    fn runs_on(&self, _ctx: &EngineContext) -> &[ServicingType] {
        RUNS_ON_ALL
    }

    fn prepare(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.servicing_type == ServicingType::RuntimeUpdate
            && ctx.spec.os.netplan != ctx.spec_old.os.netplan
        {
            // Remove old configuration
            netplan::remove().structured(ServicingError::RemoveNetplanConfig)?;
        }
        Ok(())
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
                disable_cloud_init_networking(CLOUD_INIT_CONFIG_DIR)?;

                // Apply Netplan config immediately since there is no reboot in
                // a runtime update.
                if ctx.servicing_type == ServicingType::RuntimeUpdate {
                    netplan::apply().structured(ServicingError::ApplyNetplanConfig)?;
                }
            }
            None => {
                debug!("Network config not provided");
            }
        }
        Ok(())
    }
}

fn disable_cloud_init_networking(config_dir: impl AsRef<Path>) -> Result<(), TridentError> {
    if !config_dir.as_ref().exists() {
        debug!(
            "Cloud-init config dir {} does not exist, skipping disabling cloud-init networking",
            config_dir.as_ref().display()
        );
        return Ok(());
    }

    debug!("Disabling cloud-init networking");
    let cloud_init_disable_path = config_dir.as_ref().join(CLOUD_INIT_DISABLE_FILE);
    fs::write(&cloud_init_disable_path, CLOUD_INIT_DISABLE_CONTENT)
        .with_context(|| {
            format!(
                "Failed to write to cloud-init disable file at {}",
                cloud_init_disable_path.display()
            )
        })
        .structured(ServicingError::DisableCloudInitNetworking)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disable_cloud_init_networking() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_dir = temp_dir.path();
        disable_cloud_init_networking(config_dir).unwrap();
        let disable_file_path = config_dir.join(CLOUD_INIT_DISABLE_FILE);
        let content = fs::read_to_string(disable_file_path).unwrap();
        assert_eq!(content, CLOUD_INIT_DISABLE_CONTENT);
    }

    #[test]
    fn test_disable_cloud_init_networking_non_existent_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_dir = temp_dir.path().join("non_existent");
        // Should not error even if the directory does not exist
        disable_cloud_init_networking(&config_dir).unwrap();
        assert!(!config_dir.exists());
        assert!(!config_dir.join(CLOUD_INIT_DISABLE_FILE).exists());
        // Check that the temp dir still exists and is empty
        assert!(temp_dir.path().exists());
        assert!(temp_dir.path().read_dir().unwrap().next().is_none());
    }
}
