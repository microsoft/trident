use std::fs;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Error};
use log::{debug, error, info, warn};
use nix::unistd::Uid;
use osutils::exe::RunAndCheck;
use protobufs::*;
use sys_mount::{MountFlags, UnmountFlags};
use tokio::sync::mpsc::{self, Sender, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};

use osutils::overlay::SystemDFilesystemOverlay;
use osutils::{chroot, container};
use setsail::KsTranslator;
use trident_api::config::{HostConfiguration, LocalConfigFile, Operations};
use trident_api::config::{HostConfigurationSource, InvalidHostConfigurationError};
use trident_api::error::{
    ExecutionEnvironmentMisconfigurationError, InitializationError, InternalError,
    InvalidInputError, ManagementError, ReportError, TridentError, TridentResultExt,
};

use crate::datastore::DataStore;
use crate::modules::bootentries;

mod datastore;
mod logstream;
mod modules;
mod multilog;
mod orchestrate;

pub use logstream::Logstream;
pub use modules::network::provisioning::start as start_provisioning_network;
pub use multilog::MultiLogger;
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

/// Stores the block device and relative path to the runtime Trident datastore for use by the
/// provisioning OS.
pub const TRIDENT_DATASTORE_REF_PATH: &str = "/var/lib/trident/datastore-location.conf";

/// Trident binary path.
pub const TRIDENT_BINARY_PATH: &str = "/usr/bin/trident";
pub const OS_MODIFIER_BINARY_PATH: &str = "/usr/bin/osmodifier";

/// Systemd unit root path.
const SYSTEMD_UNIT_ROOT_PATH: &str = "/etc/systemd/system";

mod protobufs {
    tonic::include_proto!("trident");
}

/// Implementation of the gRPC service.
///
/// This struct contains a tokio Sender which it uses to enqueue commands to the main trident
/// thread. It also implements the gRPC service trait, which allows it to be used as a gRPC server.
pub struct HostManagementImpl(Sender<HostUpdateCommand>);

#[tonic::async_trait]
impl host_management_server::HostManagement for HostManagementImpl {
    type UpdateHostStream = UnboundedReceiverStream<Result<HostStatusState, Status>>;

    async fn update_host(
        &self,
        request: Request<HostUpdateRequest>,
    ) -> Result<Response<Self::UpdateHostStream>, Status> {
        info!("Received update_host request");
        let request = request.into_inner();

        let host_config = serde_yaml::from_str(&request.host_configuration)
            .context("Failed to parse host config")
            .map_err(|e| Status::invalid_argument(format!("{e:?}")))?;

        let (tx, rx) = mpsc::unbounded_channel();
        self.0
            .send(HostUpdateCommand {
                allowed_operations: Operations::all(), // TODO
                host_config,
                sender: Some(tx),
            })
            .await
            .context("Failed to enqueue provision command")
            .map_err(|e| Status::from_error(e.into()))?;

        Ok(Response::new(UnboundedReceiverStream::new(rx)))
    }
}

/// A command to update the host configuration.
///
/// This struct is used to communicate between the gRPC server and the main trident thread. It
/// includes information on the command to execute, as well as a tokio Sender which the main thread
/// can use to stream status updates back to the gRPC client.
struct HostUpdateCommand {
    allowed_operations: Operations,
    host_config: HostConfiguration,
    sender: Option<UnboundedSender<Result<HostStatusState, Status>>>,
}

