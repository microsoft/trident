use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use engine::storage::rebuild;
use log::{debug, error, info, warn};
use nix::unistd::Uid;
use tokio::sync::mpsc::{self};

use osutils::{block_devices, container};
use trident_api::{
    config::{GrpcConfiguration, HostConfiguration, HostConfigurationSource, Operations},
    status::AbVolumeSelection,
};
use trident_api::{
    constants::ROOT_MOUNT_POINT_PATH,
    error::{
        ExecutionEnvironmentMisconfigurationError, InitializationError, InternalError,
        InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt,
    },
    status::{ServicingState, ServicingType},
};

#[cfg(feature = "setsail")]
use setsail::KsTranslator;

use crate::datastore::DataStore;
use crate::engine::{
    bootentries,
    storage::{image, verity},
    EngineContext,
};

mod datastore;
mod engine;
mod logging;
pub mod offline_init;
mod orchestrate;
pub mod osimage;
mod subsystems;
pub mod validation;

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

        let (phonehome_url, logstream_url) = if let Some(config) = &host_config {
            (
                config.trident.phonehome.clone(),
                config.trident.logstream.clone(),
            )
        } else if let Ok(datastore) = DataStore::open(datastore_path) {
            let host_config = &datastore.host_status().spec;
            (
                host_config.trident.phonehome.clone(),
                host_config.trident.logstream.clone(),
            )
        } else {
            (None, None)
        };

        // Set up logstream if configured
        if let Some(url) = logstream_url {
            logstream
                .set_server(url.to_string())
                .structured(InitializationError::ConnectToLogstream)?;
        }

        let orchestrator = phonehome_url
            .as_ref()
            .and_then(|url| OrchestratorConnection::new(url.clone()));

        // Set up tracestream if configured, using phonehome url for now
        if let Some(url) = phonehome_url {
            let trace_url = url.clone().replace("phonehome", "tracestream");
            tracestream
                .set_server(trace_url)
                .structured(InitializationError::ConnectToTracestream)?;
        }

        debug!(
            "Trident config:\n{}",
            serde_yaml::to_string(&host_config)
                .unwrap_or("Failed to serialize Host Configuration".into())
        );

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

        // This creates a channel to send commands to the main trident thread. It lets us use the
        // same logic for processing an initial provision command contained within the trident local
        // config as for processing commands received from the gRPC endpoint.
        let (sender, receiver) = tokio::sync::mpsc::channel(1);

        // If we have a Host Configuration source, load it and dispatch it as the first
        // command.
        if let Some(host_config) = self.host_config.clone() {
            debug!("Applying Host Configuration from local config");
            sender
                .blocking_send(HostUpdateCommand {
                    allowed_operations,
                    host_config,
                    #[cfg(feature = "grpc-dangerous")]
                    sender: None,
                })
                .structured(InternalError::EnqueueHostUpdateCommand)?;
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

        let mut datastore =
            DataStore::open_or_create(datastore_path).message("Failed to open datastore")?;

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
                    .unwrap_or("Failed to serialize host status".into()),
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

        DataStore::open(datastore_path)?
            .with_host_status(|host_status| {
                let host_config = self
                    .host_config
                    .clone()
                    .unwrap_or_else(|| host_status.spec.clone());

                // Validate the loaded Host Configuration and rebuild RAID devices
                rebuild::validate_and_rebuild_raid(&host_config, host_status)
            })?
            .structured(ServicingError::ValidateAndRebuildRaid)?;

        Ok(())
    }

    fn handle_commands(
        &mut self,
        mut receiver: mpsc::Receiver<HostUpdateCommand>,
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

        // If host's servicing state is Finalized, need to validate that the firmware correctly
        // booted from the updated runtime OS image.
        if datastore.host_status().servicing_state == ServicingState::Finalized {
            validate_boot(datastore).message(
                "Failed to validate that firmware correctly booted from updated runtime OS image",
            )?
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
        cmd.host_config.validate().map_err(|e| {
            TridentError::new(InvalidInputError::InvalidHostConfigurationStatic { inner: e })
        })?;

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
                    engine::update(cmd, datastore).message("Failed update host")
                } else {
                    warn!("Host Configuration has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run");
                    Ok(())
                }
            } else {
                debug!("Host Configuration has not been updated");

                // If Host Configuration has not been updated and the previous A/B update failed,
                // ask the user to update HC and re-run.
                if datastore.host_status().servicing_state == ServicingState::AbUpdateFailed {
                    error!("A/B update previously failed with current Host Configuration. Update Host Configuration and re-run");
                    Err(TridentError::new(
                        InvalidInputError::RerunAbUpdateWithFailedHostConfiguration,
                    ))
                } else if datastore.host_status().servicing_state == ServicingState::Staged {
                    // If an update has been previously staged, only need to finalize the update.
                    debug!("There is an update staged on the host");
                    if cmd.allowed_operations.has_finalize() {
                        engine::finalize_update(
                            datastore,
                            None,
                            #[cfg(feature = "grpc-dangerous")]
                            &mut cmd.sender,
                        )
                        .message("Failed to finalize update")
                    } else {
                        debug!("Allowed operations do not include 'finalize'. Skipping finalizing of update");
                        Ok(())
                    }
                } else {
                    // Otherwise, if servicing state is Provisioned, need to inform the user that
                    // no new servicing has been requested. Servicing state cannot be
                    // NotProvisioned or Finalized here.
                    engine::update(cmd, datastore).message("Failed to update host")
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
                    engine::clean_install(cmd, datastore).message("Failed to run clean_install()")
                } else {
                    warn!("Host Configuration has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run");
                    Ok(())
                }
            } else {
                debug!("Host Configuration has not been updated");

                // If Host Configuration has not been updated and the previous clean install servicing has
                // failed, ask the user to update HC and re-run
                if datastore.host_status().servicing_state == ServicingState::CleanInstallFailed {
                    error!("Clean install previously failed with current Host Configuration. Update Host Configuration and re-run");
                    Err(TridentError::new(
                        InvalidInputError::RerunCleanInstallWithFailedHostConfiguration,
                    ))
                } else {
                    // Otherwise, if servicing state is 'Staged', i.e. a clean install has been
                    // staged, only need to finalize the clean install, if requested. No other
                    // servicing state is possible here.
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
                        debug!("Allowed operations do not include 'finalize'. Skipping finalizing of clean install");
                        Ok(())
                    }
                }
            }
        }
    }

    pub fn retrieve_host_status(
        datastore_path: &Path,
        output_path: &Option<PathBuf>,
        config_only: bool,
    ) -> Result<(), Error> {
        let host_status = DataStore::open(datastore_path)
            .unstructured("Failed to open datastore")?
            .host_status()
            .clone();

        let yaml = if config_only {
            serde_yaml::to_string(&host_status.spec)
                .context("Failed to serialize Host Configuration")?
        } else {
            serde_yaml::to_string(&host_status).context("Failed to serialize Host Status")?
        };

        match output_path {
            Some(path) => {
                info!("Writing Host Status to {:?}", &path);
                fs::write(path, yaml)
                    .context(format!("Failed to write Host Status to {:?}", path))?;
            }
            None => {
                println!("{yaml}");
            }
        }

        Ok(())
    }
}

