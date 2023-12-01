use anyhow::{bail, Context, Error};
use datastore::DataStore;
use log::{debug, error, info, warn};
use osutils::overlay::EphemeralOverlayWithSystemD;
use osutils::{chroot, container};
use protobufs::*;
use setsail::KsTranslator;
use std::fs;
use std::net::SocketAddr;
use tokio::sync::mpsc::{self, Sender, UnboundedSender};
use trident_api::config::HostConfigurationSource;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use trident_api::config::{DatastoreConfiguration, HostConfiguration, LocalConfigFile, Operations};

mod datastore;
mod logstream;
mod modules;
mod multilog;
mod orchestrate;

pub use modules::network::provisioning::start as start_provisioning_network;

pub use logstream::Logstream;
pub use multilog::MultiLogger;
pub use orchestrate::OrchestratorConnection;

/// Default Trident configuration file path.
pub const TRIDENT_LOCAL_CONFIG_PATH: &str = "/etc/trident/config.yaml";

/// Path to a generated Trident configuration file. This is useful when
/// running inside an ephemeral provisioning OS, as the original configuration
/// file does not point to a Trident datastore, that contains information about
/// HostStatus. The regenerated configuration file will point to the Trident
/// datastore that has been generated as part of the initial provisioning process.
pub const TRIDENT_GENERATED_CONFIG_PATH: &str = "/var/run/trident/config.yaml";

/// Default Trident datastore path. Used from the runtime OS.
pub const TRIDENT_DATASTORE_PATH: &str = "/var/lib/trident/datastore.sqlite";

/// Trident binary path.
pub const TRIDENT_BINARY_PATH: &str = "/usr/bin/trident";

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
    datastore: DataStore,
    server_runtime: Option<tokio::runtime::Runtime>,
}
impl Trident {
    pub fn new(config_path: Option<PathBuf>, logstream: Logstream) -> Result<Self, Error> {
        let config_path = match config_path {
            Some(path) => path,
            None => {
                if Path::new(TRIDENT_GENERATED_CONFIG_PATH).exists() {
                    info!("Using generated config file");
                    PathBuf::from(TRIDENT_GENERATED_CONFIG_PATH)
                } else {
                    info!("Using default config file");
                    PathBuf::from(TRIDENT_LOCAL_CONFIG_PATH)
                }
            }
        };
        // Load the config file
        info!("Loading config from '{}'", config_path.display());
        let config_contents = fs::read_to_string(config_path)
            .map_err(|e| warn!("Failed to read config file: {e}"))
            .unwrap_or_default();

        // Parse the config file
        let config: LocalConfigFile = match serde_yaml::from_str(&config_contents)
            .context("Failed to parse Trident configuration")
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
                .context("Failed to set logstream URL")?;
        }

        debug!(
            "Trident config:\n{}",
            serde_yaml::to_string(&config).unwrap_or("Failed to serialize host config".into())
        );

        let datastore = match config.datastore {
            Some(DatastoreConfiguration::Load { ref load_path }) => {
                DataStore::open(load_path).context("Failed to load datastore")?
            }
            _ => DataStore::new(),
        };

