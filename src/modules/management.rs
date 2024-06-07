//! Module in charge of configuring the Trident agent on the runtime OS.

use std::{
    fs::{self},
    path::Path,
};

use anyhow::{Context, Error};
use log::{debug, info};
use trident_api::{
    config::{HostConfiguration, HostConfigurationDynamicValidationError, LocalConfigFile},
    status::{HostStatus, ServicingType},
};

use crate::{modules::Module, TRIDENT_BINARY_PATH, TRIDENT_LOCAL_CONFIG_PATH};

#[derive(Default, Debug)]
pub struct ManagementModule;
impl Module for ManagementModule {
    fn name(&self) -> &'static str {
        "management"
    }

    fn validate_host_config(
        &self,
        host_status: &HostStatus,
        host_config: &HostConfiguration,
        _planned_servicing_type: ServicingType,
    ) -> Result<(), HostConfigurationDynamicValidationError> {
        if host_config.trident.disable {
            return Ok(());
        }

        if let Some(ref current_datastore_path) = host_status.trident.datastore_path {
            if current_datastore_path != &host_config.trident.datastore_path {
                return Err(
                    HostConfigurationDynamicValidationError::ChangedDatastorePath {
                        current: current_datastore_path.to_string_lossy().to_string(),
                        new: host_config
                            .trident
                            .datastore_path
                            .to_string_lossy()
                            .to_string(),
                    },
                );
            }
        }

        Ok(())
    }

    fn provision(&mut self, host_status: &mut HostStatus, mount_path: &Path) -> Result<(), Error> {
        if host_status.spec.trident.disable {
            info!("Not provisioning management module as it is disabled");
            return Ok(());
        }

        host_status.trident.datastore_path = Some(host_status.spec.trident.datastore_path.clone());
        debug!("Datastore path: {:?}", host_status.trident.datastore_path);

        if host_status.spec.trident.self_upgrade {
            info!("Copying Trident binary to runtime OS");
            fs::copy(
                TRIDENT_BINARY_PATH,
                mount_path.join(&TRIDENT_BINARY_PATH[1..]),
            )
            .context("Failed to copy Trident binary to runtime OS")?;
        }

        Ok(())
    }

    fn configure(&mut self, host_status: &mut HostStatus, _exec_root: &Path) -> Result<(), Error> {
        if host_status.spec.trident.disable {
            return Ok(());
        }

        fs::create_dir_all(Path::new(TRIDENT_LOCAL_CONFIG_PATH).parent().unwrap())
            .context("Failed to create trident config directory")?;

        let datastore_path = host_status
            .trident
            .datastore_path
            .as_ref()
            .context("Datastore path missing from host status")?;

        create_trident_config(
            datastore_path,
            &host_status.spec,
            Path::new(TRIDENT_LOCAL_CONFIG_PATH),
        )?;
        debug!("Trident config created");

        Ok(())
    }
}

pub(super) fn create_trident_config(
    datastore_path: &Path,
    host_config: &HostConfiguration,
    trident_config_path: &Path,
) -> Result<(), Error> {
    let trident_config = LocalConfigFile::default()
        .with_datastore(datastore_path.to_path_buf())
        .with_phonehome(host_config.trident.phonehome.clone())
        .with_grpc(if host_config.trident.enable_grpc {
            Some(Default::default())
        } else {
            None
        })
        .with_host_configuration(host_config.clone());
    fs::write(
        trident_config_path,
        serde_yaml::to_string(&trident_config).context("Failed to serialize trident config")?,
    )
    .context("Failed to write Trident Configuration")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_host_config() {
        let mgmt_mod = ManagementModule;

        let mut host_status = HostStatus::default();
        let mut host_config = HostConfiguration::default();

        // Initial validation with default values should pass
        mgmt_mod
            .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
            .unwrap();

        // Setting the datastore path should pass
        host_status.trident.datastore_path = Some(Path::new("/foo").to_path_buf());
        host_config.trident.datastore_path = Path::new("/foo").to_path_buf();
        mgmt_mod
            .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
            .unwrap();

        // Different Paths
        host_config.trident.datastore_path = Path::new("/bar").to_path_buf();
        assert_eq!(
            mgmt_mod
                .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
                .unwrap_err(),
            HostConfigurationDynamicValidationError::ChangedDatastorePath {
                current: "/foo".to_string(),
                new: "/bar".to_string()
            }
        );

        // When disabled, should pass
        host_config.trident.disable = true;
        mgmt_mod
            .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
            .unwrap();
    }
}
