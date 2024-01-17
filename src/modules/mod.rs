use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{bail, Context, Error};
use log::info;

use trident_api::{
    config::{HostConfiguration, Operations, PartitionType},
    error::{InitializationError, InternalError, ReportError, TridentResultExt},
    status::{
        AbVolumeSelection, BlockDeviceInfo, HostStatus, Partition, ReconcileState, UpdateKind,
    },
    BlockDeviceId,
};

use osutils::{
    chroot,
    efibootmgr::{self, EfiBootManagerOutput},
    files::create_dirs,
};

use crate::{
    datastore::DataStore, modules::storage::image::mount, protobufs::HostStatusState,
    TRIDENT_DATASTORE_PATH,
};
use crate::{
    modules::{
        self, hooks::HooksModule, management::ManagementModule, network::NetworkModule,
        osconfig::OsConfigModule, storage::StorageModule,
    },
    HostUpdateCommand,
};

pub mod hooks;
pub mod management;
pub mod network;
pub mod osconfig;
pub mod storage;

/// The path to the root of the freshly deployed (from provisioning OS) or
/// updated OS (from runtime OS).
const UPDATE_ROOT_PATH: &str = "/mnt/newroot";
/// Bootentry name for A images
const BOOT_ENTRY_A: &str = "azlinuxA";
/// Bootentry name for B images
const BOOT_ENTRY_B: &str = "azlinuxB";
trait Module: Send {
    fn name(&self) -> &'static str;

    // // TODO: Implement dependencies
    // fn dependencies(&self) -> &'static [&'static str];

    /// Refresh the host status.
    fn refresh_host_status(&mut self, host_status: &mut HostStatus) -> Result<(), Error>;

