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
    BlockDeviceId, DatastoreConfiguration, HostConfiguration, HostConfigurationSource,
    LocalConfigFile, TridentConfiguration,
};
use trident_api::status::{
    AbVolumeSelection, BlockDeviceContents, BlockDeviceInfo, Disk, HostStatus, Partition,
    ReconcileState, UpdateKind,
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

fn get_ab_update_volume(host_status: &HostStatus) -> Option<AbVolumeSelection> {
    let active_volume = &host_status.imaging.ab_update.as_ref()?.active_volume;
    match &host_status.reconcile_state {
        ReconcileState::UpdateInProgress(update_kind) => match update_kind {
            UpdateKind::HotPatch => *active_volume,
            UpdateKind::NormalUpdate => *active_volume,
            UpdateKind::UpdateAndReboot => *active_volume,
            UpdateKind::AbUpdate => Some(if *active_volume == Some(AbVolumeSelection::VolumeA) {
                AbVolumeSelection::VolumeB
            } else {
                AbVolumeSelection::VolumeA
            }),
            UpdateKind::Incompatible => None,
        },
        ReconcileState::Ready => *active_volume,
        ReconcileState::CleanInstall => Some(AbVolumeSelection::VolumeA),
    }
}

fn set_host_status_block_device_contents(
    host_status: &mut HostStatus,
    block_device_id: &BlockDeviceId,
    contents: BlockDeviceContents,
) -> Result<(), Error> {
    if let Some(disk) = get_disk_mut(host_status, block_device_id) {
        disk.contents = contents;
        return Ok(());
    }

    if let Some(partition) = get_partition_mut(host_status, block_device_id) {
        partition.contents = contents;
        return Ok(());
    }

    if let Some(ab_update) = &host_status.imaging.ab_update {
        if let Some(ab_volume_pair) = ab_update.volume_pairs.get(block_device_id) {
            let target_id = match get_ab_update_volume(host_status) {
                Some(AbVolumeSelection::VolumeA) => Some(&ab_volume_pair.volume_a_id),
                Some(AbVolumeSelection::VolumeB) => Some(&ab_volume_pair.volume_b_id),
                None => None,
            };
            if let Some(target_id) = target_id {
                return set_host_status_block_device_contents(
                    host_status,
                    &target_id.clone(),
                    contents,
                );
            }
        }
    }

    anyhow::bail!("No block device with id '{}' found", block_device_id);
}

fn get_block_device(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<BlockDeviceInfo> {
    get_disk(host_status, block_device_id)
        .or_else(|| get_partition(host_status, block_device_id))
        .or_else(|| get_ab_volume(host_status, block_device_id))
        .or_else(|| get_raid_array(host_status, block_device_id))
}

fn get_disk(host_status: &HostStatus, block_device_id: &BlockDeviceId) -> Option<BlockDeviceInfo> {
    host_status
        .storage
        .disks
        .get(block_device_id)
        .map(|d| d.to_block_device())
}

fn get_disk_mut<'a>(
    host_status: &'a mut HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<&'a mut Disk> {
    host_status.storage.disks.get_mut(block_device_id)
}

fn get_partition(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<BlockDeviceInfo> {
    host_status
        .storage
        .disks
        .iter()
        .flat_map(|(_block_device_id, disk)| &disk.partitions)
        .find(|p| p.id == *block_device_id)
        .map(Partition::to_block_device)
}

fn get_partition_mut<'a>(
    host_status: &'a mut HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<&'a mut Partition> {
    host_status
        .storage
        .disks
        .iter_mut()
        .flat_map(|(_block_device_id, disk)| &mut disk.partitions)
        .find(|p| p.id == *block_device_id)
}

fn get_ab_volume(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<BlockDeviceInfo> {
    if let Some(ab_update) = &host_status.imaging.ab_update {
        let ab_volume = ab_update
            .volume_pairs
            .iter()
            .find(|v| v.0 == block_device_id);
        if let Some(v) = ab_volume {
            return get_ab_update_volume(host_status).and_then(|selection| match selection {
                AbVolumeSelection::VolumeA => get_block_device(host_status, &v.1.volume_a_id),
                AbVolumeSelection::VolumeB => get_block_device(host_status, &v.1.volume_b_id),
            });
        }
    }

    None
}

fn get_raid_array(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<BlockDeviceInfo> {
    host_status
        .storage
        .raid_arrays
        .get(block_device_id)
        .map(|r| r.to_block_device())
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
    use trident_api::{config::PartitionType, status::BlockDeviceContents};

    use super::*;
    use std::path::{Path, PathBuf};

    /// Validates that the `get_block_device` function works as expected for
    /// disks, partitions and ab volumes.
    #[test]
    fn test_get_block_device() {
        let host_status_yaml = indoc! {r#"
            storage:
                mount-points:
                disks:
                    os:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: 0
                        contents: unknown
                        partitions:
                          - id: efi
                            path: /dev/disk/by-partlabel/osp1
                            contents: unknown
                            start: 0
                            end: 0
                            type: esp
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: root
                            path: /dev/disk/by-partlabel/osp2
                            contents: unknown
                            start: 100
                            end: 1000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: rootb
                            path: /dev/disk/by-partlabel/osp3
                            contents: unknown
                            start: 1000
                            end: 10000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                    data:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: 1000
                        contents: unknown
                        partitions: []
                raid-arrays:
            imaging:
                ab-update:
                    volume-pairs:
                        osab:
                            volume-a-id: root
                            volume-b-id: rootb
            reconcile-state: clean-install
        "#};
        let mut host_status: HostStatus = serde_yaml::from_str(host_status_yaml).unwrap();

        assert_eq!(
            get_block_device(&host_status, &"os".to_owned()).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"efi".to_owned()).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"root".to_owned()).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(get_block_device(&host_status, &"foobar".to_owned()), None);
        assert_eq!(
            get_block_device(&host_status, &"data".to_owned()).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 1000,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned()).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            super::get_block_device(&host_status, &"osab".to_owned()).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned()).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                size: 9000,
                contents: BlockDeviceContents::Unknown,
            }
        );
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

    /// Validates logic for querying disks and partitions.
    #[test]
    fn test_get_disk_partition() {
        let host_status_yaml = indoc! {r#"
            storage:
                mount-points:
                disks:
                    os:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: 0
                        contents: unknown
                        partitions:
                          - id: efi
                            path: /dev/disk/by-partlabel/osp1
                            contents: unknown
                            start: 0
                            end: 0
                            type: esp
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: root
                            path: /dev/disk/by-partlabel/osp2
                            contents: unknown
                            start: 100
                            end: 1000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: rootb
                            path: /dev/disk/by-partlabel/osp3
                            contents: unknown
                            start: 1000
                            end: 10000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                raid-arrays:
            imaging:
                ab-update:
                    volume-pairs:
            reconcile-state: clean-install
        "#};
        let mut host_status: HostStatus = serde_yaml::from_str(host_status_yaml).unwrap();

        assert_eq!(
            get_disk(&host_status, &"os".to_owned()).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(get_disk(&host_status, &"efi".to_owned()), None);
        assert_eq!(get_partition(&host_status, &"os".to_owned()), None);
        assert_eq!(
            get_partition(&host_status, &"efi".to_owned()),
            Some(BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            })
        );

        let disk_mut = get_disk_mut(&mut host_status, &"os".to_owned());
        disk_mut.unwrap().contents = BlockDeviceContents::Zeroed;
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .contents,
            BlockDeviceContents::Zeroed
        );

        let partition_mut = get_partition_mut(&mut host_status, &"efi".to_owned());
        partition_mut.unwrap().contents = BlockDeviceContents::Initialized;
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(0)
                .unwrap()
                .contents,
            BlockDeviceContents::Initialized
        );
    }

    /// Validates logic for determining which A/B volume to update
    #[test]
    fn test_get_ab_update_volume() {
        let host_status_yaml = indoc! {r#"
            storage:
                disks:
                mount-points:
                raid-arrays:
            imaging:
                ab-update:
                    volume-pairs:
            reconcile-state: clean-install
        "#};
        let mut host_status: HostStatus = serde_yaml::from_str(host_status_yaml).unwrap();

        // test that clean-install will always use volume A for updates
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);

        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);

        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        // test that UpdateInProgress(HostPatch, NormalUpdate, UpdateAndReboot)
        // will always use the active volume for updates
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::HotPatch);
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeB)
        );
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeB)
        );
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot);
        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        // test that UpdateInProgress(AbUpdate) will use the opposite volume
        // for updates
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeB)
        );
        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        // test that UpdateInProgress(Incompatible) will return None
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::Incompatible);
        assert_eq!(get_ab_update_volume(&host_status), None);

        // test that Ready will return None
        host_status.reconcile_state = ReconcileState::Ready;
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeB)
        );
    }

    /// Validates logic for setting block device contents
    #[test]
    fn test_set_host_status_block_device_contents() {
        let host_status_yaml = indoc! {r#"
            storage:
                mount-points:
                disks:
                    os:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: 0
                        contents: unknown
                        partitions:
                          - id: efi
                            path: /dev/disk/by-partlabel/osp1
                            contents: unknown
                            start: 0
                            end: 0
                            type: esp
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: root
                            path: /dev/disk/by-partlabel/osp2
                            contents: unknown
                            start: 100
                            end: 1000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: rootb
                            path: /dev/disk/by-partlabel/osp3
                            contents: unknown
                            start: 1000
                            end: 10000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                    data:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: 1000
                        contents: unknown
                        partitions: []
                raid-arrays:
            imaging:
                ab-update:
                    volume-pairs:
                        osab:
                            volume-a-id: root
                            volume-b-id: rootb
            reconcile-state: clean-install
        "#};
        let mut host_status: HostStatus = serde_yaml::from_str(host_status_yaml).unwrap();
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .contents,
            BlockDeviceContents::Unknown
        );
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(0)
                .unwrap()
                .contents,
            BlockDeviceContents::Unknown
        );
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(1)
                .unwrap()
                .contents,
            BlockDeviceContents::Unknown
        );

        // test for disks
        let contents = BlockDeviceContents::Zeroed;
        set_host_status_block_device_contents(&mut host_status, &"os".to_owned(), contents.clone())
            .unwrap();
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .contents,
            contents.clone()
        );

        // test for partitions
        set_host_status_block_device_contents(
            &mut host_status,
            &"efi".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(0)
                .unwrap()
                .contents,
            contents.clone()
        );

        // test for ab volumes
        set_host_status_block_device_contents(
            &mut host_status,
            &"osab".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(1)
                .unwrap()
                .contents,
            contents.clone()
        );

        assert!(set_host_status_block_device_contents(
            &mut host_status,
            &"foorbar".to_owned(),
            contents.clone()
        )
        .is_err());
    }
}