pub struct Trident {
    config: LocalConfigFile,
    server_runtime: Option<tokio::runtime::Runtime>,
}
impl Trident {
    pub fn new(config_path: Option<PathBuf>, logstream: Logstream) -> Result<Self, TridentError> {
        let config_path = if let Some(path) = config_path {
            path.to_owned()
        } else if Path::new("/host").join(TRIDENT_LOCAL_CONFIG_PATH).exists() {
            Path::new("/host").join(TRIDENT_LOCAL_CONFIG_PATH)
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
                        o.report_error(format!("{e:?}"))
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
                    sender: None,
                })
                .structured(InternalError::Internal(
                    "Failed to enqueue provision command",
                ))?;
        }

        // If gRPC connection details were provided, start the gRPC server to accept commands.
        if let Some(ref grpc) = self.config.grpc {
            // TODO: make firewall this configurable
            info!("Opening firewall");
            let _ = open_firewall_for_grpc().structured(ManagementError::OpenFirewall);

            let addr = "0.0.0.0".parse().unwrap();
            let port = grpc.listen_port.unwrap_or(50051);

            info!("Preparing to listen for gRPC requests");

            let rt = tokio::runtime::Runtime::new()
                .structured(InternalError::Internal("Failed to start tokio runtime"))?;
            rt.spawn(async move {
                Server::builder()
                    .add_service(host_management_server::HostManagementServer::new(
                        HostManagementImpl(sender),
                    ))
                    .serve(SocketAddr::new(addr, port))
                    .await
                    .context("Failed while serving gRPC requests")
            });
            self.server_runtime = Some(rt);

            // Notify orchestrator that we are ready to receive commands.
            if let Some(ref orchestrator) = orchestrator {
                orchestrator.report_success()
            }
        } else {
            // If no gRPC connection details were provided, drop the sender side of the channel.
            // This causes the loop below will exit immediately after processing the initial command
            // that was enqueued above.
            drop(sender);
        }

        // When running inside a container, we want to chroot into the host's
        // root. To do this, we assume the container is created with a volume/bind
        // mount of the host's root at /host. We enter this chroot here so that
        // all subsequent commands are executed in the host's root, and dont
        // have to be aware of if Trident is running in the context of the
        // container or not.
        if container::is_running_in_container().message("Failed running in container check")? {
            debug!("Copying os modifier binary from container to host");

            let host_root_path_buf =
                container::get_host_root_path().message("Failed to get host root volume path")?;

            // Copy EMU binary from container to host
            fs::copy(
                OS_MODIFIER_BINARY_PATH,
                host_root_path_buf.join(OS_MODIFIER_BINARY_PATH.trim_start_matches('/')),
            )
            .structured(ManagementError::OSModifierCopy)?;

            info!("Running inside container, entering '/host' chroot");
            if let Err(e) = chroot::enter_host_chroot(
                &host_root_path_buf,
            )
            .message(
                "Failed to enter host chroot, which is required when executing inside a container",
            )?
            .execute_and_exit(|| self.handle_commands(receiver, &orchestrator))
            .message("Failed to execute in the host chroot")
            {
                if let Some(ref orchestrator) = orchestrator {
                    orchestrator.report_error(format!("{e:?}"));
                }
                return Err(e);
            }
        } else {
            self.handle_commands(receiver, &orchestrator)?;
        };

        if let Some(ref orchestrator) = orchestrator {
            orchestrator.report_success()
        }

        Ok(())
    }

    fn handle_commands(
        &mut self,
        mut receiver: mpsc::Receiver<HostUpdateCommand>,
        orchestrator: &Option<OrchestratorConnection>,
    ) -> Result<(), TridentError> {
        info!("Handling commands");
        let mut datastore = match self.config.datastore {
            Some(ref datastore_path) => {
                // Load an existing datastore
                DataStore::open(datastore_path.as_path()).structured(
                    InitializationError::DatastoreLoad {
                        path: datastore_path.display().to_string(),
                    },
                )?
            }
            None => DataStore::open_temporary().message("Failed to open temporary datastore")?,
        };

        // Process commands. Starting with the initial command indicated in the local config file
        // (if any). Once that has been handled, subsequent commands are received from the gRPC
        // endpoint.
        while let Some(cmd) = receiver.blocking_recv() {
            let has_sender = cmd.sender.is_some();

            if let Err(e) = self.handle_command(&mut datastore, cmd) {
                if let Some(ref orchestrator) = *orchestrator {
                    orchestrator.report_error(format!("{e:?}"));
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
            bootentries::set_boot_order(datastore_path.as_path())?;
        }

        Ok(())
    }

    fn handle_command(
        &mut self,
        datastore: &mut DataStore,
        mut cmd: HostUpdateCommand,
    ) -> Result<(), TridentError> {
        if self.config.phonehome.is_some() && cmd.host_config.management.phonehome.is_none() {
            info!("Injecting phonehome into host configuration");
            cmd.host_config.management.phonehome = self.config.phonehome.clone();
        }

        cmd.host_config
            .validate()
            .map_err(|e| TridentError::new(InvalidInputError::InvalidHostConfiguration(e)))?;

        // Use overlay for holding any changes to the host filesystem that
        // should not be persisted.
        // TODO: mount the overlay only if we actually need to perform an update
        let overlay =
            SystemDFilesystemOverlay::mount_temporary(Path::new(SYSTEMD_UNIT_ROOT_PATH), &[])
                .structured(ManagementError::MountOverlay)?;

        let result = if datastore.is_persistent() {
            modules::update(cmd, datastore).message("Failed to update host")
        } else {
            modules::provision_host(cmd, datastore).message("Failed to provision host")
        };

        match overlay
            .unmount()
            .structured(ManagementError::UnmountOverlay)
        {
            Ok(_) => result,
            Err(e2) => match result {
                Ok(_) => Err(e2),
                Err(e) => Err(e.secondary_error_context(e2)),
            },
        }
    }

    pub fn retrieve_host_status(&mut self, output_path: &Option<PathBuf>) -> Result<(), Error> {
        let host_status = if let Some(ref datastore_path) = self.config.datastore {
            info!("Opening persistent datastore");
            DataStore::open(datastore_path.as_path())?
                .host_status()
                .clone()
        } else if Path::new(TRIDENT_DATASTORE_REF_PATH).exists() {
            info!("Retrieving host status from runtime datastore");
            let datastore_ref = fs::read_to_string(TRIDENT_DATASTORE_REF_PATH)
                .context("Failed to read datastore reference")?;

            if datastore_ref.is_empty() {
                bail!("Datastore reference is empty. This is a trident issue and will be fixed in a future release");
            }

            let (block_device, relative_path) = datastore_ref
                .split_once('\n')
                .context("Failed to parse datastore reference")?;

            let mount_point =
                tempfile::tempdir_in("/mnt").context("Failed to create temporary mount point")?;
            let _mount = sys_mount::Mount::builder()
                .flags(MountFlags::RDONLY)
                .mount_autodrop(block_device, mount_point.path(), UnmountFlags::DETACH);

            let datastore_path = mount_point.path().join(relative_path);
            DataStore::open(datastore_path.as_path())?
                .host_status()
                .clone()
        } else if Path::new(TRIDENT_TEMPORARY_DATASTORE_PATH).exists() {
            info!("Opening temporary datastore");
            DataStore::open(Path::new(TRIDENT_TEMPORARY_DATASTORE_PATH))
                .context("Failed to open temporary datastore")?
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

fn open_firewall_for_grpc() -> Result<(), Error> {
    Command::new("iptables")
        .arg("-A")
        .arg("INPUT")
        .arg("-p")
        .arg("tcp")
        .arg("--dport")
        .arg("50051") // TODO
        .arg("-j")
        .arg("ACCEPT")
        .run_and_check()
        .context("Failed to open firewall for gRPC")
}

#[cfg(test)]
mod tests {
    use trident_api::{
        config::{MountPoint, PartitionType, Storage},
        constants,
        status::{BlockDeviceContents, BlockDeviceInfo, Disk, Partition},
    };

    use super::*;
    use std::path::PathBuf;

    /// Validates that the `to_block_device` function works as expected for
    /// disks and partitions.
    #[test]
    fn test_to_block_device() {
        let mut disk = Disk {
            path: PathBuf::from("/dev/disk/by-bus/foobar"),
            uuid: uuid::Uuid::nil(),
            capacity: 0,
            contents: BlockDeviceContents::Unknown,
            partitions: vec![],
        };

        assert_eq!(
            &disk.to_block_device(),
            &BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );

        disk.capacity = 1234567890;

        assert_eq!(
            &disk.to_block_device(),
            &BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 1234567890,
                contents: BlockDeviceContents::Unknown,
            }
        );

        let mut partition = Partition {
            id: "efi".to_owned(),
            path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
            contents: BlockDeviceContents::Unknown,
            start: 0,
            end: 0,
            ty: PartitionType::Esp,
            uuid: uuid::Uuid::nil(),
        };

        assert_eq!(
            &partition.to_block_device(),
            &BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );

        partition.start = 123;
        partition.end = 456;
        assert_eq!(
            &partition.to_block_device(),
            &BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                size: 333,
                contents: BlockDeviceContents::Unknown,
            }
        );
    }

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
                    filesystem: "ext4".to_string(),
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