        Ok(Self {
            config,
            datastore,
            server_runtime: None,
        })
    }

    fn load_host_config(source: &HostConfigurationSource) -> Result<Box<HostConfiguration>, Error> {
        let host_config = match source {
            HostConfigurationSource::File(path) => {
                info!("Loading host config from '{}'", path.display());

                serde_yaml::from_str(
                    &fs::read_to_string(path).context("Failed to read host config file")?,
                )
                .context("Failed to parse host config file")?
            }
            HostConfigurationSource::Embedded(contents) => contents.clone(),
            HostConfigurationSource::KickstartEmbedded(contents) => {
                match KsTranslator::new()
                    .run_pre_scripts(true)
                    .translate(setsail::load_kickstart_string(contents))
                {
                    Ok(hc) => Box::new(hc),
                    Err(e) => {
                        // TODO: handle & report kickstart errors
                        bail!(
                            "Failed to translate kickstart:\n{}",
                            serde_json::to_string_pretty(&e)?
                        );
                    }
                }
            }
            HostConfigurationSource::KickstartFile(file) => {
                match KsTranslator::new().run_pre_scripts(true).translate(
                    setsail::load_kickstart_file(
                        file.to_str()
                            .context(format!("Failed to resolve path {}", file.display()))?,
                    )?,
                ) {
                    Ok(hc) => Box::new(hc),
                    Err(e) => {
                        bail!(
                            // TODO: handle & report kickstart errors
                            "Failed to translate kickstart:\n{}",
                            serde_json::to_string_pretty(&e)?
                        );
                    }
                }
            }
        };

        debug!(
            "Host config:\n{}",
            serde_yaml::to_string(&host_config).unwrap_or("Failed to serialize host config".into())
        );

        Ok(host_config)
    }

    pub fn start_network(&mut self) -> Result<(), Error> {
        // If we have kickstart it means we don't have networking config readily available. We
        // _could_ try parsing now, but we are in an early stage of boot and we want to parse on a
        // later stage so %pre scripts can run and do their thing. It would also mean parsing twice,
        // unless we updated the config file in place. That sounds like a can of worms and we still
        // have the issue about being too early.
        if let Some(
            HostConfigurationSource::KickstartFile(_)
            | HostConfigurationSource::KickstartEmbedded(_),
        ) = self.config.get_host_configuration_source()?
        {
            warn!("Cannot set up network early when using kickstart");
            return Ok(());
        }

        let host_config = self
            .config
            .get_host_configuration_source()?
            .as_ref()
            .map(Self::load_host_config)
            .transpose()?;

        info!("Starting network");
        start_provisioning_network(self.config.network_override.clone(), host_config.as_deref())
            .context("Failed to start provisioning network")?;

        Ok(())
    }

    pub fn run(&mut self) -> Result<(), Error> {
        let orchestrator = self
            .config
            .phonehome
            .as_ref()
            .and_then(|url| OrchestratorConnection::new(url.clone()));

        // This creates a channel to send commands to the main trident thread. It lets us use the
        // same logic for processing an initial provision command contained within the trident local
        // config as for processing commands received from the gRPC endpoint.
        let (sender, mut receiver) = tokio::sync::mpsc::channel(1);

        // If we have a host config source, load it and dispatch it as the first command.
        if let Some(ref host_config_source) = self.config.get_host_configuration_source()? {
            let host_config = Self::load_host_config(host_config_source)?;

            info!("Running");
            sender
                .blocking_send(HostUpdateCommand {
                    allowed_operations: self.config.allowed_operations,
                    host_config: *host_config,
                    sender: None,
                })
                .context("Failed to enqueue provision command")?;
        }

        // If gRPC connection details were provided, start the gRPC server to accept commands.
        if let Some(ref grpc) = self.config.grpc {
            // TODO: make firewall this configurable
            info!("Opening firewall");
            let _ = open_firewall_for_grpc().context("Failed to open firewall for gRPC");

            let addr = "0.0.0.0".parse().unwrap();
            let port = grpc.listen_port.unwrap_or(50051);

            info!("Preparing to listen for gRPC requests");

            let rt = tokio::runtime::Runtime::new().context("Failed to start tokio runtime")?;
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
        let chroot = if container::is_running_in_container() {
            Some(chroot::enter_host_chroot(
                container::get_host_root_path().context("Failed to get host root mount path which is required when executing inside a container")?.as_path(),
            ).context("Failed to enter host chroot, which is required when executing inside a container")?)
        } else {
            None
        };

        // Process commands. Starting with the initial command indicated in the local config file
        // (if any). Once that has been handled, subsequent commands are received from the gRPC
        // endpoint.
        while let Some(mut cmd) = receiver.blocking_recv() {
            if self.config.phonehome.is_some() && cmd.host_config.management.phonehome.is_none() {
                info!("Injecting phonehome into host configuration");
                cmd.host_config.management.phonehome = self.config.phonehome.clone();
            }

            // TODO: mount the overlay only if we actually need to perform an update
            let overlay = EphemeralOverlayWithSystemD::mount(Path::new(SYSTEMD_UNIT_ROOT_PATH));
            if let Err(e) = &overlay {
                // we can continue, though if we need to rerun, there will be
                // subsequent errors
                error!("Failed to setup systemd configuration overlay: {e:?}");
            }

            let result = if self.datastore.is_persistent() {
                modules::update(cmd, &mut self.datastore).context("Failed to update host")
            } else {
                modules::provision(cmd, &mut self.datastore).context("Failed to provision host")
            };

            if let Err(e) = result {
                if let Some(ref orchestrator) = orchestrator {
                    orchestrator.report_error(format!("{e:?}"));
                }
                error!("{e:?}");
            }

            if let Ok(overlay) = overlay {
                if let Err(e) = overlay.unmount().context("Failed to exit overlay") {
                    error!("{e:?}");
                }
            }
        }

        // Exit the chroot if we were executing in the container.
        if let Some(chroot) = chroot {
            if let Err(e) = chroot.exit().context("Failed to exit chroot") {
                if let Some(ref orchestrator) = orchestrator {
                    orchestrator.report_error(format!("{e:?}"));
                }
                anyhow::bail!(e);
            }
        }

        if let Some(ref orchestrator) = orchestrator {
            orchestrator.report_success()
        }

        Ok(())
    }

    pub fn print_host_status(&mut self) -> Result<(), Error> {
        print!(
            "{}",
            serde_yaml::to_string(self.datastore.host_status())
                .context("Failed to serialize HostStatus")?
        );

        Ok(())
    }
}

