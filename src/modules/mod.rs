use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    thread,
    time::Duration,
};

use anyhow::Error;
use log::{error, info};

use trident_api::{
    config::{HostConfiguration, Operations},
    constants::{self, ROOT_MOUNT_POINT_PATH},
    error::{
        DatastoreError, InitializationError, InternalError, InvalidInputError, ManagementError,
        ModuleError, ReportError, TridentError, TridentResultExt,
    },
    status::{
        AbVolumeSelection, BlockDeviceInfo, HostStatus, Partition, ReconcileState, UpdateKind,
    },
    BlockDeviceId,
};

use osutils::{chroot, container, exe::RunAndCheck, mkinitrd};

use crate::{datastore::DataStore, protobufs::HostStatusState, TRIDENT_DATASTORE_REF_PATH};
use crate::{
    modules::{
        boot::BootModule, hooks::HooksModule, management::ManagementModule, network::NetworkModule,
        osconfig::OsConfigModule, storage::StorageModule,
    },
    HostUpdateCommand,
};

// Trident modules
pub mod boot;
pub mod hooks;
pub mod management;
pub mod network;
pub mod osconfig;
pub mod storage;

// Helper modules
pub mod bootentries;
pub mod kexec;
pub mod mount_root;

/// The path to the root of the freshly deployed (from provisioning OS) or
/// updated OS (from runtime OS).
const UPDATE_ROOT_PATH: &str = "/mnt/newroot";
/// Bootentry name for A images
const BOOT_ENTRY_A: &str = "AZLA";
/// Bootentry name for B images
const BOOT_ENTRY_B: &str = "AZLB";
/// Boot efi executable
const BOOT64_EFI: &str = "bootx64.efi";
/// Trident will by default prevent running Clean Install on deployments other
/// than from the Provisioning ISO, to limit chances of accidental data loss. To
/// override, user can create this file on the host.
const SAFETY_OVERRIDE_CHECK_PATH: &str = "/override-trident-safety-check";

trait Module: Send {
    fn name(&self) -> &'static str;

    // // TODO: Implement dependencies
    // fn dependencies(&self) -> &'static [&'static str];

    /// Refresh the host status.
    fn refresh_host_status(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
        Ok(())
    }

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
        _host_status: &mut HostStatus,
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        Ok(())
    }
}

lazy_static::lazy_static! {
    static ref MODULES: Mutex<Vec<Box<dyn Module>>> = Mutex::new(vec![
        Box::<StorageModule>::default(),
        Box::<BootModule>::default(),
        Box::<NetworkModule>::default(),
        Box::<OsConfigModule>::default(),
        Box::<ManagementModule>::default(),
        Box::<HooksModule>::default(),
    ]);
}

