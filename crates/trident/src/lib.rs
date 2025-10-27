use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use cli::GetKind;
use engine::{bootentries, EngineContext};
use log::{debug, error, info, warn};
use nix::unistd::Uid;

use osutils::{block_devices, container, dependencies::Dependency};
use trident_api::{
    config::{GrpcConfiguration, HostConfiguration, HostConfigurationSource, Operations},
    constants::internal_params::{
        HTTP_CONNECTION_TIMEOUT_SECONDS, ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS,
        WAIT_FOR_SYSTEMD_NETWORKD,
    },
    error::{
        ExecutionEnvironmentMisconfigurationError, InitializationError, InternalError,
        InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt,
    },
    status::{ServicingState, ServicingType},
};

#[cfg(feature = "grpc-dangerous")]
use grpc::GrpcSender;

pub mod cli;
mod datastore;
mod engine;
mod io_utils;
mod logging;
mod monitor_metrics;
pub mod offline_init;
mod orchestrate;
pub mod osimage;
mod subsystems;
pub mod validation;

#[cfg(feature = "grpc-dangerous")]
mod grpc;

use engine::{rollback, storage::rebuild};

pub use datastore::DataStore;
pub use engine::{provisioning_network, reboot};
pub use logging::{
    background_log::BackgroundLog, logstream::Logstream, multilog::MultiLogger,
    tracestream::TraceStream,
};
pub use orchestrate::OrchestratorConnection;

use crate::osimage::OsImage;

