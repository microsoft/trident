use std::{
    fs,
    path::{Path, PathBuf},
};

use engine::{bootentries, EngineContext};
use log::{debug, error, info, warn};
use nix::unistd::Uid;
use tokio::sync::mpsc::{self};

use osutils::{block_devices, container};
use trident_api::{
    config::{GrpcConfiguration, HostConfiguration, HostConfigurationSource, Operations},
    status::ServicingType,
};
use trident_api::{
    constants::internal_params::ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS,
    error::{
        ExecutionEnvironmentMisconfigurationError, InitializationError, InternalError,
        InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt,
    },
    status::ServicingState,
};

#[cfg(feature = "setsail")]
use setsail::KsTranslator;

mod datastore;
mod engine;
mod harpoon_hc;
mod logging;
pub mod offline_init;
mod orchestrate;
pub mod osimage;
mod subsystems;
pub mod validation;

#[cfg(feature = "grpc-dangerous")]
mod grpc;

use datastore::DataStore;
use engine::{rollback, storage::rebuild};
use harpoon_hc::HostConfigUpdate;

pub use engine::provisioning_network;
pub use logging::{
    background_log::BackgroundLog, logstream::Logstream, multilog::MultiLogger,
    tracestream::TraceStream,
};
pub use orchestrate::OrchestratorConnection;