/// Validates that the firmware correctly booted from the updated runtime OS image. If the firmware
/// did not boot from the expected root device, this function will return an error. In either case,
/// the function will update the Host Status.
#[tracing::instrument(skip_all)]
fn validate_boot(datastore: &mut DataStore) -> Result<(), TridentError> {
    info!("Validating whether host correctly booted from updated runtime OS image");

    // Create an EngineContext based on the Host Status
    let ctx = EngineContext {
        spec: datastore.host_status().spec.clone(),
        spec_old: datastore.host_status().spec_old.clone(),
        servicing_type: datastore.host_status().servicing_type,
        ab_active_volume: datastore.host_status().ab_active_volume,
        block_device_paths: datastore.host_status().block_device_paths.clone(),
        disks_by_uuid: datastore.host_status().disks_by_uuid.clone(),
        install_index: datastore.host_status().install_index,
        os_image: None, // Not used for boot validation logic
    };

    // Get the block device path of the current root
    let root_device_path =
        get_current_root_device_path(&ctx).message("Failed to get root block device path")?;

    // Get expected root device path
    let expected_root_device_path =
        get_expected_root_device_path(&ctx).message("Failed to get expected root device path")?;

    if compare_root_device_paths(root_device_path.clone(), expected_root_device_path.clone())
        .message("Host failed to boot from expected root device")?
    {
        info!("Host correctly booted from updated runtime OS image");

        // If it's QEMU, after confirming that we have booted into the
        // correct image, we need to update the `BootOrder` to boot from
        // the correct image next time.
        if osutils::virt::is_qemu() {
            bootentries::set_bootentries_after_reboot_for_qemu()
                .message("Failed to set boot entries after reboot")?;
        }
    } else if datastore.host_status().servicing_type == ServicingType::CleanInstall {
        datastore.with_host_status(|host_status| {
            host_status.servicing_type = ServicingType::NoActiveServicing;
            host_status.servicing_state = ServicingState::CleanInstallFailed;
        })?;

        return Err(TridentError::new(ServicingError::CleanInstallRebootCheck {
            root_device_path: root_device_path.to_string_lossy().to_string(),
            expected_device_path: expected_root_device_path.to_string_lossy().to_string(),
        }));
    } else {
        datastore.with_host_status(|host_status| {
            host_status.servicing_type = ServicingType::NoActiveServicing;
            host_status.servicing_state = ServicingState::AbUpdateFailed;
        })?;

        return Err(TridentError::new(ServicingError::AbUpdateRebootCheck {
            root_device_path: root_device_path.to_string_lossy().to_string(),
            expected_device_path: expected_root_device_path.to_string_lossy().to_string(),
        }));
    }

    match datastore.host_status().servicing_type {
        ServicingType::CleanInstall => {
            info!("Clean install of runtime OS succeeded");
            tracing::info!(metric_name = "clean_install_success", value = true);
        }
        ServicingType::AbUpdate => {
            info!("A/B update succeeded");
            tracing::info!(metric_name = "ab_update_success", value = true);
        }
        // Because the boot validation logic is currently called only on clean install and A/B
        // update, this should be unreachable.
        // TODO: When/If `UpdateAndReboot` is used, this should be updated.
        _ => unreachable!(),
    }

    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::Provisioned
    );

    datastore.with_host_status(|host_status| {
        host_status.servicing_state = ServicingState::Provisioned;
        host_status.servicing_type = ServicingType::NoActiveServicing;
        host_status.spec_old = Default::default();
        host_status.ab_active_volume = match host_status.ab_active_volume {
            None | Some(AbVolumeSelection::VolumeB) => Some(AbVolumeSelection::VolumeA),
            Some(AbVolumeSelection::VolumeA) => Some(AbVolumeSelection::VolumeB),
        };
    })?;

    Ok(())
}