    /// Select the update kind based on the host status and host config.
    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        None
    }

    /// Validate the host config.
    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
        _planned_update: ReconcileState,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Perform non-destructive preparations for an update.
    fn prepare(
        &mut self,
        _host_status: &mut HostStatus,
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Initialize state on the Runtime OS from the Provisioning OS, or migrate state from
    /// A-partition to B-partition (or vice versa).
    ///
    /// This method is called before the chroot is entered, and is used to perform any
    /// provisioning operations that need to be done before the chroot is entered.
    fn provision(
        &mut self,
        _host_status: &mut HostStatus,
        _host_config: &HostConfiguration,
        _mount_path: &Path,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Configure the system as specified by the host configuration, and update the host status
    /// accordingly.
    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error>;
}

lazy_static::lazy_static! {
    static ref MODULES: Mutex<Vec<Box<dyn Module>>> = Mutex::new(vec![
        Box::<StorageModule>::default(),
        Box::<NetworkModule>::default(),
        Box::<OsConfigModule>::default(),
        Box::<ManagementModule>::default(),
        Box::<HooksModule>::default(),
    ]);
}

pub(super) fn provision_host(
    command: HostUpdateCommand,
    state: &mut DataStore,
) -> Result<(), Error> {
    let HostUpdateCommand {
        ref host_config,
        allowed_operations,
        mut sender,
    } = command;

    // TODO: needs to be refactored once we have a way to preserve existing partitions
    // This is a safety check so that nobody accidentally formats their dev machine.
    if !fs::read_to_string("/proc/cmdline")
        .context("Failed to read /proc/cmdline")?
        .contains("root=/dev/ram0")
        && !Path::new("/override-trident-safety-check").exists()
    {
        return Err(InitializationError::SafetyCheck.into());
    }

    let mut modules = MODULES.lock().unwrap();
    state.with_host_status(|s| s.reconcile_state = ReconcileState::CleanInstall)?;

    refresh_host_status(&mut modules, state)?;
    validate_host_config(&modules, state, host_config, ReconcileState::CleanInstall)?;

    if let Some(ref mut sender) = sender {
        sender
            .send(Ok(HostStatusState {
                status: serde_yaml::to_string(state.host_status())
                    .context("Failed to serialize host status")?,
            }))
            .context("Failed to send host status")?;
    }

    if !allowed_operations.contains(Operations::Update) {
        info!("Update not requested, skipping");
        return Ok(());
    }

    prepare(&mut modules, state, host_config)?;

    // TODO: We should have a way to indicate which modules setup the root mount point, and which
    // depend on it being in place. Right now we just depend on the "storage" and "image" modules
    // being the first ones to run.
    let mount_path = Path::new(UPDATE_ROOT_PATH);
    provision(&mut modules, state, host_config, mount_path)?;

    let chroot = chroot::enter_update_chroot(mount_path).unstructured("Failed to enter chroot")?;
    let datastore_path = get_datastore_path(host_config);
    let mut root_device_path = None;
    chroot
        .execute_and_exit(|| {
            state
                .persist(datastore_path)
                .structured(InternalError::Todo("Failed to persist datastore"))?;

            configure(&mut modules, state, host_config)
                .structured(InternalError::Todo("Failed to configure"))?;

            root_device_path = Some(
                get_root_block_device_path(state.host_status())
                    .structured(InternalError::Todo("Failed to get root block device"))?,
            );

            if let Some(sender) = sender {
                sender
                    .send(Ok(HostStatusState {
                        status: serde_yaml::to_string(state.host_status())
                            .structured(InternalError::Todo("Failed to serialize host status"))?,
                    }))
                    .structured(InternalError::Todo("Failed to send host status"))?;
                drop(sender);
            }

            Ok(())
        })
        .unstructured("Failed to execute in chroot")?;

    let root_device_path = root_device_path.context("Failed to get root block device")?;

    if !allowed_operations.contains(Operations::Transition) {
        info!("Transition not requested, skipping transition");

        // Store the generated config on the current root partition so that it can
        // be used later if need be prior to rebooting.
        let datastore_source_path = Path::new(UPDATE_ROOT_PATH).join(
            datastore_path
                .strip_prefix("/")
                .context("Datastore path must be absolute")?,
        );

        create_dirs(datastore_path.parent().context(format!(
            "Cannot get parent directory of datastore path: {}",
            datastore_path.display()
        ))?)
        .context(format!(
            "Failed to create parent directory for datastore path: {}",
            datastore_path.display()
        ))?;

        fs::copy(&datastore_source_path, datastore_path).context(format!(
            "Failed to copy generated config from {} to {}",
            datastore_source_path.display(),
            datastore_path.display()
        ))?;

        mount::unmount_updated_volumes(mount_path).context("Failed to unmount target volumes")?;

        return Ok(());
    }

    info!("Root device path: {:#?}", root_device_path);
    state.close();
    transition(mount_path, &root_device_path, state.host_status())?;

    Ok(())
}

pub(super) fn update(command: HostUpdateCommand, state: &mut DataStore) -> Result<(), Error> {
    let HostUpdateCommand {
        ref host_config,
        allowed_operations,
        mut sender,
    } = command;

    let mut modules = MODULES.lock().unwrap();

    refresh_host_status(&mut modules, state)?;
    if let Some(ref mut sender) = sender {
        sender
            .send(Ok(HostStatusState {
                status: serde_yaml::to_string(state.host_status())
                    .context("Failed to serialize host status")?,
            }))
            .context("Failed to send host status")?;
    }

    let update_kind = modules
        .iter()
        .filter_map(|m| {
            let update_kind = m.select_update_kind(state.host_status(), host_config);
            if let Some(update_kind) = update_kind {
                info!(
                    "Module '{}' selected update kind: {:?}",
                    m.name(),
                    update_kind
                );
            }
            update_kind
        })
        .max();
    let Some(update_kind) = update_kind else {
        info!("No updates required");
        state.with_host_status(|s| s.reconcile_state = ReconcileState::Ready)?;
        return Ok(());
    };

    validate_host_config(
        &modules,
        state,
        host_config,
        ReconcileState::UpdateInProgress(update_kind),
    )?;

    if !allowed_operations.contains(Operations::Update) {
        info!("Update not requested, skipping");
        return Ok(());
    }

    match update_kind {
        UpdateKind::HotPatch => info!("Performing hot patch update"),
        UpdateKind::NormalUpdate => info!("Performing normal update"),
        UpdateKind::UpdateAndReboot => info!("Performing update and reboot"),
        UpdateKind::AbUpdate => info!("Performing A/B update"),
        UpdateKind::Incompatible => {
            bail!("Requested host config is not compatible with current install")
        }
    }
    state
        .with_host_status(|s| s.reconcile_state = ReconcileState::UpdateInProgress(update_kind))?;

    prepare(&mut modules, state, host_config)?;

    let mount_path = Path::new(UPDATE_ROOT_PATH);

    if let UpdateKind::AbUpdate = update_kind {
        provision(&mut modules, state, host_config, mount_path)?;
        chroot::enter_update_chroot(mount_path)
            .unstructured("Failed to enter chroot")?
            .execute_and_exit(|| {
                configure(&mut modules, state, host_config)
                    .structured(InternalError::Todo("Failed to configure"))
            })
            .unstructured("Failed to execute in chroot")?;
    } else {
        configure(&mut modules, state, host_config)?;
    }

    if let Some(sender) = sender {
        sender
            .send(Ok(HostStatusState {
                status: serde_yaml::to_string(state.host_status())
                    .context("Failed to serialize host status")?,
            }))
            .context("Failed to send host status")?;
        drop(sender);
    }

    match update_kind {
        UpdateKind::UpdateAndReboot | UpdateKind::AbUpdate => {
            let root_block_device_path = get_root_block_device_path(state.host_status())
                .context("Failed to get root block device")?;

            if !allowed_operations.contains(Operations::Transition) {
                info!("Transition not requested, skipping transition");
                mount::unmount_updated_volumes(mount_path)
                    .context("Failed to unmount target volumes")?;
                return Ok(());
            }

            info!("Root device path: {:#?}", root_block_device_path);
            state.close();
            transition(mount_path, &root_block_device_path, state.host_status())?;

            Ok(())
        }
        UpdateKind::NormalUpdate | UpdateKind::HotPatch => {
            state.with_host_status(|s| s.reconcile_state = ReconcileState::Ready)?;
            info!("Update complete");
            Ok(())
        }
        UpdateKind::Incompatible => {
            unreachable!()
        }
    }
}

pub(super) fn get_datastore_path(host_config: &HostConfiguration) -> &Path {
    host_config
        .management
        .datastore_path
        .as_deref()
        .unwrap_or(Path::new(TRIDENT_DATASTORE_PATH))
}

/// Using the / mount point, figure out what should be used as a root block device.
fn get_root_block_device_path(host_status: &HostStatus) -> Option<PathBuf> {
    host_status
        .storage
        .mount_points
        .get(Path::new("/"))
        .and_then(|m| Some(get_block_device(host_status, &m.target_id, false)?.path))
}

fn get_disk(host_status: &HostStatus, block_device_id: &BlockDeviceId) -> Option<BlockDeviceInfo> {
    host_status
        .storage
        .disks
        .get(block_device_id)
        .map(|d| d.to_block_device())
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

fn get_encrypted_volume(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<BlockDeviceInfo> {
    host_status
        .storage
        .encrypted_volumes
        .get(block_device_id)
        .map(|e| e.to_block_device())
}

/// Returns a block device info for a block device referenced by the
/// `block_device_id`. If the volume is part of an AB Volume Pair and active is
/// true it returns the active volume, and if active is false it returns the
/// update volume (i.e. the one that isn't active).
fn get_block_device(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
    active: bool,
) -> Option<BlockDeviceInfo> {
    get_disk(host_status, block_device_id)
        .or_else(|| get_partition(host_status, block_device_id))
        .or_else(|| get_ab_volume(host_status, block_device_id, active))
        .or_else(|| get_raid_array(host_status, block_device_id))
        .or_else(|| get_encrypted_volume(host_status, block_device_id))
}

/// Returns a block device info for a volume from the given AB Volume Pair. If
/// active is true it returns the active volume, and if active is false it
/// returns the update volume (i.e. the one that isn't active).
fn get_ab_volume(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
    active: bool,
) -> Option<BlockDeviceInfo> {
    if let Some(ab_update) = &host_status.storage.ab_update {
        let ab_volume = ab_update
            .volume_pairs
            .iter()
            .find(|v| v.0 == block_device_id);
        if let Some(v) = ab_volume {
            // temporary hack to have one esp partition (esp-a)
            // task https://dev.azure.com/mariner-org/ECF/_workitems/edit/6289
            if v.0 == "esp" {
                return get_block_device(host_status, &v.1.volume_a_id, false);
            } else {
                return get_ab_update_volume(host_status, active).and_then(|selection| {
                    match selection {
                        AbVolumeSelection::VolumeA => {
                            get_block_device(host_status, &v.1.volume_a_id, false)
                        }
                        AbVolumeSelection::VolumeB => {
                            get_block_device(host_status, &v.1.volume_b_id, false)
                        }
                    }
                });
            }
        }
    }
    None
}

/// Returns the volume selection for all AB Volume Pairs. This is used to
/// determine which volumes are currently active and which are meant for
/// updating. In addition, if active is true and an A/B update is in progress,
/// the active volume selection will be returned. If active is false, the volume
/// selection corresponding to the volumes to be updated will be returned.
fn get_ab_update_volume(host_status: &HostStatus, active: bool) -> Option<AbVolumeSelection> {
    let active_volume = &host_status.storage.ab_update.as_ref()?.active_volume;
    match &host_status.reconcile_state {
        ReconcileState::UpdateInProgress(UpdateKind::HotPatch)
        | ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate)
        | ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot) => *active_volume,
        ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
            if active {
                *active_volume
            } else {
                Some(if *active_volume == Some(AbVolumeSelection::VolumeA) {
                    AbVolumeSelection::VolumeB
                } else {
                    AbVolumeSelection::VolumeA
                })
            }
        }
        ReconcileState::UpdateInProgress(UpdateKind::Incompatible) => None,
        ReconcileState::Ready => {
            if active {
                *active_volume
            } else {
                None
            }
        }
        ReconcileState::CleanInstall => Some(AbVolumeSelection::VolumeA),
    }
}

