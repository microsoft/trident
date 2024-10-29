use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use engine::storage::rebuild;
use log::{debug, error, info, warn};
use nix::unistd::Uid;
use tokio::sync::mpsc::{self};

use osutils::{
    container,
    efibootmgr::{self, EfiBootManagerOutput},
    path,
};
use trident_api::{
    config::{HostConfiguration, HostConfigurationSource, LocalConfigFile, Operations},
    status::AbVolumeSelection,
};
use trident_api::{
    constants::ROOT_MOUNT_POINT_PATH,
    error::{
        ExecutionEnvironmentMisconfigurationError, InitializationError, InternalError,
        InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt,
    },
    status::{HostStatus, ServicingState, ServicingType},
};

#[cfg(feature = "setsail")]
use setsail::KsTranslator;

use crate::datastore::DataStore;
use crate::engine::{bootentries, storage::tabfile};

mod datastore;
mod engine;
mod logging;
pub mod offline_init;
mod orchestrate;
pub mod osimage;
mod subsystems;

#[cfg(feature = "grpc-dangerous")]
mod grpc;

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

/// Path to the mountinfo file in the host's proc directory that contains information about the
/// host's mount points.
const PROC_MOUNTINFO_PATH: &str = "/proc/self/mountinfo";

/// Default Trident configuration file path.
pub const TRIDENT_LOCAL_CONFIG_PATH: &str = "/etc/trident/config.yaml";

/// Location to store the Trident datastore on the provisioning OS.
pub const TRIDENT_TEMPORARY_DATASTORE_PATH: &str = "/var/lib/trident/tmp-datastore.sqlite";

/// Trident binary path.
pub const TRIDENT_BINARY_PATH: &str = "/usr/bin/trident";

/// OS Modifier (EMU) binary path.
pub const OS_MODIFIER_BINARY_PATH: &str = "/usr/bin/osmodifier";

/// Path to the Trident background log for the current servicing.
pub const TRIDENT_BACKGROUND_LOG_PATH: &str = "/var/log/trident-full.log";

