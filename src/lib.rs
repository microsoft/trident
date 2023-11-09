use anyhow::{bail, Context, Error};
use datastore::DataStore;
use log::{debug, error, info, warn};
use protobufs::*;
use setsail::KsTranslator;
use std::net::{IpAddr, SocketAddr};
use std::{fs, mem};

use std::path::Path;
use std::process::{Command, Output};
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use trident_api::config::{
    DatastoreConfiguration, HostConfiguration, HostConfigurationSource, LocalConfigFile,
    TridentConfiguration,
};

mod datastore;
mod logstream;
mod modules;
mod mount;
mod multilog;
mod orchestrate;

pub use modules::network::provisioning::start as start_provisioning_network;

pub use logstream::Logstream;
pub use multilog::MultiLogger;
pub use orchestrate::OrchestratorConnection;

pub const TRIDENT_LOCAL_CONFIG_PATH: &str = "/etc/trident/config.yaml";
pub const TRIDENT_DATASTORE_PATH: &str = "/var/lib/trident/datastore.sqlite";
pub const TRIDENT_BINARY_PATH: &str = "/usr/bin/trident";

mod protobufs {
    tonic::include_proto!("trident");
}

pub fn serve(addr: IpAddr, port: u16) -> Result<(), Error> {
    tokio::runtime::Runtime::new()
        .context("Failed to start tokio runtime")?
        .block_on(async {
            Server::builder()
                .add_service(imaging_server::ImagingServer::new(ImagingImpl))
                .serve(SocketAddr::new(addr, port))
                .await
                .context("Failed while serving gRPC requests")
        })
}

#[derive(Default)]
pub struct ImagingImpl;

#[tonic::async_trait]
impl imaging_server::Imaging for ImagingImpl {
    async fn write_image(
        &self,
        request: Request<ImageRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let _request = request.into_inner();
        // image::write_image(Path::new(&request.disk), &request.url, &request.sha256)
        //     .await
        //     .map_err(|e| Status::unknown(e.to_string()))?;

        Ok(Response::new(EmptyReply {}))
    }

    async fn chroot_exec(
        &self,
        request: Request<ChrootExecRequest>,
    ) -> Result<Response<EmptyReply>, Status> {
        let _request = request.into_inner();
        // image::chroot_exec(Path::new(&request.root_partition), &request.script)
        //     .await
        //     .map_err(|e| Status::unknown(e.to_string()))?;

        Ok(Response::new(EmptyReply {}))
    }

    async fn kexec(&self, request: Request<KexecRequest>) -> Result<Response<EmptyReply>, Status> {
        let _request = request.into_inner();
        // image::kexec(Path::new(&request.root_partition), &request.cmdline)
        //     .await
        //     .map_err(|e| Status::unknown(e.to_string()))?;
        unreachable!()
    }
}

pub struct Trident {
    config: LocalConfigFile,
    datastore: DataStore,
    _server_runtime: Option<tokio::runtime::Runtime>,
}
impl Trident {
    pub fn new(config_path: &Path, logstream: Logstream) -> Result<Self, Error> {
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
        if let Some(url) = config.trident_config.logstream.as_ref() {
            logstream
                .set_server(url.to_string())
                .context("Failed to set logstream URL")?;
        }

        debug!(
            "Trident config:\n{}",
            serde_yaml::to_string(&config).unwrap_or("Failed to serialize host config".into())
        );

        let datastore = match config.trident_config.datastore {
            Some(DatastoreConfiguration::Load { ref load_path }) => {
                DataStore::open(load_path).context("Failed to load datastore")?
            }
            _ => DataStore::new(),
        };

        Ok(Self {
            config,
            datastore,
            _server_runtime: None,
        })
    }