fn refresh_host_status(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
) -> Result<(), Error> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.refresh_host_status(s).context(format!(
                "Module '{}' failed to refresh host status",
                m.name()
            ))
        })?;
    }
    Ok(())
}

fn validate_host_config(
    modules: &[Box<dyn Module>],
    state: &DataStore,
    host_config: &HostConfiguration,
    planned_update: ReconcileState,
) -> Result<(), Error> {
    for m in modules {
        m.validate_host_config(state.host_status(), host_config, planned_update)
            .context(format!(
                "Module '{}' failed to validate host config",
                m.name()
            ))?;
    }
    info!("Host config validated");
    Ok(())
}

fn prepare(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.prepare(s, host_config)
                .context(format!("Module '{}' failed to prepare", m.name()))
        })?;
    }
    Ok(())
}

fn provision(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
    mount_point: &Path,
) -> Result<(), Error> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.provision(s, host_config, mount_point)
                .context(format!("Module '{}' failed to provision", m.name()))
        })?;
    }
    Ok(())
}

fn configure(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.configure(s, host_config)
                .context(format!("Module '{}' failed during configure", m.name()))
        })?;
    }
    Ok(())
}

fn transition(
    mount_path: &Path,
    root_block_device_path: &Path,
    host_status: &HostStatus,
) -> Result<(), Error> {
    let root_block_device_path = root_block_device_path.to_str().context(format!(
        "Failed to convert root device path {:?} to string",
        root_block_device_path
    ))?;
    info!("Setting boot entries");

    set_boot_entries(host_status).context("Failed to set boot entries")?;
    info!("Performing soft reboot");
    storage::image::kexec(
        mount_path,
        &format!("console=tty1 console=ttyS0 root={root_block_device_path}"),
    )
    .context("Failed to perform kexec")

    // TODO: Solve hard reboot(), so that the firmware chooses the correct boot
    // partition to boot from, after each A/B update. Related ADO task:
    // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6169.
    //info!("Performing hard reboot");
    //storage::image::reboot().context("Failed to perform hard reboot")
}