pub(super) fn provision_host(
    command: HostUpdateCommand,
    state: &mut DataStore,
) -> Result<(), TridentError> {
    let HostUpdateCommand {
        ref host_config,
        allowed_operations,
        mut sender,
    } = command;

    {
        // TODO: needs to be refactored once we have a way to preserve existing partitions
        // This is a safety check so that nobody accidentally formats their dev
        // machine.
        let cmdline =
            fs::read_to_string("/proc/cmdline").structured(InitializationError::SafetyCheck)?;
        if !cmdline.contains("root=/dev/ram0")
            && !cmdline.contains("root=live:LABEL=CDROM")
            && (container::is_running_in_container()? // if running on the host, check for this path
                || !Path::new(SAFETY_OVERRIDE_CHECK_PATH).exists())
            && (!container::is_running_in_container()? // if running in a container, check for this path
                || !container::get_host_root_path()?
                    .join(SAFETY_OVERRIDE_CHECK_PATH.trim_start_matches(ROOT_MOUNT_POINT_PATH))
                    .exists())
        {
            return Err(TridentError::new(InitializationError::SafetyCheck));
        }
    }

    info!("Starting provision_host");
    let mut modules = MODULES.lock().unwrap();
    state.with_host_status(|s| s.reconcile_state = ReconcileState::CleanInstall)?;

    info!("Refreshing host status");
    refresh_host_status(&mut modules, state)?;

    info!("Validating host configuration against system state");
    validate_host_config(&modules, state, host_config, ReconcileState::CleanInstall)?;

    if let Some(ref mut sender) = sender {
        sender
            .send(Ok(HostStatusState {
                status: serde_yaml::to_string(state.host_status())
                    .structured(InternalError::SerializeHostStatus)?,
            }))
            .structured(InternalError::SendHostStatus)?;
    }

    if !allowed_operations.contains(Operations::Update) {
        info!("Update not requested, skipping");
        return Ok(());
    }

    info!("Running prepare");
    prepare(&mut modules, state, host_config)?;

    info!("Preparing storage to mount new root");
    let new_root_path = Path::new(UPDATE_ROOT_PATH);
    let mounts = initialize_new_root(state, host_config, new_root_path)?;

    info!("Running provision");
    provision(&mut modules, state, host_config, new_root_path)?;

    let datastore_ref = File::create(TRIDENT_DATASTORE_REF_PATH).structured(
        ManagementError::from(DatastoreError::CreateDatastoreRefFile),
    )?;
    let datastore_path = state
        .host_status()
        .management
        .datastore_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("/tmp/datastore.sqlite"));

    info!("Entering /mnt/newroot chroot");
    let chroot = chroot::enter_update_chroot(new_root_path).message("Failed to enter chroot")?;
    let mut root_device_path = None;

    chroot
        .execute_and_exit(|| {
            info!("Persisting datastore");
            state.persist(&datastore_path)?;

            management::record_datastore_location(
                state.host_status(),
                &datastore_path,
                datastore_ref,
            )?;

            info!("Running configure");
            configure(&mut modules, state, host_config)?;

            info!("Regenerating initrd");
            regenerate_initrd()?;

            root_device_path = Some(
                get_root_block_device_path(state.host_status())
                    .structured(InternalError::GetRootBlockDevice)?,
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

            info!("Closing datastore");
            state.close();
            Ok(())
        })
        .message("Failed to execute in chroot")?;

    let root_device_path =
        root_device_path.structured(InternalError::Internal("Failed to get root block device"))?;

    if !allowed_operations.contains(Operations::Transition) {
        info!("Transition not requested, skipping transition");
        info!("Unmounting /mnt/newroot");
        mount_root::unmount_new_root(mounts, new_root_path)?;
    } else {
        info!("Performing transition");
        transition(new_root_path, &root_device_path, state.host_status())?;
    }

    Ok(())
}

