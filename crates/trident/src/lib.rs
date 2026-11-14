use crate::telemetry::events;
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use cli::GetKind;
use log::{debug, error, info, warn};
use nix::unistd::Uid;
use semver::Version;
use url::Url;

use engine::{bootentries, EngineContext, EngineContextParams};
use osutils::{block_devices, container, dependencies::Dependency, osrelease::OsRelease};
use trident_api::{
    config::{
        HostConfiguration, HostConfigurationSource, ImageSha384, Operations,
        OsImage as ConfigOsImage,
    },
    constants::internal_params::{
        HTTP_CONNECTION_TIMEOUT_SECONDS, ORCHESTRATOR_CONNECTION_TIMEOUT_SECONDS,
        WAIT_FOR_SYSTEMD_NETWORKD,
    },
    error::{
        DatastoreError, ExecutionEnvironmentMisconfigurationError, InitializationError,
        InternalError, InvalidInputError, ReportError, ServicingError, TridentError,
        TridentResultExt,
    },
    primitives::hash::Sha384Hash,
    status::{ServicingState, ServicingType},
};

pub mod agentconfig;
pub mod cli;
mod datastore;
mod diagnostics;
mod disk_selector;
mod engine;
mod grpc_client;
mod health;
pub mod init;
mod io_utils;
mod logging;
mod monitor_metrics;
mod orchestrate;
pub mod osimage;
mod reboot;
mod server;
pub mod stream;
mod subsystems;
mod telemetry;
pub mod validation;

pub use crate::{
    datastore::DataStore,
    engine::{
        manual_rollback::{self, utils::ManualRollbackRequestKind},
        provisioning_network,
    },
    grpc_client::client_main,
    logging::{
        background_log::BackgroundLog, background_uploader::BackgroundUploader,
        logfwd::LogForwarder, logstream::Logstream, tracestream::TraceStream,
    },
    orchestrate::OrchestratorConnection,
    reboot::request_reboot_with_wait,
    server::server_main,
};

use crate::{
    engine::{
        ab_update,
        rollback::{self, BootValidationResult},
        runtime_update,
        storage::rebuild,
        SUBSYSTEMS,
    },
    init::acl,
    osimage::OsImage,
    stream::DiskSelectionStrategy,
    subsystems::esp,
};

/// Trident version as provided by environment variables at build time
pub const TRIDENT_VERSION: &str = match option_env!("TRIDENT_VERSION") {
    Some(v) => v,
    None => env!("CARGO_PKG_VERSION"),
};
lazy_static::lazy_static! {
    /// Trident version parsed as a semver::Version
    pub static ref TRIDENT_SEMVER_VERSION: Version = Version::parse(TRIDENT_VERSION)
        .expect("Failed to parse TRIDENT_VERSION as semver::Version");
}

/// Trident binary path.
const TRIDENT_BINARY_PATH: &str = "/usr/bin/trident";

/// Launcher binary path.
const LAUNCHER_BINARY_PATH: &str = "/usr/bin/launcher";

/// Path to the Trident background log for the current servicing.
pub const TRIDENT_BACKGROUND_LOG_PATH: &str = "/var/log/trident-full.log";

/// Path to the Trident metrics file for the current servicing.
pub const TRIDENT_METRICS_FILE_PATH: &str = "/var/log/trident-metrics.jsonl";

/// Trident will by default prevent running Clean Install on deployments other
/// than from the Provisioning ISO, to limit chances of accidental data loss. To
/// override, user can create this file on the host.
const SAFETY_OVERRIDE_CHECK_PATH: &str = "/override-trident-safety-check";

/// Temporary location of the datastore for multiboot install scenarios.
pub const TEMPORARY_DATASTORE_PATH: &str = "/tmp/trident-datastore.sqlite";

/// Threshold in megabits per second for reporting slow streaming speeds. If
/// the streaming speed is below this threshold, it will be reported via in the
/// logs.
pub const STREAM_SLOW_SPEED_REPORTING_THRESHOLD_MBPS_DEFAULT: f64 = 15.0;

/// Interval in seconds for reporting slow streaming speeds. If the streaming
/// speed is below the threshold, it will be reported every this many seconds.
pub const STREAM_SLOW_SPEED_REPORTING_INTERVAL_SECONDS_DEFAULT: u64 = 10;

#[must_use]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExitKind {
    /// Requested operation completed successfully.
    Done,
    /// Reboot is needed to complete the operation.
    NeedsReboot,
}

