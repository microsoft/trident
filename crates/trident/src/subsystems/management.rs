//! Subsystem in charge of configuring the Trident agent on the target OS.

use std::{
    fs::{self},
    path::Path,
};

use log::info;

use osutils::path;
use trident_api::{
    config::HostConfigurationDynamicValidationError,
    constants::{
        internal_params::SELF_UPGRADE_TRIDENT, AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT,
    },
    error::{InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::ServicingType,
};

use crate::{
    engine::{EngineContext, Subsystem},
    LAUNCHER_BINARY_PATH, TRIDENT_BINARY_PATH,
};

#[derive(Default, Debug)]
pub struct ManagementSubsystem;
impl Subsystem for ManagementSubsystem {
    fn name(&self) -> &'static str {
        "management"
    }

    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.spec.trident.disable {
            return Ok(());
        }

        // Changing the datastore path is only allowed in clean installs.
        if ctx.servicing_type != ServicingType::CleanInstall {
            let current_path = &ctx.spec_old.trident.datastore_path;
            let new_path = &ctx.spec.trident.datastore_path;
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

    #[tracing::instrument(name = "management_provision", skip_all)]
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        if ctx.spec.trident.disable {
            return Ok(());
        }

        if ctx.spec.internal_params.get_flag(SELF_UPGRADE_TRIDENT) {
            info!("Copying Trident binary to target OS");
            fs::copy(
                TRIDENT_BINARY_PATH,
                path::join_relative(mount_path, TRIDENT_BINARY_PATH),
            )
            .structured(ServicingError::CopyTridentBinary)?;

            if Path::new(LAUNCHER_BINARY_PATH).exists() {
                fs::copy(
                    LAUNCHER_BINARY_PATH,
                    path::join_relative(mount_path, LAUNCHER_BINARY_PATH),
                )
                .structured(ServicingError::CopyTridentBinary)?;
            }
        }

        Ok(())
    }

    #[tracing::instrument(name = "management_configure", skip_all)]
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        configure_agent_config(
            AGENT_CONFIG_PATH,
            &ctx.spec.trident.datastore_path,
            ctx.storage_graph.root_fs_is_verity(),
        )
    }
}