pub(super) fn update(
    command: HostUpdateCommand,
    state: &mut DataStore,
) -> Result<(), TridentError> {
    let HostUpdateCommand {
        ref host_config,
        allowed_operations,
        mut sender,
    } = command;

    let mut modules = MODULES.lock().unwrap();

    info!("Refreshing host status");
    refresh_host_status(&mut modules, state)?;
    if let Some(ref mut sender) = sender {
        sender
            .send(Ok(HostStatusState {
                status: serde_yaml::to_string(state.host_status())
                    .structured(InternalError::SerializeHostStatus)?,
            }))
            .structured(InternalError::SendHostStatus)?;
    }

    info!("Determining update kind");
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

    info!("Validating host configuration against system state");
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
            return Err(TridentError::new(
                InvalidInputError::IncompatibleHostConfiguration,
            ));
        }
    }
    state
        .with_host_status(|s| s.reconcile_state = ReconcileState::UpdateInProgress(update_kind))?;

    info!("Running prepare");
    prepare(&mut modules, state, host_config)?;

    let new_root_path = Path::new(UPDATE_ROOT_PATH);

    let mounts = if let UpdateKind::AbUpdate = update_kind {
        info!("Preparing storage to mount new root");
        let mounts = initialize_new_root(state, host_config, new_root_path)?;

        info!("Running provision");
        provision(&mut modules, state, host_config, new_root_path)?;
        info!("Entering /mnt/newroot chroot");
        chroot::enter_update_chroot(new_root_path)
            .message("Failed to enter chroot")?
            .execute_and_exit(|| {
                info!("Running configure");
                configure(&mut modules, state, host_config)?;

                info!("Regenerating initrd");
                regenerate_initrd()
            })
            .message("Failed to execute in chroot")?;

        Some(mounts)
    } else {
        info!("Running configure");
        configure(&mut modules, state, host_config)?;

        info!("Regenerating initrd");
        regenerate_initrd()?;

        None
    };

    if let Some(sender) = sender {
        sender
            .send(Ok(HostStatusState {
                status: serde_yaml::to_string(state.host_status())
                    .structured(InternalError::SerializeHostStatus)?,
            }))
            .structured(InternalError::SendHostStatus)?;
        drop(sender);
    }

    match update_kind {
        UpdateKind::UpdateAndReboot | UpdateKind::AbUpdate => {
            let root_block_device_path = get_root_block_device_path(state.host_status())
                .structured(InternalError::GetRootBlockDevice)?;

            if !allowed_operations.contains(Operations::Transition) {
                info!("Transition not requested, skipping transition");
                if let Some(mounts) = mounts {
                    mount_root::unmount_new_root(mounts, new_root_path)?;
                }
                return Ok(());
            }

            info!("Closing datastore");
            state.close();
            info!("Performing transition");
            transition(new_root_path, &root_block_device_path, state.host_status())?;

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

/// Using the / mount point, figure out what should be used as a root block device.
fn get_root_block_device_path(host_status: &HostStatus) -> Option<PathBuf> {
    host_status
        .storage
        .mount_points
        .get(Path::new(constants::ROOT_MOUNT_POINT_PATH))
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
    get_ab_volume_block_device_id(host_status, block_device_id, active).and_then(
        |child_block_device_id| get_block_device(host_status, &child_block_device_id, active),
    )
}

/// Returns a block device id for a volume from the given AB Volume Pair. If
/// active is true it returns the active volume, and if active is false it
/// returns the update volume (i.e. the one that isn't active).
fn get_ab_volume_block_device_id(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
    active: bool,
) -> Option<BlockDeviceId> {
    if let Some(ab_update) = &host_status.storage.ab_update {
        let ab_volume = ab_update
            .volume_pairs
            .iter()
            .find(|v| v.0 == block_device_id);
        if let Some(v) = ab_volume {
            // TODO https://dev.azure.com/mariner-org/ECF/_workitems/edit/6808- should we support esp as part of abVolume?
            return get_ab_update_volume(host_status, active).map(|selection| match selection {
                AbVolumeSelection::VolumeA => v.1.volume_a_id.clone(),
                AbVolumeSelection::VolumeB => v.1.volume_b_id.clone(),
            });
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
    match &host_status.reconcile_state {
        ReconcileState::UpdateInProgress(UpdateKind::HotPatch)
        | ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate)
        | ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot) => {
            host_status.storage.ab_update.as_ref()?.active_volume
        }
        ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
            if active {
                host_status.storage.ab_update.as_ref()?.active_volume
            } else {
                Some(
                    if host_status.storage.ab_update.as_ref()?.active_volume
                        == Some(AbVolumeSelection::VolumeA)
                    {
                        AbVolumeSelection::VolumeB
                    } else {
                        AbVolumeSelection::VolumeA
                    },
                )
            }
        }
        ReconcileState::UpdateInProgress(UpdateKind::Incompatible) => None,
        ReconcileState::Ready => {
            if active {
                host_status.storage.ab_update.as_ref()?.active_volume
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
) -> Result<(), TridentError> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.refresh_host_status(s).structured(ManagementError::from(
                ModuleError::RefreshHostStatus { name: m.name() },
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
) -> Result<(), TridentError> {
    for m in modules {
        m.validate_host_config(state.host_status(), host_config, planned_update)
            .structured(ManagementError::from(
                ModuleError::ValidateHostConfiguration { name: m.name() },
            ))?
    }
    info!("Host config validated");
    Ok(())
}

fn prepare(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.prepare(s, host_config)
                .structured(ManagementError::from(ModuleError::Prepare {
                    name: m.name(),
                }))
        })?;
    }
    Ok(())
}

fn provision(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
    new_root_path: &Path,
) -> Result<(), TridentError> {
    for module in modules {
        state.try_with_host_status(|host_status| {
            module
                .provision(host_status, host_config, new_root_path)
                .structured(ManagementError::from(ModuleError::Provision {
                    name: module.name(),
                }))
        })?
    }

    Ok(())
}

fn initialize_new_root(
    state: &mut DataStore,
    host_config: &HostConfiguration,
    new_root_path: &Path,
) -> Result<Vec<PathBuf>, TridentError> {
    state.try_with_host_status(|host_status| {
        storage::initialize_block_devices(host_status, host_config, new_root_path)
    })?;
    let mounts = state.try_with_host_status(|host_status| {
        mount_root::mount_new_root(host_status, new_root_path)
    })?;
    Ok(mounts)
}

fn configure(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.configure(s, host_config)
                .structured(ManagementError::from(ModuleError::Configure {
                    name: m.name(),
                }))
        })?;
    }

    Ok(())
}