/// Trident version as provided by environment variables at build time
pub const TRIDENT_VERSION: &str = match option_env!("TRIDENT_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};

/// Trident binary path.
const TRIDENT_BINARY_PATH: &str = "/usr/bin/trident";

/// OS Modifier (EMU) binary path.
const OS_MODIFIER_BINARY_PATH: &str = "/usr/bin/osmodifier";

/// Path to OS Modifier on the newroot.
const OS_MODIFIER_NEWROOT_PATH: &str = "/tmp/osmodifier";

/// Path to the Trident background log for the current servicing.
pub const TRIDENT_BACKGROUND_LOG_PATH: &str = "/var/log/trident-full.log";

/// Path to the Trident metrics file for the current servicing.
pub const TRIDENT_METRICS_FILE_PATH: &str = "/var/log/trident-metrics.jsonl";

/// Trident will by default prevent running Clean Install on deployments other
/// than from the Provisioning ISO, to limit chances of accidental data loss. To
/// override, user can create this file on the host.
const SAFETY_OVERRIDE_CHECK_PATH: &str = "/override-trident-safety-check";

/// Temporary location of the datastore for multiboot install scenarios.
const TEMPORARY_DATASTORE_PATH: &str = "/tmp/trident-datastore.sqlite";

#[must_use]
pub enum ExitKind {
    /// Requested operation completed successfully.
    Done,
    /// Reboot is needed to complete the operation.
    NeedsReboot,
}

pub struct Trident {
    host_config: Option<HostConfiguration>,
    orchestrator: Option<OrchestratorConnection>,

    #[cfg_attr(not(feature = "grpc-dangerous"), allow(unused))]
    grpc: Option<GrpcConfiguration>,

    #[cfg_attr(not(feature = "grpc-dangerous"), allow(unused))]
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

        let (phonehome_url, logstream_url, connection_timeout_param, wait_for_network) =
            if let Some(config) = &host_config {
                (
                    config.trident.phonehome.clone(),
                    config.trident.logstream.clone(),
                    config
                        .internal_params
                        .get_u16(ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS),
                    config.internal_params.get_flag(WAIT_FOR_SYSTEMD_NETWORKD),
                )
            } else if let Ok(datastore) = DataStore::open(datastore_path) {
                let host_config = &datastore.host_status().spec;
                (
                    host_config.trident.phonehome.clone(),
                    host_config.trident.logstream.clone(),
                    host_config
                        .internal_params
                        .get_u16(ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS),
                    host_config
                        .internal_params
                        .get_flag(WAIT_FOR_SYSTEMD_NETWORKD),
                )
            } else {
                (None, None, None, false)
            };

        if wait_for_network {
            debug!("Waiting for systemd-networkd-wait-online");
            Dependency::Systemctl
                .cmd()
                .arg("start")
                .arg("systemd-networkd-wait-online")
                .run_and_check()
                .structured(InternalError::WaitForSystemdNetworkd)?;
            debug!("Finished waiting for systemd-networkd-wait-online");
        }

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

        info!("Running Trident version: {}", TRIDENT_VERSION);
        if container::is_running_in_container().message("Running in container check failed")? {
            info!("Running Trident in a container");
        }

        if let Ok(selinux_context) = fs::read_to_string("/proc/self/attr/current") {
            debug!(
                "Trident is running in SELinux domain '{}'",
                selinux_context.trim()
            );
        } else {
            error!("Failed to retrieve the SELinux context in which Trident is running");
        }

        if !Uid::effective().is_root() {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::CheckRootPrivileges,
            ));
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
        };

        info!(
            "Host Configuration:\n{}",
            serde_yaml::to_string(&host_config)
                .unwrap_or("Failed to serialize Host Configuration".into())
        );

        Ok(host_config)
    }

    pub fn start_network(config_source: HostConfigurationSource) -> Result<(), TridentError> {
        let host_config = Self::load_host_config(&config_source)?;

        info!("Starting network");
        provisioning_network::start(&host_config).structured(ServicingError::StartNetwork)?;

        Ok(())
    }

    /// Listen for incoming commands from an orchestrator, and execute the first one.
    pub fn listen(&mut self, datastore: &mut DataStore) -> Result<(), TridentError> {
        #[cfg(feature = "grpc-dangerous")]
        if let Some(grpc) = &self.grpc {
            let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
            self.server_runtime = Some(grpc::start(grpc, self.orchestrator.as_ref(), sender)?);

            if let Some((host_config, allowed_operations, sender)) = receiver.blocking_recv() {
                self.host_config = Some(host_config);
                if let ExitKind::NeedsReboot =
                    self.update(datastore, allowed_operations, &mut Some(sender))?
                {
                    reboot().message("Failed to reboot after grpc update")?;
                }
            }
        }

        // Avoid unused variable warning if grpc-dangerous is not enabled
        #[cfg(not(feature = "grpc-dangerous"))]
        let _ = datastore;

        Ok(())
    }

    fn execute_and_record_error<F, T>(
        &mut self,
        datastore: &mut DataStore,
        f: F,
    ) -> Result<T, TridentError>
    where
        F: FnOnce(&mut DataStore) -> Result<T, TridentError>,
    {
        datastore.with_host_status(|host_status| {
            if let Some(e) = host_status.last_error.take() {
                warn!("Previously encountered error: {e:?}");
                info!("Clearing last error");
            }
        })?;

        match f(datastore) {
            Ok(t) => Ok(t),
            Err(e) => {
                // Record error in datastore.
                let error = serde_yaml::to_value(&e).structured(InternalError::SerializeError)?;
                if let Err(e2) =
                    datastore.with_host_status(|status| status.last_error = Some(error))
                {
                    error!("Failed to record error in datastore: {e2:?}");
                }

                // Report error via phonehome.
                if let Some(ref orchestrator) = self.orchestrator {
                    orchestrator.report_error(
                        format!("{e:?}"),
                        Some(
                            serde_yaml::to_string(&datastore.host_status())
                                .unwrap_or("Failed to serialize Host Status".into()),
                        ),
                    );
                }
                // TODO: report gPRC error

                Err(e)
            }
        }
    }

    fn get_cosi_image(host_config: &mut HostConfiguration) -> Result<OsImage, TridentError> {
        let cosi_timeout = match host_config
            .internal_params
            .get_u64(HTTP_CONNECTION_TIMEOUT_SECONDS)
        {
            Some(Ok(timeout)) => Duration::from_secs(timeout),
            _ => Duration::from_secs(10), // Default timeout
        };
        OsImage::load(&mut host_config.image, cosi_timeout)
    }

    /// Rebuilds RAID devices on replaced disks on the host
    pub fn rebuild_raid(&mut self, datastore: &mut DataStore) -> Result<(), TridentError> {
        info!("Rebuilding RAID devices");
        let mut host_config = Default::default();
        let mut disks_to_rebuild = Vec::new();
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
            image: None,
            storage_graph: engine::build_storage_graph(&host_config.storage)?, // Build storage graph
            filesystems: Vec::new(), // Left empty since context does not have image
            is_uki: None,
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

    pub fn install(
        &mut self,
        datastore: &mut DataStore,
        allowed_operations: Operations,
        multiboot: bool,
        #[cfg(feature = "grpc-dangerous")] sender: &mut Option<GrpcSender>,
    ) -> Result<ExitKind, TridentError> {
        let mut host_config = self
            .host_config
            .clone()
            .structured(InternalError::Internal(
                "install called without Host Configuration set",
            ))?;

        self.execute_and_record_error(datastore, |datastore| {
            host_config
                .validate()
                .map_err(Into::into)
                .message("Invalid Host Configuration provided")?;

            // If multiboot is requested, we need to check if the host has
            // adopted partitions, otherwise there is no reason to use
            // multiboot.
            if multiboot && !host_config.has_adopted_partitions() {
                return Err(TridentError::new(
                    InvalidInputError::MultibootWithoutAdoptedPartitions,
                ))
                .message("Multiboot install requested but no adopted partitions found");
            }

            // Check if the datastore is persistent to know if this is a
            // provisioned host. If the host is not provisioned, we can proceed
            // with a clean install. If the host is provisioned, we need to
            // check if a multiboot install was requested.
            if datastore.is_persistent() {
                if !multiboot {
                    // If the host IS provisioned and multiboot is NOT requested
                    // this leads to an error as this could be an accident.
                    return Err(TridentError::new(
                        InvalidInputError::CleanInstallOnProvisionedHost,
                    ))
                    .message("Persistent datastore found on host.");
                } else {
                    // If the host IS provisioned and multiboot IS requested, we
                    // need to create a temporary datastore for the new install
                    // to avoid overwriting the existing one.
                    debug!(
                        "Detected a previous persistent datastore. Creating a temporary one for \
                        multiboot install"
                    );

                    datastore.close();
                    *datastore = DataStore::open_or_create(Path::new(TEMPORARY_DATASTORE_PATH))
                        .message("Failed to create temporary datastore for multiboot install")?;
                }
            }

            let image = Self::get_cosi_image(&mut host_config)?;

            if datastore.host_status().spec != host_config {
                debug!("Host Configuration has been updated");

                if allowed_operations.has_stage() {
                    engine::clean_install(
                        &host_config,
                        datastore,
                        &allowed_operations,
                        multiboot,
                        image,
                        #[cfg(feature = "grpc-dangerous")]
                        sender,
                    )
                    .message("Failed to execute a clean install")
                } else {
                    warn!(
                        "Host Configuration has been updated but allowed operations do not include \
                        'stage'. Add 'stage' and re-run to stage the clean install"
                    );

                    Ok(ExitKind::Done)
                }
            } else {
                debug!("Host Configuration has not been updated");

                match datastore.host_status().servicing_state {
                    ServicingState::CleanInstallStaged => {
                        // If a clean install has been staged on the host, only need to finalize the
                        // clean install, if requested.
                        debug!("There is a clean install staged on the host");
                        if allowed_operations.has_finalize() {
                            engine::finalize_clean_install(
                                datastore,
                                None,
                                None,
                                #[cfg(feature = "grpc-dangerous")]
                                sender,
                            )
                            .message("Failed to finalize clean install")
                        } else {
                            debug!(
                                "There is a clean install staged on the host, but allowed \
                                operations do not include 'finalize'. Add 'finalize' and re-run \
                                to finalize the clean install"
                            );

                            Ok(ExitKind::Done)
                        }
                    }
                    ServicingState::NotProvisioned => {
                        // Otherwise, if servicing state is NotProvisioned, need to either re-execute the
                        // failed clean install OR inform the user that no update is needed.
                        engine::clean_install(
                            &host_config,
                            datastore,
                            &allowed_operations,
                            multiboot,
                            image,
                            #[cfg(feature = "grpc-dangerous")]
                            sender,
                        )
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
        })
    }

    pub fn update(
        &mut self,
        datastore: &mut DataStore,
        allowed_operations: Operations,
        #[cfg(feature = "grpc-dangerous")] sender: &mut Option<GrpcSender>,
    ) -> Result<ExitKind, TridentError> {
        let mut host_config = self
            .host_config
            .clone()
            .structured(InternalError::Internal(
                "update called without Host Configuration set",
            ))?;

        self.execute_and_record_error(datastore, |datastore| {
            if !datastore.is_persistent() {
                return Err(TridentError::new(InvalidInputError::HostNotProvisioned))
                    .message("Persistent datastore not found on host");
            }

            // The storage section is optional for updates if COSI is in use.
            if host_config.image.is_some() && host_config.storage == Default::default() {
                host_config.storage = datastore.host_status().spec.storage.clone();
                debug!("Storage section not specified in Host Configuration, using current storage configuration:\n{}",
                    serde_yaml::to_string(&host_config.storage)
                        .unwrap_or("Failed to serialize Storage Configuration".into()));
            }

            host_config
                .validate()
                .map_err(Into::into)
                .message("Invalid Host Configuration provided")?;

            let image = Self::get_cosi_image(&mut host_config)?;

            // If HS.spec in the datastore is different from the new HC, need to both stage and
            // finalize the update, regardless of state
            if datastore.host_status().spec != host_config {
                debug!("Host Configuration has been updated");
                // If allowed operations include 'stage', start update
                if allowed_operations.has_stage() {
                    engine::update(&host_config, datastore, &allowed_operations, image, #[cfg(feature = "grpc-dangerous")] sender).message("Failed to execute an update")
                } else {
                    warn!("Host Configuration has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run to stage the update");
                    Ok(ExitKind::Done)
                }
            } else {
                debug!("Host Configuration has not been updated");

                match datastore.host_status().servicing_state {
                    ServicingState::AbUpdateStaged => {
                        // If an update has been previously staged, only need to finalize the update.
                        debug!("There is an update staged on the host");
                        if allowed_operations.has_finalize() {
                            engine::finalize_update(
                                datastore,
                                ServicingType::AbUpdate,
                                None,
                                #[cfg(feature = "grpc-dangerous")]
                                sender,
                            )
                            .message("Failed to finalize update")
                        } else {
                            warn!("There is an update staged on the host, but allowed operations do not include 'finalize'. Add 'finalize' and re-run to finalize the update");
                            Ok(ExitKind::Done)
                        }
                    }
                    ServicingState::AbUpdateFinalized | ServicingState::Provisioned => {
                        // Need to either re-execute the failed update OR inform the user that no update
                        // is needed.
                        engine::update(&host_config, datastore, &allowed_operations, image, #[cfg(feature = "grpc-dangerous")] sender).message("Failed to update host")
                    }
                    servicing_state => {
                        Err(TridentError::new(InternalError::UnexpectedServicingState {
                            state: servicing_state,
                        }))
                        .message("Failed to A/B update with same Host Configuration")
                    }
                }
            }
        })
    }

    pub fn commit(&mut self, datastore: &mut DataStore) -> Result<(), TridentError> {
        // If host's servicing state is Finalized, need to validate that the firmware correctly
        // booted from the updated target OS image.
        if datastore.host_status().servicing_state != ServicingState::CleanInstallFinalized
            && datastore.host_status().servicing_state != ServicingState::AbUpdateFinalized
        {
            info!("No update in progress, skipping commit");
            return Ok(());
        }

        let rollback_result = self.execute_and_record_error(datastore, |datastore| {
            rollback::validate_boot(datastore).message(
                "Failed to validate that firmware correctly booted from updated target OS image",
            )
        });

        if rollback_result.is_ok() {
            if let Some(ref orchestrator) = self.orchestrator {
                orchestrator.report_success(Some(
                    serde_yaml::to_string(&datastore.host_status())
                        .unwrap_or("Failed to serialize Host Status".into()),
                ))
            }
        }

        // Re"throw" the error if there was one.
        rollback_result
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
