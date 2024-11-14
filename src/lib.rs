use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use engine::storage::rebuild;
use log::{debug, error, info, warn};
use nix::unistd::Uid;
use tokio::sync::mpsc::{self};

use osutils::container;
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

/// Trident binary path.
const TRIDENT_BINARY_PATH: &str = "/usr/bin/trident";

/// OS Modifier (EMU) binary path.
const OS_MODIFIER_BINARY_PATH: &str = "/usr/bin/osmodifier";

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
            serde_yaml::to_string(&host_config).unwrap_or("Failed to serialize host config".into())
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
            "Host config:\n{}",
            serde_yaml::to_string(&host_config).unwrap_or("Failed to serialize host config".into())
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

        // If we have a host config source, load it and dispatch it as the first
        // command.
        if let Some(host_config) = self.host_config.clone() {
            debug!("Applying host configuration from local config");
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

                // Validate the loaded host config and rebuild RAID devices
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
                tabfile::get_device_path(Path::new(PROC_MOUNTINFO_PATH), &root_mount_path)
                    .structured(ServicingError::RootMountPointDevPath {
                        mountinfo_file: PROC_MOUNTINFO_PATH.to_string(),
                    })?;

            // Get expected device path of root mount point
            let expected_root_dev_path = get_expected_root_device_path(datastore.host_status())?;

            info!("Validating whether host correctly booted into the updated runtime OS image");
            if validate_reboot(root_dev_path.clone(), expected_root_dev_path.clone())
                .message("Host failed to boot from the expected root device")?
            {
                info!("Host correctly booted into the updated runtime OS image");

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
                info!("Clean install of runtime OS succeeded");
                debug!("Updating host's servicing state to Provisioned");
                tracing::info!(metric_name = "clean_install_success", value = true);
            } else {
                info!("A/B update succeeded");
                debug!("Updating host's servicing state to Provisioned");
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
                    Err(TridentError::new(
                        InvalidInputError::RerunAbUpdateWithFailedHostConfiguration,
                    ))
                } else if datastore.host_status().servicing_state == ServicingState::Staged {
                    // If an update has been previously staged, only need to finalize the update
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

                // If host config has not been updated and the previous clean install servicing has
                // failed, ask the user to update HC and re-run
                if datastore.host_status().servicing_state == ServicingState::CleanInstallFailed {
                    error!("Previous clean install attempt failed with current host config. Update host config and re-run");
                    Err(TridentError::new(
                        InvalidInputError::RerunCleanInstallWithFailedHostConfiguration,
                    ))
                } else {
                    // Otherwise, if servicing state is 'Staged', i.e. a clean install has been
                    // staged, only need to finalize the clean install, if requested. No other
                    // servicing state is possible here.
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