/// Regenerates the initrd for the host, using host-specific configuration.
fn regenerate_initrd() -> Result<(), TridentError> {
    // We could autodetect configurations on the fly, but for more predictable
    // behavior and speedier subsequent boots, we will regenerate the host-specific initrd
    // here.

    // At the moment, this is needed for RAID, encryption, adding a root
    // password into initrd and to update the hardcoded UUID of the ESP.

    mkinitrd::execute()
}

pub fn reboot() -> Result<(), TridentError> {
    // Sync all writes to the filesystem.
    nix::unistd::sync();

    info!("Rebooting system");
    Command::new("systemctl")
        .env("SYSTEMD_IGNORE_CHROOT", "true")
        .arg("reboot")
        .run_and_check()
        .structured(ManagementError::Reboot)?;

    thread::sleep(Duration::from_secs(600));

    error!("Waited for reboot for 10 minutes, but nothing happened, aborting");
    Err(TridentError::new(ManagementError::RebootTimeout))
}

fn transition(
    _mount_path: &Path,
    _root_block_device_path: &Path,
    host_status: &HostStatus,
) -> Result<(), TridentError> {
    // let root_block_device_path = root_block_device_path
    //     .to_str()
    //     .structured(ManagementError::SetKernelCmdline)
    //     .message(format!(
    //         "Failed to convert root device path {:?} to string",
    //         root_block_device_path
    //     ))?;
    info!("Setting boot entries");

    // TODO - https://dev.azure.com/mariner-org/ECF/_workitems/edit/6807/ delete boot entries
    // TODO - set_boot_entries only if ABUpdate is in state AbUpdateStaged/ CleanInstall
    // TASK - https://dev.azure.com/mariner-org/ECF/_workitems/edit/6625
    bootentries::call_set_boot_next_and_update_hs(host_status)?;
    //TODO - update ABUpdate state to AbUpdateFinalized
    // TASK - https://dev.azure.com/mariner-org/ECF/_workitems/edit/6625

    // TODO(6721): Re-enable kexec
    // TODO - update ABUpdate state to AbUpdateFinalized
    // TASK - https://dev.azure.com/mariner-org/ECF/_workitems/edit/6625
    // info!("Performing soft reboot");
    // storage::image::kexec(
    //     mount_path,
    //     &format!("console=tty1 console=ttyS0 root={root_block_device_path}"),
    // )
    // .structured(ManagementError::Kexec)

    info!("Performing reboot");
    reboot()
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use maplit::btreemap;
    use uuid::Uuid;

    use trident_api::{
        config::PartitionType,
        constants,
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
                    PathBuf::from(constants::ROOT_MOUNT_POINT_PATH) => MountPoint {
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

        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"osab".to_owned(), false),
            Some("rootb".to_owned())
        );

        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"osb".to_owned(), false),
            None
        );

        assert_eq!(
            get_ab_volume(&host_status, &"osab".to_owned(), false),
            Some(BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                size: 9000,
                contents: BlockDeviceContents::Unknown,
            })
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
}