/// Returns the current root device path, i.e. the device path that the host booted from.
fn get_current_root_device_path(ctx: &EngineContext) -> Result<PathBuf, TridentError> {
    // If the root is verity, fetch the block device path of the root data device path from the
    // 'veritysetup' output; otherwise, fetch the root device path from the host.
    let current_root_device_path = if ctx.spec.storage.root_is_verity() {
        // Get the block device ID of root
        let root_device_id = ctx
            .spec
            .storage
            .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
            .map(|m| &m.target_id)
            .structured(ServicingError::GetRootMountPointInfo {
                root_path: ROOT_MOUNT_POINT_PATH.to_string(),
            })?;
        debug!("Root device ID: {}", root_device_id);

        image::get_root_verity_data_device_path(ctx, root_device_id)
            .structured(ServicingError::GetRootVerityDataDevPath)?
    } else {
        // Fetch the root device path that the host booted from
        block_devices::get_root_device_path()?
    };

    debug!(
        "Current root device path: '{}'",
        current_root_device_path.display()
    );

    Ok(current_root_device_path)
}

/// Returns the path of the root device that the host was expected to boot from.
fn get_expected_root_device_path(ctx: &EngineContext) -> Result<PathBuf, TridentError> {
    // Get the block device ID of root
    let root_device_id = ctx
        .spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
        .map(|m| &m.target_id)
        .structured(ServicingError::GetRootMountPointInfo {
            root_path: ROOT_MOUNT_POINT_PATH.to_string(),
        })?;

    let expected_root_device_path = if ctx.spec.storage.root_is_verity() {
        // If root is on verity, fetch the block device path of the verity data device. Because
        // get_block_device_path(), which is called eventually, already has the logic for
        // determining the update volume, i.e. volume we expect to have booted from, getting the
        // block device path of the verity data device is sufficient.
        let root_verity_device_config = image::get_root_verity_device_config(ctx, root_device_id)
            .structured(ServicingError::GetRootVerityDeviceConfig)?;

        let (verity_data_path, _, _) =
            verity::get_verity_related_device_paths(ctx, &root_verity_device_config)
                .structured(ServicingError::GetRootVerityDataDevPath)?;

        verity_data_path
    } else {
        // Fetch the expected root device path
        engine::get_block_device_path(ctx, root_device_id).structured(
            ServicingError::GetBlockDevicePath {
                device_id: root_device_id.to_string(),
            },
        )?
    };

    debug!(
        "Expected root device path: '{}'",
        expected_root_device_path.display()
    );

    Ok(expected_root_device_path)
}

