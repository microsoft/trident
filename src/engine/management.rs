//! Module in charge of configuring the Trident agent on the runtime OS.

use std::{
    fs::{self},
    path::Path,
};

use anyhow::{Context, Error};
use log::{debug, info};
use osutils::path;
use trident_api::{
    config::{HostConfiguration, HostConfigurationDynamicValidationError, LocalConfigFile},
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
    status::{HostStatus, ServicingType},
};

use crate::{engine::Module, TRIDENT_BINARY_PATH, TRIDENT_LOCAL_CONFIG_PATH};

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
        planned_servicing_type: ServicingType,
    ) -> Result<(), TridentError> {
        if host_config.trident.disable {
            return Ok(());
        }

        // Changing the datastore path is only allowed in clean installs.
        if planned_servicing_type != ServicingType::CleanInstall {
            let current_path = &host_status.spec.trident.datastore_path;
            let new_path = &host_config.trident.datastore_path;
            if current_path != new_path {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::DatastorePathChanged {
                        current: current_path.display().to_string(),
                        new: new_path.display().to_string(),
                    },
                )));
            }
        }

        Ok(())
    }

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        exec_root: &Path,
    ) -> Result<(), TridentError> {
        if host_status.spec.trident.disable {
            return Ok(());
        }

        if host_status.spec.trident.self_upgrade {
            info!("Copying Trident binary to runtime OS");
            fs::copy(
                path::join_relative(exec_root, TRIDENT_BINARY_PATH),
                TRIDENT_BINARY_PATH,
            )
            .structured(ServicingError::CopyTridentBinary)?;
        }

        fs::create_dir_all(Path::new(TRIDENT_LOCAL_CONFIG_PATH).parent().unwrap())
            .structured(ServicingError::CreateTridentConfigDirectory)?;

        create_trident_config(
            &host_status.spec.trident.datastore_path,
            &host_status.spec,
            Path::new(TRIDENT_LOCAL_CONFIG_PATH),
        )
        .structured(ServicingError::CreateTridentConfig)?;
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

    use trident_api::error::ErrorKind;

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
        host_status.spec.trident.datastore_path = Path::new("/foo").into();
        host_config.trident.datastore_path = Path::new("/foo").to_path_buf();
        mgmt_mod
            .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
            .unwrap();

        // Default pathbuf (happens on clean install)
        host_config.trident.datastore_path = Default::default();
        mgmt_mod
            .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
            .unwrap();

        // Different paths
        host_config.trident.datastore_path = Path::new("/bar").to_path_buf();
        assert_eq!(
            mgmt_mod
                .validate_host_config(&host_status, &host_config, ServicingType::AbUpdate)
                .unwrap_err()
                .kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::DatastorePathChanged {
                    current: "/foo".to_string(),
                    new: "/bar".to_string(),
                }
            })
        );

        // When disabled, should pass
        host_config.trident.disable = true;
        mgmt_mod
            .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
            .unwrap();
    }
}
