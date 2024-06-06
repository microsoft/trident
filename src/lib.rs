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
use trident_api::constants::ROOT_MOUNT_POINT_PATH;
use trident_api::error::{
    ExecutionEnvironmentMisconfigurationError, InitializationError, InternalError,
    InvalidInputError, ManagementError, ReportError, TridentError, TridentResultExt,
};
use trident_api::status::{HostStatus, ServicingState, ServicingType};

use crate::datastore::DataStore;
use crate::modules::{bootentries, get_block_device, storage::tabfile};

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
            .structured(InvalidInputError::InvalidHostConfiguration {
                inner: InvalidHostConfigurationError::FailedToParse,
            })?
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
            InvalidInputError::InvalidHostConfiguration {
                inner: InvalidHostConfigurationError::FailedToParse,
            },
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
                    allowed_operations: self.config.allowed_operations.clone(),
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

    fn handle_commands(
        &mut self,
        mut receiver: mpsc::Receiver<HostUpdateCommand>,
        orchestrator: &Option<OrchestratorConnection>,
        datastore: &mut DataStore,
    ) -> Result<(), TridentError> {
        info!("Handling commands");
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

        // If host's servicing state is DeploymentFinalized, need to verify if firmware correctly
        // booted into the updated runtime OS image.
        if datastore.host_status().servicing_state == ServicingState::DeploymentFinalized {
            // Get device path of root mount point
            let root_dev_path = match tabfile::get_device_path(
                Path::new("/proc/mounts"),
                Path::new(ROOT_MOUNT_POINT_PATH),
            ) {
                Ok(path) => path,
                Err(e) => {
                    error!("Failed to get device path of root mount point: {e}");
                    return Err(TridentError::new(ManagementError::RootMountPointDevPath));
                }
            };
            info!("Validating whether firmware correctly booted into the updated runtime OS image");
            match validate_reboot(datastore.host_status(), root_dev_path) {
                Ok(_) => {
                    info!("Firmware correctly booted into the updated runtime OS image");
                }
                Err(e) => {
                    error!("Firmware performed a rollback into an incorrect OS image: {e}");
                    if datastore.host_status().servicing_type == Some(ServicingType::CleanInstall) {
                        datastore.with_host_status(|host_status| {
                            host_status.servicing_state = ServicingState::CleanInstallFailed;
                        })?;
                        return Err(TridentError::new(ManagementError::RollbackCleanInstall));
                    } else {
                        datastore.with_host_status(|host_status| {
                            host_status.servicing_state = ServicingState::AbUpdateFailed;
                        })?;
                        return Err(TridentError::new(ManagementError::RollbackAbUpdate));
                    }
                }
            }

            // Update boot order
            let datastore_path = match self.config.datastore {
                Some(ref path) => path,
                None => {
                    error!("Datastore path not set in local config");
                    return Err(TridentError::new(
                        InternalError::GetDatastorePathFromLocalTridentConfig,
                    ));
                }
            };
            info!("Setting boot order");
            bootentries::set_boot_order(datastore_path)?;

            if datastore.host_status().servicing_type == Some(ServicingType::CleanInstall) {
                info!(
                    "Clean install of runtime OS succeeded. Setting servicing state to Provisioned"
                );
                tracing::info!(metric_name = "clean_install_success", value = true);
            } else {
                info!("A/B update succeeded. Setting servicing state to Provisioned");
            }
            datastore.with_host_status(|host_status| {
                host_status.servicing_state = ServicingState::Provisioned;
                host_status.servicing_type = None;
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
            info!("Injecting phonehome into host configuration");
            cmd.host_config.trident.phonehome = self.config.phonehome.clone();
        }

        cmd.host_config.validate().map_err(|e| {
            TridentError::new(InvalidInputError::InvalidHostConfiguration { inner: e })
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

        debug!("Allowed operations: {:?}", cmd.allowed_operations);
        // If Trident loads from a persistent datastore, firmware is booted from runtime OS
        if datastore.is_persistent() {
            // If HS.spec in the datastore is different from the new HC, need to both stage and
            // finalize the update, regardless of state
            if datastore.host_status().spec != cmd.host_config {
                debug!("Host config has been updated");
                // If allowed operations include 'stage', start update
                if cmd.allowed_operations.has_stage() {
                    modules::update(cmd, datastore).message("Failed to run update()")
                } else {
                    warn!("Host config has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run");
                    Ok(())
                }
            } else {
                debug!("Host config has not been updated");

                // If host config has not been updated and a rollback occurred with this HC
                // before, ask the user to update HC and re-run
                if datastore.host_status().servicing_state == ServicingState::AbUpdateFailed {
                    error!("Rollback occurred when Trident attempted A/B update with current host config. Update host config and re-run");
                    return Err(TridentError::new(ManagementError::RollbackAbUpdate));
                }

                // If an update has been previously staged, only need to finalize the update
                if datastore.host_status().servicing_state == ServicingState::DeploymentStaged {
                    debug!("There is an update staged on the host");
                    if cmd.allowed_operations.has_finalize() {
                        modules::finalize_update(
                            datastore,
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
                    // state is StagingDeployment OR FinalizingDeployment, need to re-do update.
                    //
                    // State cannot be NotProvisioned or DeploymentFinalized here; DeploymentStaged
                    // and AbUpdateFailed were addressed above
                    modules::update(cmd, datastore).message("Failed to run update()")
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
                    modules::clean_install(cmd, datastore).message("Failed to run clean_install()")
                } else {
                    warn!("Host config has been updated but allowed operations do not include 'stage'. Add 'stage' and re-run");
                    Ok(())
                }
            } else {
                debug!("Host config has not been updated");

                // If host config has not been updated but a rollback occurred before, return
                if datastore.host_status().servicing_state == ServicingState::CleanInstallFailed {
                    error!("Rollback occurred when Trident attempted clean install with current host config. Update host config and re-run");
                    return Err(TridentError::new(ManagementError::RollbackCleanInstall));
                }

                // If HS.spec matches the new HS, only need to finalize the clean install
                if datastore.host_status().servicing_state == ServicingState::DeploymentStaged {
                    debug!("There is a clean install staged on the host");
                    if cmd.allowed_operations.has_finalize() {
                        // Remount new root and custom mounts if we're finalizing a clean install
                        let (new_root_path, _, _) =
                            modules::initialize_new_root(datastore, &cmd.host_config)
                                .message("Failed to remount new root")?;

                        modules::finalize_clean_install(
                            datastore,
                            &new_root_path,
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
                    // If servicing state is StagingDeployment OR FinalizingDeployment, need to
                    // re-do update.
                    //
                    // State cannot be NotProvisioned, Provisioned, AbUpdateFailed, or
                    // DeploymentFinalized here; DeploymentStaged and CleanInstallFailed were
                    // addressed above.
                    modules::clean_install(cmd, datastore).message("Failed to run clean_install()")
                }
            }
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

/// Validates that the host correctly booted from the updated image after finalizing CleanInstall
/// or A/B update. Otherwise, if a rollback occurred, returns an error.
#[tracing::instrument(skip_all)]
fn validate_reboot(host_status: &HostStatus, root_dev_path: PathBuf) -> Result<(), Error> {
    // Fetch mount point for root from host status and fetch target_id of root device
    let root_target_id = match host_status
        .spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
    {
        Some(mp) => &mp.target_id,
        None => {
            bail!(
                "Failed to get mount point for root '{}'. Disk intended for reboot no longer exists",
                ROOT_MOUNT_POINT_PATH
            );
        }
    };
    debug!(
        "Firmware booted from block device with target_id {:?}",
        root_target_id
    );

    // Fetch expected_root_dev_path. active=false b/c need to fetch info for volume that we expect
    // to be active at this point, after firmware has already rebooted, and it used to be the
    // update volume before the reboot.
    let expected_root_path = match get_block_device(host_status, root_target_id, false) {
        Some(block_device_info) => block_device_info.path,
        None => {
            bail!(
                "Failed to get block device info for root '{}'",
                root_target_id
            );
        }
    };
    debug!(
        "Non-canonicalized root device path: {}",
        expected_root_path.display()
    );
    let expected_root_path_canonicalized = match expected_root_path.canonicalize() {
        Ok(path) => path,
        Err(e) => {
            bail!(
                "Failed to get canonical path for expected root '{}': {e}",
                expected_root_path.display()
            );
        }
    };

    // If current root device path is NOT the same as the expected root device path, firmware did
    // not boot into the updated runtime OS image, but instead, performed a rollback into prov OS
    // or old runtime OS.
    // NOTE: For verity, path like /dev/mapper/root, which is what we use in host status, gets
    // resolved to /dev/dm-0. So, compare if matches either.
    if root_dev_path != expected_root_path && root_dev_path != expected_root_path_canonicalized {
        debug!(
            "Expected root device path: {:?}. In canonicalized format: {:?}",
            expected_root_path, expected_root_path_canonicalized
        );
        debug!("Current root device path: {:?}", root_dev_path);
        // If root_device_path is None, Trident tried to perform reboot as part of CleanInstall
        if host_status.servicing_type == Some(ServicingType::CleanInstall) {
            bail!("Reboot validation failed: Firmware rolled back into provisioning OS");
        } else {
            bail!(
                "Reboot validation failed: Firmware rolled back into '{}'",
                root_dev_path.display()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use trident_api::config::{FileSystemType, InternalMountPoint, Storage};

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
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use maplit::btreemap;
    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystemType, InternalMountPoint, Partition,
            PartitionSize, PartitionType, Storage,
        },
        status::{AbVolumeSelection, BlockDeviceContents, BlockDeviceInfo, Storage as HostStorage},
    };

    /// Validates that validate_reboot() correctly detects rollback when root is a partition.
    #[functional_test]
    fn test_validate_reboot() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "esp".to_owned(),
                                size: PartitionSize::Fixed(100),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root-a".to_owned(),
                                size: PartitionSize::Fixed(900),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "root-b".to_owned(),
                                size: PartitionSize::Fixed(9000),
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
            storage: HostStorage {
                block_devices: [].into(),
                ..Default::default()
            },
            servicing_state: ServicingState::DeploymentFinalized,
            servicing_type: Some(ServicingType::CleanInstall),
            ..Default::default()
        };

        // Test case #0: If no mount points defined, should return an error.
        let result0 = validate_reboot(&host_status, PathBuf::from("/dev/sda2"));
        let error_message0 = result0.unwrap_err().root_cause().to_string();
        assert_eq!(
            error_message0,
            "Failed to get mount point for root '/'. Disk intended for reboot no longer exists"
        );

        // Test case #1: If no root target id in block devices, should return an error.
        host_status.spec.storage.internal_mount_points = vec![InternalMountPoint {
            path: PathBuf::from("/"),
            target_id: "root".to_string(),
            filesystem: FileSystemType::Ext4,
            options: vec![],
        }];
        let result1 = validate_reboot(&host_status, PathBuf::from("/dev/sda2"));
        let error_message1 = result1.unwrap_err().root_cause().to_string();
        assert_eq!(
            error_message1,
            "Failed to get block device info for root 'root'"
        );

        // Test case #2: After CleanInstall, Trident correctly booted into root-a.
        host_status.storage.block_devices = btreemap! {
            "os".to_owned() => BlockDeviceInfo {
                path: PathBuf::from("/dev/sda"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            },
            "efi".to_owned() => BlockDeviceInfo {
                path: PathBuf::from("/dev/sda1"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            },
            "root-a".to_owned() => BlockDeviceInfo {
                path: PathBuf::from("/dev/sda2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            },
            "root-b".to_owned() => BlockDeviceInfo {
                path: PathBuf::from("/dev/sda3"),
                size: 9000,
                contents: BlockDeviceContents::Unknown,
            },
        };
        let result2 = validate_reboot(&host_status, PathBuf::from("/dev/sda2"));
        assert!(result2.is_ok());

        // Test case #3: After CleanInstall, Trident performed a rollback into prov OS.
        let result3 = validate_reboot(&host_status, PathBuf::from("/provOS/path"));
        let error_message3 = result3.unwrap_err().root_cause().to_string();
        assert_eq!(
            error_message3,
            "Reboot validation failed: Firmware rolled back into provisioning OS"
        );

        // Test case #4: After A/B update from A to B, Trident correctly booted into root-b.
        host_status.servicing_type = Some(ServicingType::AbUpdate);
        host_status.servicing_state = ServicingState::DeploymentFinalized;
        // Update root_device_path to /dev/sda2 and active volume to VolumeA
        host_status.storage.root_device_path = Some(PathBuf::from("/dev/sda2"));
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        let result4 = validate_reboot(&host_status, PathBuf::from("/dev/sda3"));
        assert!(result4.is_ok());

        // Test case #5: After A/B update from A to B, Trident performed a rollback into root-a.
        let result5 = validate_reboot(&host_status, PathBuf::from("/dev/sda2"));
        let error_message5 = result5.unwrap_err().root_cause().to_string();
        assert_eq!(
            error_message5,
            "Reboot validation failed: Firmware rolled back into '/dev/sda2'"
        );

        // Test case #6: After A/B update from B to A, Trident correctly booted into root-a.
        // Update root_device_path to /dev/sda3 and active volume to VolumeB
        host_status.storage.root_device_path = Some(PathBuf::from("/dev/sda3"));
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        let result6 = validate_reboot(&host_status, PathBuf::from("/dev/sda2"));
        assert!(result6.is_ok());

        // Test case #7: After A/B update from B to A, Trident performed a rollback into root-b.
        let result7 = validate_reboot(&host_status, PathBuf::from("/dev/sda3"));
        let error_message7 = result7.unwrap_err().root_cause().to_string();
        assert_eq!(
            error_message7,
            "Reboot validation failed: Firmware rolled back into '/dev/sda3'"
        );
    }
}