pub struct Trident {
    host_config: Option<HostConfiguration>,
    orchestrator: Option<OrchestratorConnection>,
    is_stream_image: bool,
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

        // Trace features enabled in the Host Configuration.
        if let Some(hc) = &host_config {
            hc.feature_tracing();
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

        tracing::info!(metric_name = events::TRIDENT_START);

        Ok(Self {
            host_config,
            orchestrator,
            is_stream_image: false,
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

                validation::parse_host_config(&contents, Some(path))?
            }

            // Load the Host Configuration from a raw string.
            HostConfigurationSource::RawString(contents) => {
                info!("Loading Host Configuration from raw string");

                validation::parse_host_config(contents, None::<&Path>)?
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

    /// Records operation errors, preserving the original A/B validation error if recovery fails.
    fn execute_and_record_error<F, T>(
        &mut self,
        datastore: &mut DataStore,
        f: F,
    ) -> Result<T, TridentError>
    where
        F: FnOnce(&mut DataStore) -> Result<T, TridentError>,
    {
        // AbUpdateHealthCheckFailed and AbUpdateHypervisorCheckFailed are
        // special cases where the last error must be preserved across failed
        // recovery attempts. This keeps the original health/hypervisor failure
        // visible instead of replacing it with a secondary recovery error.
        debug!(
            "Execute and record error with servicing state: {:?} and last error: {:?}",
            datastore.host_status().servicing_state,
            datastore.host_status().last_error
        );
        let last_error_to_preserve = if matches!(
            datastore.host_status().servicing_state,
            ServicingState::AbUpdateHealthCheckFailed
                | ServicingState::AbUpdateHypervisorCheckFailed
        ) {
            datastore.host_status().last_error.clone()
        } else {
            None
        };

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
                let error = match last_error_to_preserve {
                    Some(err) => err,
                    None => {
                        // Create trace error for new errors.
                        tracing::error!(
                            error_kind = e.kind().as_str(),
                            error_subkind = e.subkind(),
                        );
                        serde_yaml::to_value(&e).structured(InternalError::SerializeError)?
                    }
                };
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
        match host_config.image {
            Some(ref mut image_source) => OsImage::load(image_source, cosi_timeout),
            None => Err(TridentError::new(InvalidInputError::MissingOsImage)),
        }
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

        let ctx = EngineContext::new(EngineContextParams {
            spec: host_status.spec.clone(),
            spec_old: host_status.spec_old.clone(),
            servicing_type: ServicingType::NoActiveServicing,
            is_stream_image: false,
            ab_active_volume: host_status.ab_active_volume,
            partition_paths: host_status.partition_paths.clone(),
            disk_uuids: host_status.disk_uuids.clone(),
            install_index: host_status.install_index,
            image: None,
            is_uki: None,
        })?;

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
        prefetched_image: Option<OsImage>,
    ) -> Result<(ExitKind, Option<Sha384Hash>, ServicingType), TridentError> {
        let finalize_only = !allowed_operations.has_stage() && allowed_operations.has_finalize();

        // If we are doing finalize only, we need to use the existing host
        // configuration from the datastore instead of the newly loaded file.
        let mut host_config = if finalize_only {
            // However, if we are doing a multiboot install and a persistent
            // datastore exists, we need to check if there is an ephemeral
            // datastore left around by the staging process.
            if multiboot && datastore.is_persistent() {
                // Reject a missing staged datastore before the existing loader can create it.
                let staged_datastore_path = temporary_datastore_path()?;
                let staged_datastore = fs::metadata(&staged_datastore_path)
                    .structured(ServicingError::Datastore {
                        inner: DatastoreError::LoadDatastore {
                            path: staged_datastore_path.display().to_string(),
                        },
                    })
                    .and_then(|_| DataStore::open(&staged_datastore_path))
                    .message("Failed to open staged multiboot datastore")?;

                // Close the current persistent datastore and replace it with the staged one.
                datastore.close();
                *datastore = staged_datastore;
            }

            // Use the host configuration from the ephemeral datastore.
            datastore.host_status().spec.clone()
        } else {
            // Use the newly provided host configuration.
            self.host_config
                .clone()
                .structured(InternalError::Internal(
                    "install called without Host Configuration set",
                ))?
        };

        let is_stream_image = self.is_stream_image;

        self.execute_and_record_error(datastore, |datastore| {
            if finalize_only {
                disk_selector::check_no_selectors(&host_config)?;
            }

            let storage_graph = host_config
                .validate_with_storage_graph()
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

                    let staged_datastore_path = temporary_datastore_path()?;
                    datastore.close();
                    *datastore = DataStore::open_or_create(&staged_datastore_path)
                        .message("Failed to create temporary datastore for multiboot install")?;
                }
            }

            // Use a prefetched image if provided, otherwise load the image
            // specified in the Host Configuration.
            let image = match prefetched_image {
                Some(image) => image,
                None => Self::get_cosi_image(&mut host_config)?,
            };
            let image_hash = Some(image.metadata_sha384());

            if allowed_operations.has_stage() {
                disk_selector::resolve_install(&mut host_config, &image, &storage_graph)?;
            }

            if datastore.host_status().spec != host_config {
                debug!("Host Configuration has been updated");

                if allowed_operations.has_stage() {
                    engine::clean_install(
                        &host_config,
                        datastore,
                        &allowed_operations,
                        multiboot,
                        image,
                        is_stream_image,
                    )
                    .message("Failed to execute a clean install")
                    .map(|ek| (ek, image_hash, ServicingType::CleanInstall))
                } else {
                    warn!(
                        "Host Configuration has been updated but allowed operations do not include \
                        'stage'. Add 'stage' and re-run to stage the clean install"
                    );

                    Ok((ExitKind::Done, None, ServicingType::NoActiveServicing))
                }
            } else {
                debug!("Host Configuration has not been updated");

                match datastore.host_status().servicing_state {
                    ServicingState::CleanInstallStaged => {
                        // If a clean install has been staged on the host, only need to finalize the
                        // clean install, if requested.
                        debug!("There is a clean install staged on the host");
                        if allowed_operations.has_finalize() {
                            engine::finalize_clean_install(datastore, None, None, is_stream_image)
                                .message("Failed to finalize clean install")
                                .map(|ek| (ek, image_hash.clone(), ServicingType::CleanInstall))
                        } else {
                            debug!(
                                "There is a clean install staged on the host, but allowed \
                                operations do not include 'finalize'. Add 'finalize' and re-run \
                                to finalize the clean install"
                            );

                            Ok((ExitKind::Done, None, ServicingType::NoActiveServicing))
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
                            is_stream_image,
                        )
                        .message("Failed to execute a clean install")
                        .map(|ek| (ek, image_hash, ServicingType::CleanInstall))
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

    /// Adopts untracked ACL hosts as needed, then stages/finalizes the update.
    pub fn update(
        &mut self,
        datastore: &mut DataStore,
        allowed_operations: Operations,
    ) -> Result<(ExitKind, Option<Sha384Hash>, ServicingType), TridentError> {
        let mut host_config = self
            .host_config
            .clone()
            .or_else(|| {
                if !allowed_operations.has_stage() && allowed_operations.has_finalize() {
                    Some(datastore.host_status().spec.clone())
                } else {
                    None
                }
            })
            .structured(InternalError::Internal(
                "update called without Host Configuration set",
            ))?;

        self.execute_and_record_error(datastore, |datastore| {
            // Ensure that the datastore exists.
            if !datastore.is_persistent() {
                if acl::is_acl().structured(InvalidInputError::DeriveHostConfiguration).message("Failed to determine if host is running ACL")? {
                    // For ACL, initialize datastore with known initial state when
                    // the datastore is not already created.
                    let initial_host_status = acl::initial_host_status()
                        .structured(InvalidInputError::DeriveHostConfiguration)
                        .message("Failed to initialize host status for ACL")?;

                    // Mirror offline-initialize's bootstrap: ACL lazy adoption derives
                    // HostStatus metadata only, it never touches the ESP. Seed EFI/AZLA from
                    // EFI/BOOT now so manual rollback's "AZLA must already exist" invariant
                    // holds for ACL-adopted hosts too.
                    let esp_path = initial_host_status
                        .spec
                        .storage
                        .filesystems
                        .iter()
                        .find(|fs| fs.is_esp)
                        .and_then(|fs| fs.mount_point_path())
                        .structured(InternalError::Internal(
                            "Failed to find ESP filesystem in ACL-derived Host Status",
                        ))?;

                    // Resolve the ESP path relative to the host filesystem root,
                    // matching the A/B update and manual-rollback paths. Without
                    // this, containerized Trident bootstraps AZLA under the
                    // container's own /boot instead of the host's ESP.
                    let esp_path = container::get_host_relative_path(esp_path.to_path_buf())
                        .message("Failed to resolve ESP path relative to host for ACL")?;

                    esp::bootstrap_azla_from_fallback(&esp_path)
                        .structured(InvalidInputError::InvalidBootConfiguration)
                        .message("Failed to bootstrap AZLA ESP path for ACL")?;

                    datastore
                        .with_host_status(|status| {
                            *status = initial_host_status;
                            status.is_management_os = false;
                        })
                        .message("Failed to initialize datastore")?;
                } else {
                    // For non-ACL images, if the datastore is not persistent, return error
                    return Err(TridentError::new(InvalidInputError::HostNotProvisioned))
                        .message("Persistent datastore not found on host");
                }
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
            let image_hash = Some(image.metadata_sha384());

            // Ensure that the cosi and host os_release align
            let host_os_release = OsRelease::read().unwrap_or_default();
            OsRelease::ensure_matching_distro(&host_os_release, image.os_release())
                .structured(InvalidInputError::MismatchedOsRelease)
                .message("Cannot run A/B Update due to OS release mismatch")?;

            // If HS.spec in the datastore is different from the new HC, need to both stage and
            // finalize the update, regardless of state
            if datastore.host_status().spec != host_config {
                debug!("Host Configuration has been updated");
                // If allowed operations include 'stage', start update
                if allowed_operations.has_stage() {
                    engine::update(&host_config, datastore, &allowed_operations, image)
                        .message("Failed to execute an update")
                        .map(|(ek, st)| (ek, image_hash, st))
                } else {
                    warn!("Host Configuration has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run to stage the update");
                    Ok((ExitKind::Done, None, ServicingType::NoActiveServicing))
                }
            } else {
                debug!("Host Configuration has not been updated");

                match datastore.host_status().servicing_state {
                    ServicingState::AbUpdateStaged => {
                        // If an A/B update has been previously staged, only need to finalize the update.
                        debug!("There is an A/B update staged on the host");
                        if allowed_operations.has_finalize() {
                            ab_update::finalize_update(
                                datastore,
                                None,
                            )
                            .message("Failed to finalize A/B update")
                            .map(|ek| (ek, image_hash.clone(), ServicingType::AbUpdate))
                        } else {
                            warn!("There is an A/B update staged on the host, but allowed operations do not include 'finalize'. Add 'finalize' and re-run to finalize the A/B update");
                            Ok((ExitKind::Done, None, ServicingType::NoActiveServicing))
                        }
                    }
                    ServicingState::RuntimeUpdateStaged => {
                        // If a runtime update has been previously staged, only need to finalize the update.
                        debug!("There is a runtime update staged on the host");
                        if allowed_operations.has_finalize() {
                            let mut subsystems = SUBSYSTEMS.lock().unwrap();
                            runtime_update::finalize_update(
                                &mut subsystems,
                                datastore,
                                None,
                            )
                            .map(|ek| (ek, image_hash.clone(), ServicingType::RuntimeUpdate))
                        } else {
                            warn!("There is a runtime update staged on the host, but allowed operations do not include 'finalize'. Add 'finalize' and re-run to finalize the runtime update");
                            Ok((ExitKind::Done, None, ServicingType::NoActiveServicing))
                        }
                    }
                    ServicingState::AbUpdateFinalized | ServicingState::Provisioned => {
                        // Need to either re-execute the failed update OR inform the user that no update
                        // is needed.
                        engine::update(&host_config, datastore, &allowed_operations, image)
                            .message("Failed to update host")
                            .map(|(ek, st)| (ek, image_hash, st))
                    }
                    servicing_state => {
                        Err(TridentError::new(InternalError::UnexpectedServicingState {
                            state: servicing_state,
                        }))
                    }
                }
            }
        })
    }

    pub fn stream_image(
        &mut self,
        datastore: &mut DataStore,
        image_url: &Url,
        hash: &str,
    ) -> Result<(ExitKind, Option<Sha384Hash>, ServicingType), TridentError> {
        tracing::info!(metric_name = events::STREAM_IMAGE_START, value = true,);
        let mut image_source = ConfigOsImage {
            url: image_url.clone(),
            sha384: ImageSha384::new(hash)?,
        };

        let mut image = OsImage::load(&mut image_source, Duration::from_secs(10))
            .message("Failed to download OS image")?;

        let original_disk_size = image
            .original_disk_size()
            .structured(InvalidInputError::DeriveHostConfiguration)
            .message(
                "Image does not contain disk metadata; streaming requires a COSI v1.2 or newer image with disk information.",
            )?;

        let mut config = image
            .derive_host_configuration("/dev/sda") // Use /dev/sda as a placeholder.
            .structured(InvalidInputError::DeriveHostConfiguration)
            .message("Host Configuration cannot be derived from this OS image.")?
            .structured(InvalidInputError::DeriveHostConfiguration)?;

        // Sanity check the derived Host Configuration
        config
            .validate()
            .map_err(|e| TridentError::new(InternalError::from(e)))?;

        stream::update_target_disk_path(
            &mut config,
            original_disk_size,
            DiskSelectionStrategy::SmallestThatWillFit,
        )?;

        self.host_config = Some(config);
        self.is_stream_image = true;

        self.install(datastore, Operations::all(), false, Some(image))
    }

    /// Validates finalized servicing/recovery and returns whether a reboot is needed.
    pub fn commit(
        &mut self,
        datastore: &mut DataStore,
    ) -> Result<(ExitKind, ServicingType), TridentError> {
        // If the host's servicing state is *Finalized, AbUpdateHealthCheckFailed,
        // or AbUpdateHypervisorCheckFailed, re-evaluate the current state of
        // the host to validate either the target boot or the recovery boot.
        let servicing_state = datastore.host_status().servicing_state;
        if !matches!(
            servicing_state,
            ServicingState::CleanInstallFinalized
                | ServicingState::AbUpdateFinalized
                | ServicingState::AbUpdateHealthCheckFailed
                | ServicingState::AbUpdateHypervisorCheckFailed
                | ServicingState::ManualRollbackAbFinalized
        ) {
            info!(
                "No servicing in progress ({:?}), skipping commit",
                servicing_state
            );
            return Ok((ExitKind::Done, ServicingType::NoActiveServicing));
        }

        let servicing_type = match servicing_state {
            ServicingState::AbUpdateFinalized
            | ServicingState::AbUpdateHealthCheckFailed
            | ServicingState::AbUpdateHypervisorCheckFailed => ServicingType::AbUpdate,
            ServicingState::CleanInstallFinalized => ServicingType::CleanInstall,
            ServicingState::ManualRollbackAbFinalized => ServicingType::ManualRollbackAb,
            _ => ServicingType::NoActiveServicing,
        };

        let rollback_result = self.execute_and_record_error(datastore, |datastore| {
            rollback::validate_boot(datastore).message(
                "Failed to validate that firmware correctly booted from updated target OS image",
            )
        });

        // Report success only after provisioning, not when rollback is required.
        if matches!(
            rollback_result,
            Ok(BootValidationResult::ValidBootProvisioned)
        ) {
            if let Some(ref orchestrator) = self.orchestrator {
                orchestrator.report_success(Some(
                    serde_yaml::to_string(&datastore.host_status())
                        .unwrap_or("Failed to serialize Host Status".into()),
                ))
            }
        }

        match rollback_result {
            Ok(BootValidationResult::ValidBootProvisioned) => Ok((ExitKind::Done, servicing_type)),
            Ok(BootValidationResult::InvalidHypervisor(e))
            | Ok(BootValidationResult::ValidBootHealthCheckFailed(e)) => {
                // The caller must reboot to complete automatic recovery.
                debug!("Boot validation requires rollback: {e:?}");
                Ok((ExitKind::NeedsReboot, servicing_type))
            }
            Err(e) => {
                error!("Boot validation failed: {e:?}");
                Err(e)
            }
        }
    }

    pub fn get(
        datastore_path: &Path,
        output_path: &Option<PathBuf>,
        kind: GetKind,
    ) -> Result<(), TridentError> {
        let datastore = DataStore::open(datastore_path).message("Failed to open datastore")?;
        let host_status = datastore.host_status().clone();

        let yaml = match kind {
            GetKind::Configuration => serde_yaml::to_string(&host_status.spec)
                .structured(InternalError::SerializeHostStatus)?,
            GetKind::Status => serde_yaml::to_string(&host_status)
                .structured(InternalError::SerializeHostStatus)?,
            GetKind::LastError => serde_yaml::to_string(&host_status.last_error)
                .structured(InternalError::SerializeError)?,
            GetKind::RollbackTarget | GetKind::RollbackChain => {
                manual_rollback::get_rollback_info(&datastore, kind)?
            }
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

    /// Handle a manual rollback request. Either print information about
    /// available rollbacks, or execute a rollback.
    ///
    /// Returns the exit kind and the resulting servicing type, i.e.
    /// `ServicingType::NoActiveServicing` if no rollback was performed.
    pub fn rollback(
        &mut self,
        datastore: &mut DataStore,
        invoke_if_next_is_runtime: bool,
        invoke_available_ab: bool,
        allowed_operations: Operations,
    ) -> Result<(ExitKind, ServicingType), TridentError> {
        // If host's servicing state is not in Provisioned or ManualRollback*, cannot
        // execute a rollback.
        if !matches!(
            datastore.host_status().servicing_state,
            ServicingState::Provisioned
                | ServicingState::ManualRollbackAbStaged
                | ServicingState::ManualRollbackRuntimeStaged
        ) {
            info!(
                "Cannot trigger rollback from current state ({:?})",
                datastore.host_status().servicing_state
            );
            return Ok((ExitKind::Done, ServicingType::NoActiveServicing));
        }

        let rollback_result = self.execute_and_record_error(datastore, |datastore| {
            manual_rollback::execute_rollback(
                datastore,
                ManualRollbackRequestKind::from_flags(
                    invoke_if_next_is_runtime,
                    invoke_available_ab,
                )?,
                &allowed_operations,
            )
            .message("Failed to rollback")
        });

        if rollback_result.is_ok() {
            if let Some(ref orchestrator) = self.orchestrator {
                orchestrator.report_success(Some(
                    serde_yaml::to_string(&datastore.host_status())
                        .unwrap_or("Failed to serialize Host Status".into()),
                ))
            }
        }

        rollback_result
    }

    // Handle diagnose command
    pub fn diagnose(output_path: &Path, full: bool, selinux: bool) -> Result<(), TridentError> {
        diagnostics::generate_and_bundle(output_path, full, selinux)?;
        Ok(())
    }
}

/// Keeps multiboot state reachable across separate container invocations.
pub(crate) fn temporary_datastore_path() -> Result<PathBuf, TridentError> {
    container::get_host_relative_path(TEMPORARY_DATASTORE_PATH.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    use trident_api::{
        config::{Disk, DiskSelector, HostConfigurationStaticValidationError, Operation, Storage},
        error::ErrorKind,
    };

    #[test]
    fn finalize_only_rejects_saved_selectors() {
        let saved = HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "resolved".into(),
                        device: "/dev/sda".into(),
                        ..Default::default()
                    },
                    Disk {
                        id: "saved-selector".into(),
                        selector: Some(DiskSelector::default()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        for supplied in [None, Some(HostConfiguration::default())] {
            let directory = TempDir::new().unwrap();
            let mut datastore =
                DataStore::open_or_create(&directory.path().join("datastore.sqlite")).unwrap();
            datastore
                .with_host_status(|status| {
                    status.servicing_state = ServicingState::CleanInstallStaged;
                    status.spec = saved.clone();
                })
                .unwrap();
            let mut trident = Trident {
                host_config: supplied,
                orchestrator: None,
                is_stream_image: false,
            };

            let error = trident
                .install(&mut datastore, Operation::Finalize.into(), false, None)
                .unwrap_err();
            assert_eq!(
                error.kind(),
                &ErrorKind::InvalidInput(InvalidInputError::DiskSelection {
                    disk_id: "saved-selector".into(),
                })
            );
            assert_eq!(&datastore.host_status().spec, &saved);
            assert_eq!(
                datastore.host_status().servicing_state,
                ServicingState::CleanInstallStaged
            );
        }
    }

    #[test]
    fn finalize_only_uses_saved_host_configuration() {
        // Fail validation before image loading or disk I/O.
        let saved = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "saved".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let ignored = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "ignored".into(),
                    selector: Some(DiskSelector::default()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        for supplied in [None, Some(ignored)] {
            let directory = TempDir::new().unwrap();
            let mut datastore =
                DataStore::open_or_create(&directory.path().join("datastore.sqlite")).unwrap();
            datastore
                .with_host_status(|status| {
                    status.servicing_state = ServicingState::CleanInstallStaged;
                    status.spec = saved.clone();
                })
                .unwrap();
            let mut trident = Trident {
                host_config: supplied,
                orchestrator: None,
                is_stream_image: false,
            };

            let error = trident
                .install(&mut datastore, Operation::Finalize.into(), false, None)
                .unwrap_err();
            assert_eq!(
                error.kind(),
                &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationStatic {
                    inner: HostConfigurationStaticValidationError::InvalidDiskTarget {
                        disk_id: "saved".into(),
                    },
                })
            );
            assert_eq!(
                datastore.host_status().servicing_state,
                ServicingState::CleanInstallStaged
            );
        }
    }
}