/// Trident will by default prevent running Clean Install on deployments other
/// than from the Provisioning ISO, to limit chances of accidental data loss. To
/// override, user can create this file on the host.
const SAFETY_OVERRIDE_CHECK_PATH: &str = "/override-trident-safety-check";

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
                .structured(InitializationError::ConnectToLogstream)?;
        }

        // Set up tracestream if configured, using phonehome url for now
        if let Some(url) = config.phonehome.as_ref() {
            let trace_url = url.clone().replace("phonehome", "tracestream");
            tracestream
                .set_server(trace_url)
                .structured(InitializationError::ConnectToTracestream)?;
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
            .structured(InvalidInputError::GetHostConfigurationSource)?
            .as_ref()
            .map(Self::load_host_config)
            .transpose()
    }

    fn load_host_config(
        source: &HostConfigurationSource,
    ) -> Result<Box<HostConfiguration>, TridentError> {
        let host_config = match source {
            // Load the host configuration from a file.
            HostConfigurationSource::File(path) => {
                info!("Loading host config from file at path '{}'", path.display());

                serde_yaml::from_str(&fs::read_to_string(path).structured(
                    InvalidInputError::LoadHostConfigurationFile {
                        path: path.display().to_string(),
                    },
                )?)
                .structured(InvalidInputError::ParseHostConfigurationFile {
                    path: path.display().to_string(),
                })?
            }

            // Use the embedded host configuration.
            HostConfigurationSource::Embedded(contents) => contents.clone(),

            // When enabled, load a kickstart body from the local config and translate it to a host
            // configuration.
            #[cfg(feature = "setsail")]
            HostConfigurationSource::KickstartEmbedded(contents) => Box::new(
                KsTranslator::new()
                    .run_pre_scripts(true)
                    .translate(setsail::load_kickstart_string(contents))
                    .structured(InvalidInputError::TranslateKickstart)?,
            ),

            // When enabled, load a kickstart file from the local config and translate it to a host
            // configuration.
            #[cfg(feature = "setsail")]
            HostConfigurationSource::KickstartFile(ref file) => Box::new(
                KsTranslator::new()
                    .run_pre_scripts(true)
                    .translate(setsail::load_kickstart_file(file).structured(
                        InvalidInputError::LoadKickstart {
                            path: file.display().to_string(),
                        },
                    )?)
                    .structured(InvalidInputError::TranslateKickstart)?,
            ),
        };

        info!(
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
        #[cfg(feature = "setsail")]
        if let Some(
            HostConfigurationSource::KickstartFile(_)
            | HostConfigurationSource::KickstartEmbedded(_),
        ) = self
            .config
            .get_host_configuration_source()
            .structured(InvalidInputError::GetHostConfigurationSource)?
        {
            warn!("Cannot set up network early when using kickstart");
            return Ok(());
        }

        let host_config = Self::get_host_configuration(&self.config)?;

        info!("Starting network");
        provisioning_network::start(
            self.config.network_override.clone(),
            host_config.as_deref(),
            self.config.wait_for_provisioning_network,
        )
        .structured(ServicingError::StartNetwork)?;

        Ok(())
    }

    pub fn run(&mut self) -> Result<(), TridentError> {
        let orchestrator = self
            .config
            .phonehome
            .as_ref()
            .and_then(|url| OrchestratorConnection::new(url.clone()));

        info!("Running Trident version: {}", TRIDENT_VERSION);

        if container::is_running_in_container().unwrap_or(false) {
            info!("Running Trident in a container");
        }

        if !Uid::effective().is_root() {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::CheckRootPrivileges,
            ));
        }

        // This creates a channel to send commands to the main trident thread. It lets us use the
        // same logic for processing an initial provision command contained within the trident local
        // config as for processing commands received from the gRPC endpoint.
        let (sender, receiver) = tokio::sync::mpsc::channel(1);

        // If we have a host config source, load it and dispatch it as the first
        // command.
        if let Some(host_config) = Self::get_host_configuration(&self.config)? {
            debug!("Applying host configuration from local config");
            sender
                .blocking_send(HostUpdateCommand {
                    allowed_operations: self.config.allowed_operations.clone(),
                    host_config: *host_config,
                    #[cfg(feature = "grpc-dangerous")]
                    sender: None,
                })
                .structured(InternalError::EnqueueHostUpdateCommand)?;
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

        let mut datastore = match self.config.datastore {
            Some(ref datastore_path) => DataStore::open(datastore_path)?,
            None => DataStore::open_temporary().message("Failed to open temporary datastore")?,
        };

        if let Err(e) = self.handle_commands(receiver, &orchestrator, &mut datastore) {
            let error = serde_yaml::to_value(&e).structured(InternalError::SerializeError)?;
            if let Err(e2) = datastore.with_host_status(|status| status.last_error = Some(error)) {
                error!("Failed to record error in datastore: {e2:?}");
            }

            return Err(e);
        }

        if let Some(ref orchestrator) = orchestrator {
            orchestrator.report_success(Some(
                serde_yaml::to_string(&datastore.host_status())
                    .unwrap_or("Failed to serialize host status".into()),
            ))
        }
        Ok(())
    }

    /// Rebuilds RAID devices on replaced disks on the host
    pub fn rebuild_raid(&mut self) -> Result<(), TridentError> {
        info!("Rebuilding RAID devices");
        if !Uid::effective().is_root() {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::CheckRootPrivileges,
            ));
        }
        // If we have a host config source load it or else fail
        let binding = Self::get_host_configuration(&self.config)?;
        // Unbox host configuration
        let host_config = binding
            .as_deref()
            .structured(InitializationError::LoadLocalConfig)?;

        let mut datastore = match self.config.datastore {
            Some(ref datastore_path) => DataStore::open(datastore_path)?,
            None => {
                return Err(TridentError::new(
                    InternalError::GetDatastorePathFromLocalTridentConfig,
                ))
            }
        };

        datastore
            .with_host_status(|host_status| {
                // Validate the loaded host config and rebuild RAID devices
                rebuild::validate_and_rebuild_raid(host_config, host_status)
            })?
            .structured(ServicingError::ValidateAndRebuildRaid)?;

        Ok(())
    }

    fn handle_commands(
        &mut self,
        mut receiver: mpsc::Receiver<HostUpdateCommand>,
        orchestrator: &Option<OrchestratorConnection>,
        datastore: &mut DataStore,
    ) -> Result<(), TridentError> {
        debug!(
            "Current servicing type: {:?}",
            datastore.host_status().servicing_type
        );
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

        // If host's servicing state is Finalized, need to verify if firmware correctly booted from
        // the updated runtime OS image.
        if datastore.host_status().servicing_state == ServicingState::Finalized {
            // If Trident is running inside a container, need to get the device path for the mount
            // point at '/host', since the host's root path is mounted at '/host' inside the
            // container.
            let root_mount_path = if container::is_running_in_container()
                .message("Running in container check failed")?
            {
                let host_root_path =
                    container::get_host_root_path().message("Failed to get host's root path")?;
                debug!(
                    "Running inside a container. Using root mount path '{}'",
                    host_root_path.display()
                );
                host_root_path
            } else {
                debug!(
                    "Not running inside a container. Using default root mount path '{}'",
                    ROOT_MOUNT_POINT_PATH
                );
                Path::new(ROOT_MOUNT_POINT_PATH).to_path_buf()
            };

            // Get device path of root mount point. Contents of '/host/proc/self/mountinfo' and
            // '/proc/self/mountinfo' are identical, so we use the latter by default.
            let root_dev_path =
                match tabfile::get_device_path(Path::new(PROC_MOUNTINFO_PATH), &root_mount_path) {
                    Ok(path) => path,
                    Err(e) => {
                        error!("Failed to get device path of root mount point: {e}");
                        return Err(TridentError::new(ServicingError::RootMountPointDevPath {
                            mountinfo_file: PROC_MOUNTINFO_PATH.to_string(),
                        }));
                    }
                };

            // Get expected device path of root mount point
            let expected_root_dev_path = get_expected_root_device_path(datastore.host_status())?;

            info!("Validating whether host correctly booted into the updated runtime OS image");
            if validate_reboot(root_dev_path.clone(), expected_root_dev_path.clone())
                .message("Host failed to boot from the expected root device")?
            {
                info!("Host correctly booted into the updated runtime OS image");

                // If it's QEMU, after confirming that we have booted into the
                // correct image, we set the `BootCurrent` entry as the first
                // entry in `BootOrder`.
                if osutils::virt::is_qemu() {
                    // Get `BootCurrent` from the boot manager output.
                    let bootmgr_output: EfiBootManagerOutput =
                        efibootmgr::list_and_parse_bootmgr_entries()
                            .structured(ServicingError::ListAndParseBootEntries)?;
                    let boot_current = &bootmgr_output.boot_current;

                    // Modify `BootOrder` to have `BootCurrent` as the first entry.
                    bootentries::first_boot_order(boot_current).structured(
                        ServicingError::SetBootOrder {
                            boot_entry_number: boot_current.to_string(),
                        },
                    )?;
                }
            } else if datastore.host_status().servicing_type == ServicingType::CleanInstall {
                datastore.with_host_status(|host_status| {
                    host_status.servicing_type = ServicingType::NoActiveServicing;
                    host_status.servicing_state = ServicingState::CleanInstallFailed;
                })?;

                return Err(TridentError::new(ServicingError::CleanInstallRebootCheck {
                    root_device_path: root_dev_path.to_string_lossy().to_string(),
                    expected_device_path: expected_root_dev_path.to_string_lossy().to_string(),
                }));
            } else {
                datastore.with_host_status(|host_status| {
                    host_status.servicing_type = ServicingType::NoActiveServicing;
                    host_status.servicing_state = ServicingState::AbUpdateFailed;
                })?;

                return Err(TridentError::new(ServicingError::AbUpdateRebootCheck {
                    root_device_path: root_dev_path.to_string_lossy().to_string(),
                    expected_device_path: expected_root_dev_path.to_string_lossy().to_string(),
                }));
            }

            if datastore.host_status().servicing_type == ServicingType::CleanInstall {
                info!(
                    "Clean install of runtime OS succeeded. Setting servicing state to Provisioned"
                );
                tracing::info!(metric_name = "clean_install_success", value = true);
            } else {
                info!("A/B update succeeded. Setting servicing state to Provisioned");
                tracing::info!(metric_name = "ab_update_success", value = true);
            }
            datastore.with_host_status(|host_status| {
                host_status.servicing_state = ServicingState::Provisioned;
                host_status.servicing_type = ServicingType::NoActiveServicing;
                host_status.spec_old = Default::default();
                host_status.ab_active_volume = match host_status.ab_active_volume {
                    None | Some(AbVolumeSelection::VolumeB) => Some(AbVolumeSelection::VolumeA),
                    Some(AbVolumeSelection::VolumeA) => Some(AbVolumeSelection::VolumeB),
                };
            })?;
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

        Ok(())
    }

    fn handle_command(
        &mut self,
        datastore: &mut DataStore,
        mut cmd: HostUpdateCommand,
    ) -> Result<(), TridentError> {
        if self.config.phonehome.is_some() && cmd.host_config.trident.phonehome.is_none() {
            debug!("Injecting phonehome into host configuration");
            cmd.host_config.trident.phonehome = self.config.phonehome.clone();
        }

        cmd.host_config.validate().map_err(|e| {
            TridentError::new(InvalidInputError::InvalidHostConfigurationStatic { inner: e })
        })?;

        // Populate internal fields in host configuration.
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
                debug!("Host config has been updated");
                // If allowed operations include 'stage', start update
                if cmd.allowed_operations.has_stage() {
                    engine::update(cmd, datastore).message("Failed to run update()")
                } else {
                    warn!("Host config has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run");
                    Ok(())
                }
            } else {
                debug!("Host config has not been updated");

                // If host config has not been updated and the previous A/B update failed, ask the
                // user to update HC and re-run
                if datastore.host_status().servicing_state == ServicingState::AbUpdateFailed {
                    error!("Previous A/B update failed with current host config. Update host config and re-run");
                    return Err(TridentError::new(
                        InvalidInputError::RerunAbUpdateWithFailedHostConfiguration,
                    ));
                }

                // If an update has been previously staged, only need to finalize the update
                if datastore.host_status().servicing_state == ServicingState::Staged {
                    debug!("There is an update staged on the host");
                    if cmd.allowed_operations.has_finalize() {
                        engine::finalize_update(
                            datastore,
                            None,
                            #[cfg(feature = "grpc-dangerous")]
                            &mut cmd.sender,
                        )
                        .message("Failed to run finalize_update()")
                    } else {
                        debug!("Allowed operations do not include 'finalize'. Skipping finalizing of update");
                        Ok(())
                    }
                } else {
                    // If servicing state is Provisioned, need to refresh host status. If servicing
                    // state is Staging, need to re-do update.
                    //
                    // State cannot be NotProvisioned or Finalized here; Staged and AbUpdateFailed
                    // were addressed above
                    engine::update(cmd, datastore).message("Failed to run update()")
                }
            }
        } else {
            // If datastore is temporary, firmware booted from prov OS, so can only do clean
            // install.
            //
            // If HS.spec in the datastore is different from the new HC, need to both stage and
            // finalize the clean install
            if datastore.host_status().spec != cmd.host_config {
                debug!("Host config has been updated");

                if cmd.allowed_operations.has_stage() {
                    engine::clean_install(cmd, datastore).message("Failed to run clean_install()")
                } else {
                    warn!("Host config has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run");
                    Ok(())
                }
            } else {
                debug!("Host config has not been updated");

                // If host config has not been updated and the previous clean install attempt
                // failed, ask the user to update HC and re-run
                if datastore.host_status().servicing_state == ServicingState::CleanInstallFailed {
                    error!("Previous clean install attempt failed with current host config. Update host config and re-run");
                    return Err(TridentError::new(
                        InvalidInputError::RerunCleanInstallWithFailedHostConfiguration,
                    ));
                }

                // If HS.spec matches the new HS, only need to finalize the clean install
                if datastore.host_status().servicing_state == ServicingState::Staged {
                    debug!("There is a clean install staged on the host");
                    if cmd.allowed_operations.has_finalize() {
                        // Remount new root and custom mounts if we're finalizing a clean install
                        let root_mount = engine::initialize_new_root(datastore.host_status())
                            .message("Failed to remount new root")?;

                        engine::finalize_clean_install(
                            datastore,
                            root_mount,
                            None,
                            #[cfg(feature = "grpc-dangerous")]
                            &mut cmd.sender,
                        )
                        .message("Failed to run finalize_clean_install()")
                    } else {
                        debug!("Allowed operations do not include 'finalize'. Skipping finalizing of clean install");
                        Ok(())
                    }
                } else {
                    // If servicing state is Staging, need to re-do update.
                    //
                    // State cannot be NotProvisioned, Provisioned, AbUpdateFailed, or
                    // Finalized here; Staged and CleanInstallFailed were addressed above.
                    engine::clean_install(cmd, datastore).message("Failed to run clean_install()")
                }
            }
        }
    }

    pub fn retrieve_host_status(&mut self, output_path: &Option<PathBuf>) -> Result<(), Error> {
        let host_status = if let Some(ref datastore_path) = self.config.datastore {
            debug!("Opening persistent datastore");
            DataStore::open(datastore_path)
                .unstructured("Failed to open persistent datastore")?
                .host_status()
                .clone()
        } else if Path::new(TRIDENT_TEMPORARY_DATASTORE_PATH).exists() {
            debug!("Opening temporary datastore");
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

/// Returns the path of the root device that the host was expected to boot from, to finalize a
/// clean install or an A/B update.
fn get_expected_root_device_path(host_status: &HostStatus) -> Result<PathBuf, TridentError> {
    // Fetch mount point for root from host status and fetch ID of root device
    let root_device_id = match host_status
        .spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
    {
        Some(mp) => &mp.target_id,
        None => {
            return Err(TridentError::new(ServicingError::GetRootMountPointInfo {
                root_path: ROOT_MOUNT_POINT_PATH.to_string(),
            }));
        }
    };

    // Fetch the expected root device path from host status, based on device ID of root. Set
    // active=false b/c need to fetch info for volume that we expect to be active at this point,
    // after host has already rebooted, and it used to be the update volume before the reboot.
    let expected_root_path = engine::get_block_device_path_hs(host_status, root_device_id)
        .structured(ServicingError::GetBlockDevicePath {
            device_id: root_device_id.to_string(),
        })?;

    Ok(expected_root_path)
}

/// Validates whether the host rebooted from the expected runtime OS image by comparing the root
/// device path with the expected root device path. Returns true if the host booted from the
/// expected root device path, false otherwise, or an error if the expected root device path cannot
/// be canonicalized, making a comparison impossible.
///
/// This function is called after the host rebooted to finalize a clean install or an A/B update.
///
#[tracing::instrument(skip_all)]
fn validate_reboot(
    root_dev_path: PathBuf,
    expected_root_dev_path: PathBuf,
) -> Result<bool, TridentError> {
    let expected_root_path_canonicalized =
        expected_root_dev_path
            .canonicalize()
            .structured(ServicingError::CanonicalizePath {
                path: expected_root_dev_path.display().to_string(),
            })?;

    info!(
        "Expected host to boot from device with path '{}', canonicalized to '{}'",
        expected_root_dev_path.display(),
        expected_root_path_canonicalized.display()
    );

    // If current root device path is NOT the same as the expected root device path, return false.
    // NOTE: For verity, path like /dev/mapper/root, which is what we use in host status, gets
    // resolved to /dev/dm-0. So, compare if matches either.
    if root_dev_path != expected_root_dev_path && root_dev_path != expected_root_path_canonicalized
    {
        info!(
            "But host booted from an unexpected root device with path '{}'",
            root_dev_path.display()
        );

        return Ok(false);
    }

    info!("Host booted from the expected root device");

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use maplit::btreemap;

    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystemType, InternalMountPoint, Partition,
            PartitionType, Storage,
        },
        error::ErrorKind,
        status::AbVolumeSelection,
    };

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
                internal_mount_points: vec![InternalMountPoint {
                    path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
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

    #[test]
    fn test_get_expected_root_device_path() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "esp".to_owned(),
                                size: 100.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root-a".to_owned(),
                                size: 900.into(),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "root-b".to_owned(),
                                size: 9000.into(),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "root".to_string(),
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            servicing_state: ServicingState::Finalized,
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        // Test case #0: If no mount points defined, should return an error.
        assert_eq!(
            get_expected_root_device_path(&host_status)
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::GetRootMountPointInfo {
                root_path: ROOT_MOUNT_POINT_PATH.to_string()
            })
        );

        // Test case #1: If no root ID in block devices, should return an error.
        host_status.spec.storage.internal_mount_points = vec![InternalMountPoint {
            path: PathBuf::from("/"),
            target_id: "root".to_string(),
            filesystem: FileSystemType::Ext4,
            options: vec![],
        }];
        assert_eq!(
            get_expected_root_device_path(&host_status)
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::GetBlockDevicePath {
                device_id: "root".to_string()
            })
        );

        // Test case #3: When block devices are defined, should return the expected root device
        // path of 'root-a'.
        host_status.block_device_paths = btreemap! {
            "os".to_owned() => PathBuf::from("/dev/sda"),
            "efi".to_owned() => PathBuf::from("/dev/sda1"),
            "root-a".to_owned() => PathBuf::from("/dev/sda2"),
            "root-b".to_owned() => PathBuf::from("/dev/sda3"),
        };
        assert_eq!(
            get_expected_root_device_path(&host_status).unwrap(),
            PathBuf::from("/dev/sda2")
        );

        // Test case #4: After rebooting after an A/B update, should return the expected root
        // device path of 'root-b'.
        host_status.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        host_status.servicing_type = ServicingType::AbUpdate;
        host_status.servicing_state = ServicingState::Finalized;
        assert_eq!(
            get_expected_root_device_path(&host_status).unwrap(),
            PathBuf::from("/dev/sda3")
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    /// Validates that validate_reboot() correctly detects rollback when root is a partition.
    #[functional_test]
    fn test_validate_reboot() {
        // Test case #0: If current root device path is the same as the expected root device path,
        // should return true.
        assert!(validate_reboot(PathBuf::from("/dev/sda2"), PathBuf::from("/dev/sda2")).unwrap());

        // Test case #1: If current root device path is NOT the same as the expected root device
        // path, should return false.
        assert!(!validate_reboot(PathBuf::from("/dev/sda2"), PathBuf::from("/dev/sda3")).unwrap());
    }
}