/// Creates a boot entry for the updated AB partition and sets the `BootNext` variable to boot from the updated partition on next boot.
fn set_boot_entries(host_status: &HostStatus) -> Result<(), Error> {
    //TODO- temporary https://dev.azure.com/mariner-org/ECF/_workitems/edit/6383/

    let bootloader_path = Path::new(r"/EFI/BOOT/bootx64.efi");
    let (entry_label_new, bootloader_path_new) =
        match modules::get_ab_update_volume(host_status, false) {
            Some(AbVolumeSelection::VolumeA) => (BOOT_ENTRY_A, bootloader_path),
            Some(AbVolumeSelection::VolumeB) => (BOOT_ENTRY_B, bootloader_path),
            None => bail!("Unsupported AB volume selection"),
        };

    let output = efibootmgr::list_bootmgr_entries()?;
    let bootmgr_output = EfiBootManagerOutput::parse_efibootmgr_output(&output)?;

    if !bootmgr_output.boot_entry_exists(entry_label_new)? {
        let disk_path = get_first_partition_of_type(host_status, PartitionType::Esp)
            .context("Failed to fetch esp disk path ")?;
        efibootmgr::create_boot_entry(entry_label_new, disk_path, bootloader_path_new)
            .context("Failed to add boot entry")?;
    }
    let output = efibootmgr::list_bootmgr_entries()?;
    let bootmgr_output = EfiBootManagerOutput::parse_efibootmgr_output(&output)?;

    let added_entry_number = bootmgr_output
        .get_boot_entry_number(entry_label_new)
        .context("Failed to get boot entry number")?;
    efibootmgr::set_bootnext(&added_entry_number).context("Failed to get set `BootNext`")
}