/// Trident version as provided by environment variables at build time
pub const TRIDENT_VERSION: &str = match option_env!("TRIDENT_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Trident binary path.
const TRIDENT_BINARY_PATH: &str = "/usr/bin/trident";

/// OS Modifier (EMU) binary path.
const OS_MODIFIER_BINARY_PATH: &str = "/usr/bin/osmodifier";

/// Path to the Trident background log for the current servicing.
pub const TRIDENT_BACKGROUND_LOG_PATH: &str = "/var/log/trident-full.log";

/// Path to the Trident metrics file for the current servicing.
pub const TRIDENT_METRICS_FILE_PATH: &str = "/var/log/trident-metrics.jsonl";

/// Trident will by default prevent running Clean Install on deployments other
/// than from the Provisioning ISO, to limit chances of accidental data loss. To
/// override, user can create this file on the host.
const SAFETY_OVERRIDE_CHECK_PATH: &str = "/override-trident-safety-check";

/// A command to update the Host Configuration.
///
/// This struct is used to communicate between the gRPC server and the main Trident thread. It
/// includes information on the command to execute, as well as a tokio Sender which the main thread
/// can use to stream status updates back to the gRPC client.
struct HostUpdateCommand {
    allowed_operations: Operations,
    host_config: HostConfiguration,
    #[cfg(feature = "grpc-dangerous")]
    sender: Option<mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>>,
}

#[derive(clap::ValueEnum, Copy, Clone, Debug, Eq, PartialEq)]
pub enum GetKind {
    Configuration,
    Status,
    LastError,
}

pub struct Trident {
    host_config: Option<HostConfiguration>,
    orchestrator: Option<OrchestratorConnection>,
    grpc: Option<GrpcConfiguration>,

    #[allow(unused)]
    server_runtime: Option<tokio::runtime::Runtime>,
}

impl Trident {
    pub fn new(
        config_source: Option<HostConfigurationSource>,
        datastore_path: &Path,
        logstream: Logstream,
        tracestream: TraceStream,
    ) -> Result<Self, TridentError> {
        let host_config = config_source
            .map(|source| Self::load_host_config(&source))
            .transpose()?;

        let (phonehome_url, logstream_url, connection_timeout_param) =
            if let Some(config) = &host_config {
                (
                    config.trident.phonehome.clone(),
                    config.trident.logstream.clone(),
                    config
                        .internal_params
                        .get_u16(ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS),
                )
            } else if let Ok(datastore) = DataStore::open(datastore_path) {
                let host_config = &datastore.host_status().spec;
                (
                    host_config.trident.phonehome.clone(),
                    host_config.trident.logstream.clone(),
                    host_config
                        .internal_params
                        .get_u16(ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS),
                )
            } else {
                (None, None, None)
            };

        let connection_timeout = if let Some(connection_timeout_result) = connection_timeout_param {
            match connection_timeout_result {
                Ok(connection_timeout_value) => Some(connection_timeout_value),
                Err(e) => return Err(TridentError::new(e)),
            }
        } else {
            None
        };

        // Set up logstream if configured
        if let Some(url) = logstream_url {
            logstream
                .set_server(url.to_string())
                .structured(InitializationError::ConnectToLogstream)?;
        }

        let orchestrator = phonehome_url
            .as_ref()
            .and_then(|url| OrchestratorConnection::new(url.clone(), connection_timeout));

        // Set up tracestream if configured, using phonehome url for now
        if let Some(url) = phonehome_url {
            let trace_url = url.clone().replace("phonehome", "tracestream");
            tracestream
                .set_server(trace_url)
                .structured(InitializationError::ConnectToTracestream)?;
        }

        Ok(Self {
            host_config,
            orchestrator,
            server_runtime: None,
            grpc: None,
        })
    }

    fn load_host_config(
        source: &HostConfigurationSource,
    ) -> Result<HostConfiguration, TridentError> {
        let host_config = match source {
            // Load the Host Configuration from a file.
            HostConfigurationSource::File(path) => {
                info!(
                    "Loading Host Configuration from file at path '{}'",
                    path.display()
                );

                let contents = fs::read_to_string(path).structured(
                    InvalidInputError::LoadHostConfigurationFile {
                        path: path.display().to_string(),
                    },
                )?;

                validation::parse_host_config(&contents, path)?
            }

            // Use the embedded Host Configuration.
            HostConfigurationSource::Embedded(contents) => *contents.clone(),

            // When enabled, load a kickstart body from the local config and translate it to a host
            // configuration.
            #[cfg(feature = "setsail")]
            HostConfigurationSource::KickstartEmbedded(contents) => KsTranslator::new()
                .run_pre_scripts(true)
                .translate(setsail::load_kickstart_string(contents))
                .structured(InvalidInputError::TranslateKickstart)?,

            // When enabled, load a kickstart file from the local config and translate it to a host
            // configuration.
            #[cfg(feature = "setsail")]
            HostConfigurationSource::KickstartFile(ref file) => KsTranslator::new()
                .run_pre_scripts(true)
                .translate(setsail::load_kickstart_file(file).structured(
                    InvalidInputError::LoadKickstart {
                        path: file.display().to_string(),
                    },
                )?)
                .structured(InvalidInputError::TranslateKickstart)?,
        };

        info!(
            "Host Configuration:\n{}",
            serde_yaml::to_string(&host_config)
                .unwrap_or("Failed to serialize Host Configuration".into())
        );

        Ok(host_config)
    }

    pub fn start_network(config_source: HostConfigurationSource) -> Result<(), TridentError> {
        // If we have kickstart it means we don't have networking config readily available. We
        // _could_ try parsing now, but we are in an early stage of boot and we want to parse on a
        // later stage so %pre scripts can run and do their thing. It would also mean parsing twice,
        // unless we updated the config file in place. That sounds like a can of worms and we still
        // have the issue about being too early.
        #[cfg(feature = "setsail")]
        if let HostConfigurationSource::KickstartFile(_)
        | HostConfigurationSource::KickstartEmbedded(_) = config_source
        {
            warn!("Cannot set up network early when using kickstart");
            return Ok(());
        }

        let host_config = Self::load_host_config(&config_source)?;

        info!("Starting network");
        provisioning_network::start(&host_config).structured(ServicingError::StartNetwork)?;

        Ok(())
    }

    pub fn run(
        &mut self,
        datastore_path: &Path,
        allowed_operations: Operations,
    ) -> Result<(), TridentError> {
        info!("Running Trident version: {}", TRIDENT_VERSION);

        if container::is_running_in_container().unwrap_or(false) {
            info!("Running Trident in a container");
        }

        if !Uid::effective().is_root() {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::CheckRootPrivileges,
            ));
        }

        // Open the datastore.
        let mut datastore =
            DataStore::open_or_create(datastore_path).message("Failed to open datastore")?;

        // This creates a channel to send commands to the main trident thread. It lets us use the
        // same logic for processing an initial provision command contained within the trident local
        // config as for processing commands received from the gRPC endpoint.
        let (sender, receiver) = tokio::sync::mpsc::channel(1);

        // If we have a local Host Configuration source, load it and dispatch it as the first
        // command.
        if let Some(local_host_config) = self.host_config.clone() {
            debug!("Applying Host Configuration from local config");
            sender
                .blocking_send(HostUpdateCommand {
                    allowed_operations,
                    host_config: local_host_config,
                    #[cfg(feature = "grpc-dangerous")]
                    sender: None,
                })
                .structured(InternalError::EnqueueHostUpdateCommand)?;
        } else {
            // Otherwise, ONLY IF:
            // - Harpoon support is enabled+configured AND
            // - The host is provisioned
            //
            // Then query Harpoon for an updated HC.
            harpoon_hc::try_on_harpoon_enabled(
                &datastore.host_status().spec,
                |harpoon_config| -> Result<(), TridentError> {
                    // We only check if the system is provisioned.
                    if datastore.host_status().servicing_state != ServicingState::Provisioned {
                        return Ok(());
                    }

                    info!(
                        "Querying server for updated Host Configuration. URL: {}, App ID: {}, Track: {}, Document Version: {}",
                        harpoon_config.url, harpoon_config.app_id, harpoon_config.track, harpoon_config.document_version
                    );

                    // Call into harpoon module to get an updated HC.
                    match harpoon_hc::query_and_fetch_host_config(harpoon_config)? {
                        HostConfigUpdate::Updated {
                            host_config,
                            version,
                        } => {
                            info!("Server replied with new Host configuration v{version}, applying...");
                            sender
                                .blocking_send(HostUpdateCommand {
                                    allowed_operations,
                                    host_config: *host_config,
                                    #[cfg(feature = "grpc-dangerous")]
                                    sender: None,
                                })
                                .structured(InternalError::EnqueueHostUpdateCommand)?;
                        }
                        HostConfigUpdate::NoUpdate => {
                            warn!("No update available. No action will be taken.");
                        }
                    }

                    Ok(())
                },
            )?;
        }

        if !cfg!(feature = "grpc-dangerous") || self.grpc.is_none() {
            // If no gRPC connection details were provided, drop the sender side of the channel.
            // This causes the loop below will exit immediately after processing the initial command
            // that was enqueued above.
            drop(sender);
        } else if let Some(_grpc) = &self.grpc {
            #[cfg(feature = "grpc-dangerous")]
            {
                self.server_runtime = Some(grpc::start(_grpc, self.orchestrator.as_ref(), sender)?);
            }
        }

        if let Err(e) = self.handle_commands(receiver, &mut datastore) {
            let error = serde_yaml::to_value(&e).structured(InternalError::SerializeError)?;
            if let Err(e2) = datastore.with_host_status(|status| status.last_error = Some(error)) {
                error!("Failed to record error in datastore: {e2:?}");
            }

            return Err(e);
        }

        if let Some(ref orchestrator) = self.orchestrator {
            orchestrator.report_success(Some(
                serde_yaml::to_string(&datastore.host_status())
                    .unwrap_or("Failed to serialize Host Status".into()),
            ))
        }
        Ok(())
    }

    /// Rebuilds RAID devices on replaced disks on the host
    pub fn rebuild_raid(&mut self, datastore_path: &Path) -> Result<(), TridentError> {
        info!("Rebuilding RAID devices");
        if !Uid::effective().is_root() {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::CheckRootPrivileges,
            ));
        }
        let mut host_config = Default::default();
        let mut disks_to_rebuild = Vec::new();
        let mut datastore = DataStore::open(datastore_path)?;
        let _ = datastore.with_host_status(|host_status| -> Result<(), TridentError> {
            host_config = self
                .host_config
                .clone()
                .unwrap_or_else(|| host_status.spec.clone());

            let resolved_disks = block_devices::get_resolved_disks(&host_config)
                .structured(ServicingError::GetResolvedDisks)?;
            disks_to_rebuild =
                rebuild::get_disks_to_rebuild(&host_status.disk_uuids, &resolved_disks)
                    .structured(ServicingError::GetDisksToRebuild)?;
            info!("Validating and rebuilding RAID devices");
            // Validate the loaded Host Configuration
            rebuild::validate_rebuild_raid(&host_config, host_status, &disks_to_rebuild)
                .structured(ServicingError::ValidateRebuildRaid)?;
            // Rebuild RAID devices
            rebuild::rebuild_raid(&host_config, host_status)
                .structured(ServicingError::RebuildRaid)?;

            Ok(())
        })?;

        let host_status = datastore.host_status();

        let ctx = EngineContext {
            spec: host_status.spec.clone(),
            spec_old: host_status.spec_old.clone(),
            servicing_type: ServicingType::NoActiveServicing,
            ab_active_volume: host_status.ab_active_volume,
            partition_paths: host_status.partition_paths.clone(),
            disk_uuids: host_status.disk_uuids.clone(),
            install_index: host_status.install_index,
            os_image: None,
            storage_graph: engine::build_storage_graph(&host_config.storage), // Build storage graph
        };

        if ctx.ab_active_volume.is_none() {
            return Err(TridentError::new(InternalError::Internal(
                "No active volume selected",
            )));
        }

        let entry_labels = bootentries::get_entry_labels(ctx.install_index)?;

        info!("Creating and updating boot variables after rebuilding RAID devices");
        // Create boot entries and update boot variables after rebuilding RAID devices
        bootentries::create_and_update_boot_variables_after_rebuilding(
            &ctx,
            entry_labels.to_vec(),
            &disks_to_rebuild,
        )
    }

    fn handle_commands(
        &mut self,
        mut receiver: mpsc::Receiver<HostUpdateCommand>,
        datastore: &mut DataStore,
    ) -> Result<(), TridentError> {
        debug!(
            "Current servicing state: {:?}",
            datastore.host_status().servicing_state
        );

        datastore.with_host_status(|host_status| {
            if let Some(e) = host_status.last_error.take() {
                warn!("Previously encountered error: {e:?}");
                info!("Clearing last error");
            }
        })?;

        // If host's servicing state is Finalized, need to validate that the firmware correctly
        // booted from the updated runtime OS image.
        if datastore.host_status().servicing_state == ServicingState::CleanInstallFinalized
            || datastore.host_status().servicing_state == ServicingState::AbUpdateFinalized
        {
            let rollback_result = rollback::validate_boot(datastore).message(
                "Failed to validate that firmware correctly booted from updated runtime OS image",
            );

            harpoon_hc::on_harpoon_enabled_event(
                &datastore.host_status().spec,
                harpoon::EventType::Update,
                match rollback_result {
                    Ok(_) => harpoon::EventResult::SuccessReboot,
                    Err(_) => harpoon::EventResult::Error,
                },
            );

            // Re"throw" the error if there was one.
            rollback_result?;
        }

        // Process commands. Starting with the initial command indicated in the local config file
        // (if any). Once that has been handled, subsequent commands are received from the gRPC
        // endpoint.
        while let Some(cmd) = receiver.blocking_recv() {
            #[cfg(feature = "grpc-dangerous")]
            let has_sender = cmd.sender.is_some();
            #[cfg(not(feature = "grpc-dangerous"))]
            let has_sender = false;

            if let Err(e) = self.handle_command(datastore, cmd) {
                if let Some(ref orchestrator) = self.orchestrator {
                    orchestrator.report_error(
                        format!("{e:?}"),
                        Some(
                            serde_yaml::to_string(&datastore.host_status())
                                .unwrap_or("Failed to serialize Host Status".into()),
                        ),
                    );
                }

                // When harpoon is enabled, try to report an error to the server.
                harpoon_hc::on_harpoon_enabled_event(
                    &datastore.host_status().spec,
                    harpoon::EventType::Install,
                    harpoon::EventResult::Error,
                );

                if has_sender {
                    // TODO: report the error back to the sender and then
                    // possibly restart Trident
                    error!("Failed to handle command: {e:?}");
                } else {
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    fn handle_command(
        &mut self,
        datastore: &mut DataStore,
        mut cmd: HostUpdateCommand,
    ) -> Result<(), TridentError> {
        cmd.host_config
            .validate()
            .map_err(Into::into)
            .message("Invalid Host Configuration provided")?;

        // Populate internal fields in Host Configuration.
        // This is needed because the external API and the internal logic use different fields.
        // This call ensures that the internal fields are populated from the external fields.
        cmd.host_config.populate_internal();

        // When running inside a container, we need access to various host
        // paths. For now, check at least for presence of /host, which needs to
        // point to host's /. This function will return an error if running in a
        // container and /host is not mounted.
        container::is_running_in_container().message("Running in container check failed")?;

        // If Trident loads from a persistent datastore, firmware is booted from runtime OS
        if datastore.is_persistent() {
            // If HS.spec in the datastore is different from the new HC, need to both stage and
            // finalize the update, regardless of state
            if datastore.host_status().spec != cmd.host_config {
                debug!("Host Configuration has been updated");
                // If allowed operations include 'stage', start update
                if cmd.allowed_operations.has_stage() {
                    engine::update(cmd, datastore).message("Failed to execute an update")
                } else {
                    warn!("Host Configuration has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run to stage the update");
                    Ok(())
                }
            } else {
                debug!("Host Configuration has not been updated");

                match datastore.host_status().servicing_state {
                    ServicingState::AbUpdateStaged => {
                        // If an update has been previously staged, only need to finalize the update.
                        debug!("There is an update staged on the host");
                        if cmd.allowed_operations.has_finalize() {
                            engine::finalize_update(
                                datastore,
                                ServicingType::AbUpdate,
                                None,
                                #[cfg(feature = "grpc-dangerous")]
                                &mut cmd.sender,
                            )
                            .message("Failed to finalize update")
                        } else {
                            warn!("There is an update staged on the host, but allowed operations do not include 'finalize'. Add 'finalize' and re-run to finalize the update");
                            Ok(())
                        }
                    }
                    ServicingState::AbUpdateFinalized | ServicingState::Provisioned => {
                        // Need to either re-execute the failed update OR inform the user that no update
                        // is needed.
                        engine::update(cmd, datastore).message("Failed to update host")
                    }
                    servicing_state => {
                        Err(TridentError::new(InternalError::UnexpectedServicingState {
                            state: servicing_state,
                        }))
                        .message("Failed to A/B update with same Host Configuration")
                    }
                }
            }
        } else {
            // If datastore is temporary, firmware booted from prov OS, so can only do clean
            // install.
            //
            // If HS.spec in the datastore is different from the new HC, need to both stage and
            // finalize the clean install.
            if datastore.host_status().spec != cmd.host_config {
                debug!("Host Configuration has been updated");

                if cmd.allowed_operations.has_stage() {
                    engine::clean_install(cmd, datastore)
                        .message("Failed to execute a clean install")
                } else {
                    warn!("Host Configuration has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run to stage the clean install");
                    Ok(())
                }
            } else {
                debug!("Host Configuration has not been updated");

                match datastore.host_status().servicing_state {
                    ServicingState::CleanInstallStaged => {
                        // If a clean install has been staged on the host, only need to finalize the
                        // clean install, if requested.
                        debug!("There is a clean install staged on the host");
                        if cmd.allowed_operations.has_finalize() {
                            engine::finalize_clean_install(
                                datastore,
                                None,
                                None,
                                #[cfg(feature = "grpc-dangerous")]
                                &mut cmd.sender,
                            )
                            .message("Failed to finalize clean install")
                        } else {
                            debug!("There is a clean install staged on the host, but allowed operations do not include 'finalize'. Add 'finalize' and re-run to finalize the clean install");
                            Ok(())
                        }
                    }
                    ServicingState::NotProvisioned => {
                        // Otherwise, if servicing state is NotProvisioned, need to either re-execute the
                        // failed clean install OR inform the user that no update is needed.
                        engine::clean_install(cmd, datastore)
                            .message("Failed to execute a clean install")
                    }
                    servicing_state => {
                        Err(TridentError::new(InternalError::UnexpectedServicingState {
                            state: servicing_state,
                        }))
                        .message("Failed to run from management OS")
                    }
                }
            }
        }
    }

    pub fn get(
        datastore_path: &Path,
        output_path: &Option<PathBuf>,
        kind: GetKind,
    ) -> Result<(), TridentError> {
        let host_status = DataStore::open(datastore_path)
            .message("Failed to open datastore")?
            .host_status()
            .clone();

        let yaml = match kind {
            GetKind::Configuration => serde_yaml::to_string(&host_status.spec)
                .structured(InternalError::SerializeHostStatus)?,
            GetKind::Status => serde_yaml::to_string(&host_status)
                .structured(InternalError::SerializeHostStatus)?,
            GetKind::LastError => serde_yaml::to_string(&host_status.last_error)
                .structured(InternalError::SerializeError)?,
        };

        match output_path {
            Some(path) => {
                info!("Writing to {:?}", &path);
                fs::write(path, yaml).structured(InvalidInputError::WriteOutputFile {
                    path: path.display().to_string(),
                })?
            }
            None => {
                println!("{yaml}");
            }
        }

        Ok(())
    }
}