fn configure_agent_config(
    agent_config_path: &str,
    datastore_path: &Path,
    is_root_verity: bool,
) -> Result<(), TridentError> {
    // Ensure that Trident agent config exists with correct datastore path
    if Path::new(agent_config_path).exists() {
        // If the agent config exists, check that the datastore matches the expected path.
        if let Ok(contents) = std::fs::read_to_string(agent_config_path) {
            let mut datastore_path_line_present = false;
            let mut datastore_path_configured = TRIDENT_DATASTORE_PATH_DEFAULT;
            for line in contents.lines() {
                if let Some(path) = line.strip_prefix("DatastorePath=") {
                    datastore_path_line_present = true;
                    datastore_path_configured = path.trim();
                    break;
                }
            }

            if datastore_path_line_present {
                // An explicit DatastorePath= line is present: it must match
                // the expected path exactly.
                if datastore_path != Path::new(datastore_path_configured) {
                    return Err(TridentError::new(
                        InvalidInputError::ImageBadAgentConfiguration,
                    ))
                    .message(format!(
                        "Datastore path in agent config ({}) does not match expected path ({})",
                        datastore_path_configured,
                        datastore_path.display()
                    ));
                }
            } else if datastore_path != Path::new(TRIDENT_DATASTORE_PATH_DEFAULT) {
                // No DatastorePath= line: the file may still carry other
                // settings (e.g. Telemetry=) that must be preserved. Missing
                // DatastorePath only implies the default path, so if a
                // non-default path is expected, merge a DatastorePath= line
                // into the existing file rather than treating it as a
                // mismatch.
                if is_root_verity {
                    // For root-verity, do not attempt to modify the agent config.
                    return Err(TridentError::new(
                        InvalidInputError::ImageBadAgentConfiguration,
                    ))
                    .message(
                        "Agent configuration file does not set a non-default datastore path \
                         and root filesystem is verity",
                    );
                }

                let mut updated_contents = contents;
                if !updated_contents.is_empty() && !updated_contents.ends_with('\n') {
                    updated_contents.push('\n');
                }
                updated_contents.push_str(&format!("DatastorePath={}\n", datastore_path.display()));
                fs::write(agent_config_path, updated_contents).structured(
                    ServicingError::CreateConfigurationFile {
                        path: agent_config_path.into(),
                    },
                )?;
            }
        }
    } else if datastore_path != Path::new(TRIDENT_DATASTORE_PATH_DEFAULT) {
        // Only attempt to create the agent config if the datastore path is not the default.

        if is_root_verity {
            // For root-verity, do not attempt to create the agent config.
            return Err(TridentError::new(
                InvalidInputError::ImageBadAgentConfiguration,
            ))
            .message("Agent configuration file does not exist and root filesystem is verity");
        }

        let datastore_configuration = format!("DatastorePath={}", datastore_path.display());
        fs::write(agent_config_path, datastore_configuration).structured(
            ServicingError::CreateConfigurationFile {
                path: agent_config_path.into(),
            },
        )?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use trident_api::{
        config::{HostConfiguration, HostConfigurationDynamicValidationError},
        error::ErrorKind,
    };

    #[test]
    fn test_validate_host_config() {
        let mgmt_mod = ManagementSubsystem;

        let mut ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };
        ctx.spec = HostConfiguration::default();

        // Initial validation with default values should pass
        mgmt_mod.validate_host_config(&ctx).unwrap();

        // Setting the datastore path should pass
        ctx.spec_old.trident.datastore_path = Path::new("/foo").into();
        ctx.spec.trident.datastore_path = Path::new("/foo").to_path_buf();
        mgmt_mod.validate_host_config(&ctx).unwrap();

        // Default pathbuf (happens on clean install)
        ctx.spec.trident.datastore_path = Default::default();
        mgmt_mod.validate_host_config(&ctx).unwrap();

        // Different paths
        ctx.spec.trident.datastore_path = Path::new("/bar").to_path_buf();
        ctx.servicing_type = ServicingType::AbUpdate;
        assert_eq!(
            mgmt_mod.validate_host_config(&ctx).unwrap_err().kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::DatastorePathChanged {
                    current: "/foo".to_string(),
                    new: "/bar".to_string(),
                }
            })
        );

        // When disabled, should pass
        ctx.spec.trident.disable = true;
        ctx.servicing_type = ServicingType::CleanInstall;
        mgmt_mod.validate_host_config(&ctx).unwrap();
    }

    #[test]
    fn test_configure_host_config() {
        {
            // Default datastore path, agent does not exist
            let agent_config_folder = tempfile::tempdir().unwrap();
            let agent_config_path = agent_config_folder.path().join("trident.conf");
            configure_agent_config(
                &agent_config_path.to_string_lossy(),
                Path::new(TRIDENT_DATASTORE_PATH_DEFAULT),
                false,
            )
            .unwrap();
            // agent config should not be created
            assert!(!agent_config_path.exists());
        }

        let nonstandard_datastore_path = "/var/lib/trident/nonstandard-datastore.sqlite";
        {
            // Non-standard datastore path, agent config does not exist
            let agent_config_folder = tempfile::tempdir().unwrap();
            let agent_config_path = agent_config_folder.path().join("trident.conf");
            configure_agent_config(
                &agent_config_path.to_string_lossy(),
                Path::new(nonstandard_datastore_path),
                false,
            )
            .unwrap();
            // Agent config should be created with non-standard datastore path
            let contents = std::fs::read_to_string(agent_config_path).unwrap();
            print!("Contents of agent config file:\n{contents}");
            let expected_contents = format!("DatastorePath={nonstandard_datastore_path}");
            assert!(contents.contains(expected_contents.as_str()));
        }

        {
            // Non-standard datastore path, agent config does not exist, root-verity
            let agent_config_folder = tempfile::tempdir().unwrap();
            let agent_config_path = agent_config_folder.path().join("trident.conf");
            configure_agent_config(
                &agent_config_path.to_string_lossy(),
                Path::new(nonstandard_datastore_path),
                true,
            )
            .unwrap_err();
        }

        {
            // Agent config exists with matching datastore path
            let agent_config_folder = tempfile::tempdir().unwrap();
            let agent_config_path = agent_config_folder.path().join("trident.conf");
            fs::write(
                &agent_config_path,
                format!("DatastorePath={nonstandard_datastore_path}"),
            )
            .unwrap();

            configure_agent_config(
                &agent_config_path.to_string_lossy(),
                Path::new(nonstandard_datastore_path),
                false,
            )
            .unwrap();
        }

        {
            // agent config exists with mismatched datastore path
            let mismatched_datastore_path = "/var/lib/trident/mismatched-datastore.sqlite";
            let agent_config_folder = tempfile::tempdir().unwrap();
            let agent_config_path = agent_config_folder.path().join("trident.conf");
            fs::write(
                &agent_config_path,
                format!("DatastorePath={mismatched_datastore_path}"),
            )
            .unwrap();

            configure_agent_config(
                &agent_config_path.to_string_lossy(),
                Path::new(nonstandard_datastore_path),
                false,
            )
            .unwrap_err();
        }

        {
            // Regression test: agent config exists but only carries other
            // settings (e.g. Telemetry=OptIn), with no DatastorePath= line.
            // A non-default datastore path must be merged in, preserving the
            // existing settings, rather than treated as a mismatch.
            let agent_config_folder = tempfile::tempdir().unwrap();
            let agent_config_path = agent_config_folder.path().join("trident.conf");
            fs::write(
                &agent_config_path,
                "Telemetry=OptIn
",
            )
            .unwrap();

            configure_agent_config(
                &agent_config_path.to_string_lossy(),
                Path::new(nonstandard_datastore_path),
                false,
            )
            .unwrap();

            let contents = std::fs::read_to_string(&agent_config_path).unwrap();
            assert!(contents.contains("Telemetry=OptIn"));
            assert!(contents.contains(&format!("DatastorePath={nonstandard_datastore_path}")));
        }

        {
            // Same as above, but root-verity: must not attempt to modify
            // the agent config, and must error like the "file does not
            // exist" root-verity case.
            let agent_config_folder = tempfile::tempdir().unwrap();
            let agent_config_path = agent_config_folder.path().join("trident.conf");
            fs::write(
                &agent_config_path,
                "Telemetry=OptIn
",
            )
            .unwrap();

            configure_agent_config(
                &agent_config_path.to_string_lossy(),
                Path::new(nonstandard_datastore_path),
                true,
            )
            .unwrap_err();

            // The file must be left untouched.
            let contents = std::fs::read_to_string(&agent_config_path).unwrap();
            assert_eq!(
                contents,
                "Telemetry=OptIn
"
            );
        }
    }
}