/// Returns disk path based on partitionType
fn get_first_partition_of_type(
    host_status: &HostStatus,
    partition_ty: PartitionType,
) -> Result<PathBuf, Error> {
    return host_status
        .storage
        .disks
        .values()
        .find_map(|disk| {
            disk.partitions
                .iter()
                .find(|partition| partition.ty == partition_ty)
                .map(|_| disk.to_block_device().path.clone())
        })
        .context("Failed to find disk path");
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use maplit::btreemap;
    use uuid::Uuid;

    use trident_api::{
        config::{Management, PartitionType},
        status::{AbUpdate, AbVolumePair, BlockDeviceContents, Disk, MountPoint, Storage},
    };

    use super::*;

    #[test]
    fn test_get_root_block_device_path() {
        let host_status = HostStatus {
            storage: Storage {
                disks: btreemap! {
                    "foo".into() => Disk {
                        uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000000u128),
                        path: PathBuf::from("/dev/sda"),
                        capacity: 10,
                        contents: BlockDeviceContents::Initialized,
                        partitions: vec![
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000001u128),
                                path: PathBuf::from("/dev/sda1"),
                                id: "boot".into(),
                                start: 1,
                                end: 3,
                                ty: PartitionType::Esp,
                                contents: BlockDeviceContents::Initialized,
                            },
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000002u128),
                                path: PathBuf::from("/dev/sda2"),
                                id: "root".into(),
                                start: 4,
                                end: 10,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Initialized,
                            },
                        ],
                    },
                },
                mount_points: btreemap! {
                    PathBuf::from("/boot") => MountPoint {
                        target_id: "boot".to_owned(),
                        filesystem: "fat32".to_owned(),
                        options: vec![],
                    },
                    PathBuf::from("/") => MountPoint {
                        target_id: "root".to_owned(),
                        filesystem: "ext4".to_owned(),
                        options: vec![],
                    },
                },
                ..Default::default()
            },
            reconcile_state: ReconcileState::CleanInstall,
            ..Default::default()
        };

        assert_eq!(
            get_root_block_device_path(&host_status),
            Some(PathBuf::from("/dev/sda2"))
        );
    }

    /// Validates that the `get_block_device_for_update` function works as expected for
    /// disks, partitions and ab volumes.
    #[test]
    fn test_get_block_device_for_update() {
        let mut host_status = HostStatus {
            storage: Storage {
                disks: btreemap! {
                    "os".to_owned() => Disk {
                        uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000000u128),
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                uuid: Uuid::from_u128(
                                    0x00000000_0000_0000_0000_000000000001u128,
                                ),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                                id: "efi".into(),
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                contents: BlockDeviceContents::Unknown,
                            },
                            Partition {
                                uuid: Uuid::from_u128(
                                    0x00000000_0000_0000_0000_000000000002u128,
                                ),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                                id: "root".into(),
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Unknown,
                            },
                            Partition {
                                uuid: Uuid::from_u128(
                                    0x00000000_0000_0000_0000_000000000003u128,
                                ),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                                id: "rootb".into(),
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Unknown,
                            },
                        ],
                    },
                    "data".into() => Disk {
                        uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000004u128),
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        capacity: 1000,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![],
                    },
                },
                ab_update: Some(AbUpdate {
                    volume_pairs: btreemap! {
                        "osab".to_owned() => AbVolumePair {
                            volume_a_id: "root".to_owned(),
                            volume_b_id: "rootb".to_owned(),
                        },
                    },
                    ..Default::default()
                }),
                ..Default::default()
            },
            reconcile_state: ReconcileState::CleanInstall,
            ..Default::default()
        };

        assert_eq!(
            get_block_device(&host_status, &"os".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"efi".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"root".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"foobar".to_owned(), false),
            None
        );
        assert_eq!(
            get_block_device(&host_status, &"data".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 1000,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            super::get_block_device(&host_status, &"osab".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                size: 9000,
                contents: BlockDeviceContents::Unknown,
            }
        );

        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), true).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
    }

    /// Validates logic for determining which A/B volume to use
    fn test_get_ab_update_volume(active: bool) -> HostStatus {
        let mut host_status = HostStatus {
            storage: Storage {
                ab_update: Some(AbUpdate {
                    volume_pairs: BTreeMap::new(),
                    ..Default::default()
                }),
                ..Default::default()
            },
            reconcile_state: ReconcileState::CleanInstall,
            ..Default::default()
        };

        // test that clean-install will always use volume A for updates
        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);

        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);

        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeA)
        );

        // test that UpdateInProgress(HostPatch, NormalUpdate, UpdateAndReboot)
        // will always use the active volume for updates
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::HotPatch);
        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeB)
        );
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeB)
        );
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot);
        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeA)
        );

        // test that UpdateInProgress(Incompatible) will return None
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::Incompatible);
        assert_eq!(get_ab_update_volume(&host_status, active), None);

        host_status
    }

    /// Validates logic for determining which A/B volume to update
    #[test]
    fn test_get_ab_update_volume_update() {
        let mut host_status = test_get_ab_update_volume(false);

        // test that Ready will return the None
        host_status.reconcile_state = ReconcileState::Ready;
        assert_eq!(get_ab_update_volume(&host_status, false), None);

        // test that UpdateInProgress(AbUpdate) will use the opposite volume
        // for updates
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status, false),
            Some(AbVolumeSelection::VolumeB)
        );
        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_ab_update_volume(&host_status, false),
            Some(AbVolumeSelection::VolumeA)
        );
    }

    /// Validates logic for determining which A/B volume is active
    #[test]
    fn test_get_ab_update_volume_active() {
        let mut host_status = test_get_ab_update_volume(true);

        // test that Ready will return the active volume
        host_status.reconcile_state = ReconcileState::Ready;
        assert_eq!(
            get_ab_update_volume(&host_status, true),
            Some(AbVolumeSelection::VolumeA)
        );

        // test that UpdateInProgress(AbUpdate) will use the active volume
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status, true),
            Some(AbVolumeSelection::VolumeA)
        );
        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_ab_update_volume(&host_status, true),
            Some(AbVolumeSelection::VolumeB)
        );
    }

    /// Validates logic for querying disks and partitions.
    #[test]
    fn test_get_disk_partition() {
        let host_status = HostStatus {
            storage: Storage {
                disks: btreemap! {
                    "os".to_owned() => Disk {
                        uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000000u128),
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000001u128),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                                id: "efi".into(),
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                contents: BlockDeviceContents::Unknown,
                            },
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000002u128),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                                id: "root".into(),
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Unknown,
                            },
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000003u128),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                                id: "rootb".into(),
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Unknown,
                            },
                        ],
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

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
    }

    #[test]
    fn test_get_datastore_path() {
        let host_config = HostConfiguration {
            ..Default::default()
        };
        assert_eq!(
            get_datastore_path(&host_config),
            Path::new(TRIDENT_DATASTORE_PATH)
        );

        let host_config = HostConfiguration {
            management: Default::default(),
            ..Default::default()
        };
        assert_eq!(
            get_datastore_path(&host_config),
            Path::new(TRIDENT_DATASTORE_PATH)
        );

        let host_config = HostConfiguration {
            management: Management {
                datastore_path: Some(PathBuf::from("/foo")),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(get_datastore_path(&host_config), Path::new("/foo"));
    }

    #[test]
    fn test_get_first_partition_of_type() {
        let host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/sda"),
                        uuid: Uuid::nil(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "efi".to_string(),
                                path: PathBuf::from("/dev/sda1"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root-a".to_string(),
                                path: PathBuf::from("/dev/sda2"),
                                contents: BlockDeviceContents::Unknown,
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                path: PathBuf::from("/dev/sda3"),
                                contents: BlockDeviceContents::Unknown,
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            }

                        ],
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let result = get_first_partition_of_type(&host_status, PartitionType::Esp);
        assert_eq!(result.unwrap(), PathBuf::from("/dev/sda"));

        let result = get_first_partition_of_type(&host_status, PartitionType::Root);
        assert_eq!(result.unwrap(), PathBuf::from("/dev/sda"));
        let result = get_first_partition_of_type(&host_status, PartitionType::Var);
        assert!(result.is_err());
    }
}

#[cfg(feature = "functional-tests")]
mod functional_tests {
    #[cfg(test)]
    use super::*;

    #[cfg(test)]
    use maplit::btreemap;
    #[cfg(test)]
    use uuid::Uuid;

    #[cfg(test)]
    use osutils::efibootmgr;
    use pytest_gen::pytest;
    #[cfg(test)]
    use trident_api::status::{AbUpdate, AbVolumePair, BlockDeviceContents, Disk, Storage};

    #[cfg(test)]
    fn test_helper_set_bootentries(entry_label: &str, host_status: &HostStatus) {
        let output1 = efibootmgr::list_bootmgr_entries().unwrap();
        let bootmgr_output1: EfiBootManagerOutput =
            EfiBootManagerOutput::parse_efibootmgr_output(&output1).unwrap();
        if bootmgr_output1.boot_entry_exists(entry_label).unwrap() {
            let boot_entry_num1 = bootmgr_output1.get_boot_entry_number(entry_label).unwrap();
            efibootmgr::delete_boot_entry(&boot_entry_num1).unwrap();
        }
        set_boot_entries(host_status).unwrap();
        let output2 = efibootmgr::list_bootmgr_entries().unwrap();
        let bootmgr_output2: EfiBootManagerOutput =
            EfiBootManagerOutput::parse_efibootmgr_output(&output2).unwrap();
        let boot_entry_num2 = bootmgr_output2.get_boot_entry_number(entry_label).unwrap();
        assert_eq!(bootmgr_output2.boot_next, boot_entry_num2);
        efibootmgr::delete_boot_entry(&boot_entry_num2).unwrap();
    }

    #[pytest()]
    fn test_set_bootentries() {
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/sda"),
                        uuid: Uuid::nil(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "efi".to_string(),
                                path: PathBuf::from("/dev/sda1"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root-a".to_string(),
                                path: PathBuf::from("/dev/sda2"),
                                contents: BlockDeviceContents::Unknown,
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                path: PathBuf::from("/dev/sda3"),
                                contents: BlockDeviceContents::Unknown,
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },

                        ],
                    },

                },
                ab_update: Some(AbUpdate {
                    volume_pairs: btreemap! {
                        "root".to_string() => AbVolumePair {
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        },
                    },
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        //for cleanInstall add A partition entry
        test_helper_set_bootentries(BOOT_ENTRY_A, &host_status);

        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);

        test_helper_set_bootentries(BOOT_ENTRY_A, &host_status);

        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate);

        test_helper_set_bootentries(BOOT_ENTRY_B, &host_status);
    }
}