fn run_command(command: &mut Command) -> Result<Output, Error> {
    let output = command.output()?;
    if !output.status.success() {
        match output.status.code() {
            Some(exit_code) => bail!(
                "Command failed: {:?} with exit code: {}\n\nstdout:\n{}\n\nstderr:\n{}",
                command,
                exit_code,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
            None => bail!(
                "Command failed: {:?}\n\nstdout:\n{}\n\nstderr:\n{}",
                command,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    }
    Ok(output)
}

fn open_firewall_for_grpc() -> Result<(), Error> {
    run_command(
        Command::new("iptables")
            .arg("-A")
            .arg("INPUT")
            .arg("-p")
            .arg("tcp")
            .arg("--dport")
            .arg("50051") // TODO
            .arg("-j")
            .arg("ACCEPT"),
    )
    .context("Failed to open firewall for gRPC")?;
    Ok(())
}

mod tests {
    #![allow(unused_imports)]
    use indoc::indoc;
    use trident_api::{
        config::PartitionType,
        status::{
            AbVolumeSelection, BlockDeviceContents, BlockDeviceInfo, Disk, Partition,
            ReconcileState, UpdateKind,
        },
    };

    use super::*;
    use anyhow::anyhow;
    use std::path::{Path, PathBuf};

    #[test]
    fn test_run_command() {
        let result = run_command(Command::new("bash").arg("-c").arg("echo foobar"));
        assert_eq!(
            String::from_utf8_lossy(&result.as_ref().unwrap().stderr),
            ""
        );
        assert_eq!(String::from_utf8_lossy(&result.unwrap().stdout), "foobar\n");

        let result = run_command(Command::new("bash").arg("-c").arg("this-should-not-exist"));
        assert_eq!(result.err().unwrap().to_string(), "Command failed: \"bash\" \"-c\" \"this-should-not-exist\" with exit code: 127\n\nstdout:\n\n\nstderr:\nbash: line 1: this-should-not-exist: command not found\n");
    }

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
}