/// Compares the expected root device path with the current root device path that the host booted
/// from. Returns true if they match; false otherwise.
fn compare_root_device_paths(
    root_dev_path: PathBuf,
    expected_root_dev_path: PathBuf,
) -> Result<bool, TridentError> {
    // Canonicalize both paths
    let root_dev_path_canonicalized =
        root_dev_path
            .canonicalize()
            .structured(ServicingError::CanonicalizePath {
                path: root_dev_path.display().to_string(),
            })?;

    let expected_root_path_canonicalized =
        expected_root_dev_path
            .canonicalize()
            .structured(ServicingError::CanonicalizePath {
                path: expected_root_dev_path.display().to_string(),
            })?;

    info!(
        "Expected host to boot from block device with path '{}'",
        expected_root_path_canonicalized.display()
    );

    // If current root device path is NOT the same as the expected root device path, return false.
    if root_dev_path_canonicalized != expected_root_path_canonicalized {
        info!(
            "But host booted from an unexpected device with path '{}'",
            root_dev_path.display()
        );

        return Ok(false);
    }

    info!(
        "Host booted from the expected root device '{}'",
        root_dev_path.display()
    );

    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use maplit::btreemap;

    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystemType, Image, ImageFormat, ImageSha256,
            InternalMountPoint, InternalVerityDevice, MountOptions, MountPoint, Partition,
            PartitionType, VerityFileSystem,
        },
        constants::MOUNT_OPTION_READ_ONLY,
        error::ErrorKind,
        status::AbVolumeSelection,
    };

    #[test]
    fn test_get_expected_root_device_path() {
        let mut ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        // Add a disk and partitions
        ctx.spec.storage.disks.push(Disk {
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
        });

        // Add the required A/B update configuration
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        // Test case #0: If no mount points defined, should return an error.
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::GetRootMountPointInfo {
                root_path: ROOT_MOUNT_POINT_PATH.to_string()
            })
        );

        // Test case #1: If no root ID in block devices, should return an error.
        ctx.spec.storage.internal_mount_points = vec![InternalMountPoint {
            path: PathBuf::from("/"),
            target_id: "root".to_string(),
            filesystem: FileSystemType::Ext4,
            options: vec![],
        }];
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::GetBlockDevicePath {
                device_id: "root".to_string()
            })
        );

        // Test case #3: When block devices are defined, should return the expected root device
        // path of 'root-a'.
        ctx.block_device_paths = btreemap! {
            "os".to_owned() => PathBuf::from("/dev/sda"),
            "efi".to_owned() => PathBuf::from("/dev/sda1"),
            "root-a".to_owned() => PathBuf::from("/dev/sda2"),
            "root-b".to_owned() => PathBuf::from("/dev/sda3"),
        };
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda2")
        );

        // Test case #4: After rebooting after an A/B update, should return the expected root
        // device path of 'root-b'.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda3")
        );
    }

    /// Validates that get_expected_root_device_path() returns the expected root device path when
    /// root is a verity device.
    #[test]
    fn test_get_expected_root_device_path_verity() {
        let mut ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        // Add a disk and partitions
        ctx.spec.storage.disks.push(Disk {
            id: "os".to_owned(),
            device: PathBuf::from("/dev/disk/by-bus/foobar"),
            partitions: vec![
                Partition {
                    id: "esp".to_owned(),
                    size: 100.into(),
                    partition_type: PartitionType::Esp,
                },
                Partition {
                    id: "root-data-a".to_owned(),
                    size: 900.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root-data-b".to_owned(),
                    size: 9000.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root-hash-a".to_owned(),
                    size: 900.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root-hash-b".to_owned(),
                    size: 9000.into(),
                    partition_type: PartitionType::RootVerity,
                },
                Partition {
                    id: "root".to_owned(),
                    size: 9000.into(),
                    partition_type: PartitionType::RootVerity,
                },
            ],
            ..Default::default()
        });

        // Add the required A/B update configuration
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![
                AbVolumePair {
                    id: "root-data".to_string(),
                    volume_a_id: "root-data-a".to_string(),
                    volume_b_id: "root-data-b".to_string(),
                },
                AbVolumePair {
                    id: "root-hash".to_string(),
                    volume_a_id: "root-hash-a".to_string(),
                    volume_b_id: "root-hash-b".to_string(),
                },
                AbVolumePair {
                    id: "trident-overlay".to_string(),
                    volume_a_id: "trident-overlay-a".to_string(),
                    volume_b_id: "trident-overlay-b".to_string(),
                },
            ],
        });

        // Update the block device paths
        ctx.block_device_paths = btreemap! {
            "os".to_owned() => PathBuf::from("/dev/sda"),
            "efi".to_owned() => PathBuf::from("/dev/sda1"),
            "root-data-a".to_owned() => PathBuf::from("/dev/sda2"),
            "root-data-b".to_owned() => PathBuf::from("/dev/sda3"),
            "root-hash-a".to_owned() => PathBuf::from("/dev/sda4"),
            "root-hash-b".to_owned() => PathBuf::from("/dev/sda5"),
            "trident-overlay-a".to_owned() => PathBuf::from("/dev/sda6"),
            "trident-overlay-b".to_owned() => PathBuf::from("/dev/sda7"),
        };

        // Add internal mount points
        ctx.spec.storage.internal_mount_points = vec![
            InternalMountPoint {
                path: PathBuf::from("/"),
                target_id: "root".to_string(),
                filesystem: FileSystemType::Ext4,
                options: vec![],
            },
            InternalMountPoint {
                path: PathBuf::from("/var/lib/trident-overlay"),
                target_id: "trident-overlay".to_string(),
                filesystem: FileSystemType::Ext4,
                options: vec![],
            },
        ];

        // Add verity file systems
        ctx.spec.storage.verity_filesystems = vec![VerityFileSystem {
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            data_image: Image {
                url: "http://example.com/root-data.img".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://example.com/root-hash.img".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            fs_type: FileSystemType::Ext4,
            mount_point: MountPoint {
                path: ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
            },
        }];

        // Test case #0: If no internal verity devices defined, should return an error.
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::GetRootVerityDeviceConfig)
        );

        // Test case #1. Add an internal verity device configuration. Should now correctly return
        // the expected root device path of 'root-data-a', since servicing type is CleanInstall.
        ctx.spec.storage.internal_verity = vec![InternalVerityDevice {
            id: "root".into(),
            device_name: "root".into(),
            data_target_id: "root-data".into(),
            hash_target_id: "root-hash".into(),
        }];

        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda2")
        );

        // Test case #2. Change active volume to VolumeA and servicing type to AbUpdate, and
        // validate that the expected root device path is now the verity data device path of
        // 'root-data-b'.
        ctx.servicing_type = ServicingType::AbUpdate;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda3")
        );

        // Test case #3. Change active volume to VolumeB and validate that the expected root device
        // path is now the verity data device path of 'root-data-a'.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_expected_root_device_path(&ctx).unwrap(),
            PathBuf::from("/dev/sda2")
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    #[functional_test]
    fn test_compare_root_device_paths() {
        // Test case #0: If current root device path is the same as the expected root device path,
        // should return true.
        assert!(
            compare_root_device_paths(PathBuf::from("/dev/sda2"), PathBuf::from("/dev/sda2"))
                .unwrap()
        );

        // Test case #1: If current root device path is NOT the same as the expected root device
        // path, should return false.
        assert!(
            !compare_root_device_paths(PathBuf::from("/dev/sda2"), PathBuf::from("/dev/sda3"))
                .unwrap()
        );
    }
}
