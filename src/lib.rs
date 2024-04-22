use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Error};
use log::{debug, error, info, warn};
use nix::unistd::Uid;
use tokio::sync::mpsc::{self};

use osutils::{container, path};
use setsail::KsTranslator;
use trident_api::config::{
    HostConfiguration, HostConfigurationSource, InvalidHostConfigurationError, LocalConfigFile,
    Operations,
};
use trident_api::error::{
    ExecutionEnvironmentMisconfigurationError, InitializationError, InternalError,
    InvalidInputError, ManagementError, ReportError, TridentError, TridentResultExt,
};
use trident_api::status::{HostStatus, ReconcileState};

use crate::datastore::DataStore;
use crate::modules::bootentries;

mod datastore;
mod logging;
mod modules;
mod orchestrate;

#[cfg(feature = "grpc-dangerous")]
mod grpc;

pub use logging::{
    background_log::BackgroundLog, logstream::Logstream, multilog::MultiLogger,
    tracestream::TraceStream,
};
pub use modules::network::provisioning::start as start_provisioning_network;
pub use orchestrate::OrchestratorConnection;

/// Trident version as provided by environment variables at build time
pub const TRIDENT_VERSION: &str = match option_env!("TRIDENT_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Default Trident configuration file path.
pub const TRIDENT_LOCAL_CONFIG_PATH: &str = "/etc/trident/config.yaml";

/// Default Trident datastore path. Used from the runtime OS.
pub const TRIDENT_DATASTORE_PATH: &str = "/var/lib/trident/datastore.sqlite";

/// Location to store the Trident datastore on the provisioning OS.
pub const TRIDENT_TEMPORARY_DATASTORE_PATH: &str = "/var/lib/trident/tmp-datastore.sqlite";

/// Trident binary path.
pub const TRIDENT_BINARY_PATH: &str = "/usr/bin/trident";

/// OS Modifier (EMU) binary path.
pub const OS_MODIFIER_BINARY_PATH: &str = "/usr/bin/osmodifier";

/// Trident background log path.
pub const TRIDENT_BACKGROUND_LOG_PATH: &str = "/var/log/trident-full.log";

/// A command to update the host configuration.
///
/// This struct is used to communicate between the gRPC server and the main trident thread. It
/// includes information on the command to execute, as well as a tokio Sender which the main thread
/// can use to stream status updates back to the gRPC client.
struct HostUpdateCommand {
    allowed_operations: Operations,
    host_config: HostConfiguration,
    #[cfg(feature = "grpc-dangerous")]
    sender: Option<mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>>,
}

pub struct Trident {
    config: LocalConfigFile,
    #[allow(unused)]
    server_runtime: Option<tokio::runtime::Runtime>,
}
impl Trident {
    pub fn new(
        config_path: Option<PathBuf>,
        logstream: Logstream,
        tracestream: TraceStream,
    ) -> Result<Self, TridentError> {
        let config_path = if let Some(path) = config_path {
            path.to_owned()
        } else if path::host_relative(TRIDENT_LOCAL_CONFIG_PATH).exists() {
            path::host_relative(TRIDENT_LOCAL_CONFIG_PATH)
        } else {
            Path::new(TRIDENT_LOCAL_CONFIG_PATH).to_owned()
        };

        // Load the config file
        info!("Loading config from '{}'", config_path.display());
        let config_contents =
            fs::read_to_string(&config_path).structured(InitializationError::LoadLocalConfig)?;

        // Parse the config file
        let config: LocalConfigFile = match serde_yaml::from_str(&config_contents)
            .structured(InitializationError::ParseLocalConfig)
        {
            Ok(config) => config,
            Err(e) => {
                warn!("{e:?}");

                // If parsing the config file failed, maybe we can still understand enough of it to
                // extract the phonehome URL.
                if let Some(url) = config_contents
                    .lines()
                    .find(|l| l.starts_with("phonehome:"))
                    .map(|l| l[10..].trim())
                    .filter(|l| reqwest::Url::parse(l).is_ok())
                {
                    if let Some(o) = OrchestratorConnection::new(url.to_string()) {
                        o.report_error(format!("{e:?}"), None)
                    }
                }
                return Err(e);
            }
        };

        // Set up logstream if configured
        if let Some(url) = config.logstream.as_ref() {
            logstream
                .set_server(url.to_string())
                .structured(InitializationError::StartLogstream)?;
        }

        // Set up tracestream if configured, using phonehome url for now
        if let Some(url) = config.phonehome.as_ref() {
            let trace_url = url.clone().replace("phonehome", "tracestream");
            tracestream
                .set_server(trace_url)
                .structured(InitializationError::StartTraceStream)?;
        }

        debug!(
            "Trident config:\n{}",
            serde_yaml::to_string(&config).unwrap_or("Failed to serialize host config".into())
        );

        Ok(Self {
            config,
            server_runtime: None,
        })
    }

    fn get_host_configuration(
        config: &LocalConfigFile,
    ) -> Result<Option<Box<HostConfiguration>>, TridentError> {
        config
            .get_host_configuration_source()
            .structured(InvalidInputError::InvalidHostConfiguration(
                InvalidHostConfigurationError::FailedToParse,
            ))?
            .as_ref()
            .map(Self::load_host_config)
            .transpose()
    }

    fn load_host_config(
        source: &HostConfigurationSource,
    ) -> Result<Box<HostConfiguration>, TridentError> {
        let host_config = match source {
            HostConfigurationSource::File(path) => {
                info!("Loading host config from '{}'", path.display());

                serde_yaml::from_str(&fs::read_to_string(path).structured(
                    InvalidInputError::LoadHostConfiguration {
                        path: path.display().to_string(),
                    },
                )?)
                .structured(InvalidInputError::ParseHostConfiguration)?
            }
            HostConfigurationSource::Embedded(contents) => contents.clone(),
            HostConfigurationSource::KickstartEmbedded(contents) => Box::new(
                KsTranslator::new()
                    .run_pre_scripts(true)
                    .translate(setsail::load_kickstart_string(contents))
                    .structured(InvalidInputError::KickstartTranslation)?,
            ),
            HostConfigurationSource::KickstartFile(ref file) => Box::new(
                KsTranslator::new()
                    .run_pre_scripts(true)
                    .translate(setsail::load_kickstart_file(file).structured(
                        InvalidInputError::LoadKickstart {
                            path: file.display().to_string(),
                        },
                    )?)
                    .structured(InvalidInputError::KickstartTranslation)?,
            ),
        };

        debug!(
            "Host config:\n{}",
            serde_yaml::to_string(&host_config).unwrap_or("Failed to serialize host config".into())
        );

        Ok(host_config)
    }

    pub fn start_network(&mut self) -> Result<(), TridentError> {
        // If we have kickstart it means we don't have networking config readily available. We
        // _could_ try parsing now, but we are in an early stage of boot and we want to parse on a
        // later stage so %pre scripts can run and do their thing. It would also mean parsing twice,
        // unless we updated the config file in place. That sounds like a can of worms and we still
        // have the issue about being too early.
        if let Some(
            HostConfigurationSource::KickstartFile(_)
            | HostConfigurationSource::KickstartEmbedded(_),
        ) = self.config.get_host_configuration_source().structured(
            InvalidInputError::InvalidHostConfiguration(
                InvalidHostConfigurationError::FailedToParse,
            ),
        )? {
            warn!("Cannot set up network early when using kickstart");
            return Ok(());
        }

        let host_config = Self::get_host_configuration(&self.config)?;

        info!("Starting network");
        start_provisioning_network(
            self.config.network_override.clone(),
            host_config.as_deref(),
            self.config.wait_for_provisioning_network,
        )
        .structured(ManagementError::StartNetwork)?;

        Ok(())
    }

    pub fn run(&mut self) -> Result<(), TridentError> {
        let orchestrator = self
            .config
            .phonehome
            .as_ref()
            .and_then(|url| OrchestratorConnection::new(url.clone()));

        info!("Running Trident version: {}", TRIDENT_VERSION);

        if !Uid::effective().is_root() {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::MissingRequiredPermissions,
            ));
        }

        // This creates a channel to send commands to the main trident thread. It lets us use the
        // same logic for processing an initial provision command contained within the trident local
        // config as for processing commands received from the gRPC endpoint.
        let (sender, receiver) = tokio::sync::mpsc::channel(1);

        // If we have a host config source, load it and dispatch it as the first
        // command.
        if let Some(host_config) = Self::get_host_configuration(&self.config)? {
            info!("Applying host configuration from local config");
            sender
                .blocking_send(HostUpdateCommand {
                    allowed_operations: self.config.allowed_operations,
                    host_config: *host_config,
                    #[cfg(feature = "grpc-dangerous")]
                    sender: None,
                })
                .structured(InternalError::Internal(
                    "Failed to enqueue provision command",
                ))?;
        }

        if !cfg!(feature = "grpc-dangerous") || self.config.grpc.is_none() {
            // If no gRPC connection details were provided, drop the sender side of the channel.
            // This causes the loop below will exit immediately after processing the initial command
            // that was enqueued above.
            drop(sender);
        } else if let Some(_grpc) = &self.config.grpc {
            #[cfg(feature = "grpc-dangerous")]
            {
                self.server_runtime = Some(grpc::start(_grpc, orchestrator.as_ref(), sender)?);
            }
        }

        let host_status = self.handle_commands(receiver, &orchestrator)?;

        if let Some(ref orchestrator) = orchestrator {
            orchestrator.report_success(Some(
                serde_yaml::to_string(&host_status)
                    .unwrap_or("Failed to serialize host status".into()),
            ))
        }

        Ok(())
    }

    fn handle_commands(
        &mut self,
        mut receiver: mpsc::Receiver<HostUpdateCommand>,
        orchestrator: &Option<OrchestratorConnection>,
    ) -> Result<HostStatus, TridentError> {
        info!("Handling commands");
        let mut datastore = match self.config.datastore {
            Some(ref datastore_path) => DataStore::open(datastore_path)?,
            None => DataStore::open_temporary().message("Failed to open temporary datastore")?,
        };

        // Process commands. Starting with the initial command indicated in the local config file
        // (if any). Once that has been handled, subsequent commands are received from the gRPC
        // endpoint.
        while let Some(cmd) = receiver.blocking_recv() {
            #[cfg(feature = "grpc-dangerous")]
            let has_sender = cmd.sender.is_some();
            #[cfg(not(feature = "grpc-dangerous"))]
            let has_sender = false;

            if let Err(e) = self.handle_command(&mut datastore, cmd) {
                if let Some(ref orchestrator) = *orchestrator {
                    orchestrator.report_error(
                        format!("{e:?}"),
                        Some(
                            serde_yaml::to_string(&datastore.host_status())
                                .unwrap_or("Failed to serialize host status".into()),
                        ),
                    );
                }
                if has_sender {
                    // TODO: report the error back to the sender and then
                    // possibly restart Trident
                    error!("Failed to handle command: {e:?}");
                } else {
                    return Err(e);
                }
            }
        }

        // Temporarily calling set_boot_order here until we have a better place to call it
        // TODO -  https://dev.azure.com/mariner-org/ECF/_workitems/edit/6814
        if let Some(ref datastore_path) = self.config.datastore {
            info!("Setting boot order");
            bootentries::set_boot_order(datastore_path)?;
        }

        Ok(datastore.host_status().clone())
    }

    fn handle_command(
        &mut self,
        datastore: &mut DataStore,
        mut cmd: HostUpdateCommand,
    ) -> Result<(), TridentError> {
        if self.config.phonehome.is_some() && cmd.host_config.trident.phonehome.is_none() {
            info!("Injecting phonehome into host configuration");
            cmd.host_config.trident.phonehome = self.config.phonehome.clone();
        }

        cmd.host_config
            .validate()
            .map_err(|e| TridentError::new(InvalidInputError::InvalidHostConfiguration(e)))?;

        // When running inside a container, we need access to various host
        // paths. For now, check at least for presence of /host, which needs to
        // point to host's /. This function will return an error if running in a
        // container and /host is not mounted.
        container::is_running_in_container().message("Running in container check failed")?;

        if datastore.is_persistent() {
            modules::update(cmd, datastore).message("Failed to update host")
        } else {
            if datastore.host_status().spec != cmd.host_config {
                datastore.with_host_status(|status| {
                    *status = HostStatus {
                        spec: cmd.host_config.clone(),
                        reconcile_state: ReconcileState::CleanInstall,
                        ..Default::default()
                    }
                })?;
            }
            modules::clean_install(cmd, datastore).message("Failed to provision host")
        }
    }

    pub fn retrieve_host_status(&mut self, output_path: &Option<PathBuf>) -> Result<(), Error> {
        let host_status = if let Some(ref datastore_path) = self.config.datastore {
            info!("Opening persistent datastore");
            DataStore::open(datastore_path)
                .unstructured("Failed to open persistent datastore")?
                .host_status()
                .clone()
        } else if Path::new(TRIDENT_TEMPORARY_DATASTORE_PATH).exists() {
            info!("Opening temporary datastore");
            DataStore::open(Path::new(TRIDENT_TEMPORARY_DATASTORE_PATH))
                .unstructured("Failed to open temporary datastore")?
                .host_status()
                .clone()
        } else {
            bail!("No datastore found")
        };

        let host_status_yaml =
            serde_yaml::to_string(&host_status).context("Failed to serialize Host Status")?;
        match output_path {
            Some(path) => {
                info!("Writing Host Status to {:?}", &path);
                fs::write(path, host_status_yaml)
                    .context(format!("Failed to write Host Status to {:?}", path))?;
            }
            None => {
                println!("{host_status_yaml}");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use trident_api::{
        config::{FileSystemType, MountPoint, Storage},
        constants,
    };

    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_get_host_configuration() {
        // missing HC source
        let trident_config = LocalConfigFile::default();
        assert!(Trident::get_host_configuration(&trident_config)
            .unwrap()
            .is_none());

        // missing HC file
        let trident_config = LocalConfigFile::default().with_host_configuration_source(
            HostConfigurationSource::File(PathBuf::from("/does/not/exist")),
        );
        assert!(Trident::get_host_configuration(&trident_config).is_err());

        // ok
        let host_config_original = HostConfiguration {
            storage: Storage {
                mount_points: vec![MountPoint {
                    path: PathBuf::from(constants::ROOT_MOUNT_POINT_PATH),
                    target_id: "sda1".to_string(),
                    filesystem: FileSystemType::Ext4,
                    options: vec![],
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let trident_config =
            LocalConfigFile::default().with_host_configuration(host_config_original.clone());
        let host_config = Trident::get_host_configuration(&trident_config)
            .unwrap()
            .unwrap();
        assert_eq!(*host_config, host_config_original);
    }
}