    fn load_host_config(&mut self) -> Result<Option<Box<HostConfiguration>>, Error> {
        let host_config = match &mut self.config.host_config_source {
            HostConfigurationSource::File(path) => {
                info!("Loading host config from '{}'", path.display());

                Some(
                    serde_yaml::from_str(
                        &fs::read_to_string(path).context("Failed to read host config file")?,
                    )
                    .context("Failed to parse host config file")?,
                )
            }
            HostConfigurationSource::Embedded(contents) => Some(mem::take(contents)),
            HostConfigurationSource::GrpcCommand { .. } => None,
            HostConfigurationSource::KickstartEmbedded(contents) => {
                match KsTranslator::new()
                    .run_pre_scripts(true)
                    .translate(setsail::load_kickstart_string(contents))
                {
                    Ok(hc) => Some(Box::new(hc)),
                    Err(e) => {
                        // TODO: handle & report kickstart errors
                        error!(
                            "Failed to translate kickstart:\n{}",
                            serde_json::to_string_pretty(&e)?
                        );
                        None
                    }
                }
            }
            HostConfigurationSource::Kickstart(file) => {
                match KsTranslator::new().run_pre_scripts(true).translate(
                    setsail::load_kickstart_file(
                        file.to_str()
                            .context(format!("Failed to resolve path {}", file.display()))?,
                    )?,
                ) {
                    Ok(hc) => Some(Box::new(hc)),
                    Err(e) => {
                        error!(
                            // TODO: handle & report kickstart errors
                            "Failed to translate kickstart:\n{}",
                            serde_json::to_string_pretty(&e)?
                        );
                        None
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
        if let HostConfigurationSource::Kickstart(_)
        | HostConfigurationSource::KickstartEmbedded(_) = self.config.host_config_source
        {
            warn!("Cannot set up network early when using kickstart");
            return Ok(());
        }

        let host_config = self.load_host_config()?;

        info!("Starting network");
        start_provisioning_network(
            self.config.trident_config.network_override.clone(),
            host_config.as_deref(),
        )
        .context("Failed to start provisioning network")
    }

    pub fn run(&mut self) -> Result<(), Error> {
        let host_config = self.load_host_config()?;

        let orchestrator = self
            .config
            .trident_config
            .phonehome
            .as_ref()
            .and_then(|url| OrchestratorConnection::new(url.clone()));

        match self.config.host_config_source {
            HostConfigurationSource::File(_)
            | HostConfigurationSource::Embedded(_)
            | HostConfigurationSource::Kickstart(_)
            | HostConfigurationSource::KickstartEmbedded(_) => {
                info!("Running");
                match run(
                    *host_config.unwrap(),
                    &self.config.trident_config,
                    &mut self.datastore,
                ) {
                    Ok(()) => {
                        if let Some(orchestrator) = orchestrator {
                            orchestrator.report_success()
                        }
                    }
                    Err(e) => {
                        error!("{e:?}");
                        if let Some(orchestrator) = orchestrator {
                            orchestrator.report_error(format!("{e:?}"));
                        }
                    }
                }
            }
            HostConfigurationSource::GrpcCommand { listen_port } => {
                info!("Listening");
                if let Some(orchestrator) = orchestrator {
                    orchestrator.report_success()
                }
                serve("0.0.0.0".parse().unwrap(), listen_port.unwrap_or(50051))?;
            }
        }
        Ok(())
    }
}

fn run(
    mut host_config: HostConfiguration,
    trident_config: &TridentConfiguration,
    datastore: &mut DataStore,
) -> Result<(), Error> {
    if trident_config.phonehome.is_some() && host_config.management.phonehome.is_none() {
        info!("Injecting phonehome into host configuration");
        host_config.management.phonehome = trident_config.phonehome.clone();
    }

    match &trident_config.datastore {
        Some(DatastoreConfiguration::Load { .. }) => {
            modules::update(&host_config, trident_config, datastore)
                .context("Failed to update host config")
        }
        Some(DatastoreConfiguration::Create { .. }) | None => {
            modules::provision(&host_config, trident_config, datastore)
                .context("Failed to provision")
        }
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
