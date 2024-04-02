use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    process::Command,
    sync::Mutex,
    thread,
    time::Duration,
};

use anyhow::Error;
use log::{debug, error, info};

use trident_api::{
    config::{HostConfiguration, Operations},
    constants::{self, ROOT_MOUNT_POINT_PATH, UPDATE_ROOT_FALLBACK_PATH, UPDATE_ROOT_PATH},
    error::{
        DatastoreError, InitializationError, InternalError, InvalidInputError, ManagementError,
        ModuleError, ReportError, TridentError, TridentResultExt,
    },
    status::{AbVolumeSelection, BlockDeviceInfo, HostStatus, ReconcileState, UpdateKind},
    BlockDeviceId,
};

use osutils::{chroot, container, exe::RunAndCheck, mkinitrd, mount};

use crate::{
    datastore::DataStore,
    modules::{
        boot::BootModule, hooks::HooksModule, management::ManagementModule, network::NetworkModule,
        osconfig::OsConfigModule, storage::StorageModule,
    },
    protobufs::HostStatusState,
    HostUpdateCommand, TRIDENT_DATASTORE_REF_PATH,
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
mod etc_overlay;
mod kexec;
mod mount_root;

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

    fn writable_etc_overlay(&self) -> bool {
        true
    }

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
    fn prepare(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
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

pub(super) fn clean_install(
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
    prepare(&mut modules, state)?;

    info!("Preparing storage to mount new root");
    let (new_root_path, mounts) = initialize_new_root(state, host_config)?;

    info!("Running provision");
    provision(&mut modules, state, host_config, &new_root_path)?;

    let datastore_ref = File::create(TRIDENT_DATASTORE_REF_PATH).structured(
        ManagementError::from(DatastoreError::CreateDatastoreRefFile),
    )?;
    let datastore_path = state
        .host_status()
        .trident
        .datastore_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("/tmp/datastore.sqlite"));

    info!("Entering '{}' chroot", new_root_path.display());
    let chroot = chroot::enter_update_chroot(&new_root_path).message("Failed to enter chroot")?;
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

            // If verity is present, it means that we are currently doing root
            // verity. For now, we can assume that /etc is readonly, so we setup
            // a writable overlay for it.
            let use_overlay = !host_config.storage.verity.is_empty();

            info!("Running configure");
            configure(&mut modules, state, host_config, use_overlay)?;

            regenerate_initrd(use_overlay)?;

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

    info!("Root device path: {:#?}", root_device_path);
    if !allowed_operations.contains(Operations::Transition) {
        info!("Transition not requested, skipping transition");
        info!("Unmounting '{}'", new_root_path.display());
        mount_root::unmount_new_root(mounts, &new_root_path)?;
    } else {
        info!("Performing transition");
        transition(&new_root_path, &root_device_path, state.host_status())?;
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

    info!("Selected update kind: {:?}", update_kind);
    let reconcile_state = ReconcileState::UpdateInProgress(update_kind);

    info!("Validating host configuration against system state");
    validate_host_config(&modules, state, host_config, reconcile_state)?;

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
    state.with_host_status(|s| {
        s.reconcile_state = reconcile_state;
        s.spec = host_config.clone();
    })?;

    info!("Running prepare");
    prepare(&mut modules, state)?;

    let (new_root_path, mounts) = if let UpdateKind::AbUpdate = update_kind {
        info!("Preparing storage to mount new root");
        let (new_root_path, mounts) = initialize_new_root(state, host_config)?;

        info!("Running provision");
        provision(&mut modules, state, host_config, &new_root_path)?;

        // If verity is present, it means that we are currently doing root
        // verity. For now, we can assume that /etc is readonly, so we setup
        // a writable overlay for it.
        let use_overlay = !host_config.storage.verity.is_empty();

        info!("Entering '{}' chroot", new_root_path.display());
        chroot::enter_update_chroot(&new_root_path)
            .message("Failed to enter chroot")?
            .execute_and_exit(|| {
                info!("Running configure");
                configure(&mut modules, state, host_config, use_overlay)?;

                regenerate_initrd(use_overlay)
            })
            .message("Failed to execute in chroot")?;

        (new_root_path, Some(mounts))
    } else {
        info!("Running configure");
        configure(&mut modules, state, host_config, false)?;

        regenerate_initrd(false)?;

        (PathBuf::from(ROOT_MOUNT_POINT_PATH), None)
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
                    mount_root::unmount_new_root(mounts, &new_root_path)?;
                }
                return Ok(());
            }

            info!("Closing datastore");
            state.close();
            info!("Performing transition");
            transition(&new_root_path, &root_block_device_path, state.host_status())?;

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
        .spec
        .storage
        .path_to_mount_point(Path::new(constants::ROOT_MOUNT_POINT_PATH))
        .and_then(|m| Some(get_block_device(host_status, &m.target_id, false)?.path))
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
    host_status
        .storage
        .block_devices
        .get(block_device_id)
        .cloned()
        .or_else(|| get_ab_volume(host_status, block_device_id, active))
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
    if let Some(ab_update) = &host_status.spec.storage.ab_update {
        let ab_volume = ab_update
            .volume_pairs
            .iter()
            .find(|v| &v.id == block_device_id);
        if let Some(v) = ab_volume {
            // TODO https://dev.azure.com/mariner-org/ECF/_workitems/edit/6808- should we support esp as part of abVolume?
            return get_ab_update_volume(host_status, active).map(|selection| match selection {
                AbVolumeSelection::VolumeA => v.volume_a_id.clone(),
                AbVolumeSelection::VolumeB => v.volume_b_id.clone(),
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
            host_status.storage.ab_active_volume
        }
        ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
            if active {
                host_status.storage.ab_active_volume
            } else {
                Some(
                    if host_status.storage.ab_active_volume == Some(AbVolumeSelection::VolumeA) {
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
                host_status.storage.ab_active_volume
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
    for module in modules {
        debug!("Starting stage 'Refresh' for module '{}'", module.name());
        state.try_with_host_status(|s| {
            module
                .refresh_host_status(s)
                .structured(ManagementError::from(ModuleError::RefreshHostStatus {
                    name: module.name(),
                }))
        })?;
        debug!("Finished stage 'Refresh' for module '{}'", module.name());
    }
    Ok(())
}

fn validate_host_config(
    modules: &[Box<dyn Module>],
    state: &DataStore,
    host_config: &HostConfiguration,
    planned_update: ReconcileState,
) -> Result<(), TridentError> {
    for module in modules {
        debug!("Starting stage 'Validate' for module '{}'", module.name());
        module
            .validate_host_config(state.host_status(), host_config, planned_update)
            .structured(ManagementError::from(
                ModuleError::ValidateHostConfiguration {
                    name: module.name(),
                },
            ))?;
        debug!("Finished stage 'Validate' for module '{}'", module.name());
    }
    info!("Host config validated");
    Ok(())
}

fn prepare(modules: &mut [Box<dyn Module>], state: &mut DataStore) -> Result<(), TridentError> {
    for module in modules {
        debug!("Starting stage 'Prepare' for module '{}'", module.name());
        state.try_with_host_status(|s| {
            module
                .prepare(s)
                .structured(ManagementError::from(ModuleError::Prepare {
                    name: module.name(),
                }))
        })?;
        debug!("Finished stage 'Prepare' for module '{}'", module.name());
    }
    Ok(())
}

fn provision(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
    new_root_path: &Path,
) -> Result<(), TridentError> {
    // If verity is present, it means that we are currently doing root
    // verity. For now, we can assume that /etc is readonly, so we setup
    // a writable overlay for it.
    let use_overlay = !host_config.storage.verity.is_empty();

    for module in modules {
        debug!("Starting stage 'Provision' for module '{}'", module.name());
        let _etc_overlay_mount = if use_overlay {
            Some(etc_overlay::create(
                Path::new(new_root_path),
                module.writable_etc_overlay(),
            )?)
        } else {
            None
        };
        state.try_with_host_status(|host_status| {
            module
                .provision(host_status, host_config, new_root_path)
                .structured(ManagementError::from(ModuleError::Provision {
                    name: module.name(),
                }))
        })?;
        debug!("Finished stage 'Provision' for module '{}'", module.name());
    }

    Ok(())
}

fn initialize_new_root(
    state: &mut DataStore,
    host_config: &HostConfiguration,
) -> Result<(PathBuf, Vec<PathBuf>), TridentError> {
    let mut new_root_path = Path::new(UPDATE_ROOT_PATH);
    if mount::ensure_mount_directory(new_root_path).is_err() {
        new_root_path = Path::new(UPDATE_ROOT_FALLBACK_PATH);
    }

    state.try_with_host_status(|host_status| {
        storage::initialize_block_devices(host_status, host_config, new_root_path)
    })?;
    let mounts = state.try_with_host_status(|host_status| {
        mount_root::mount_new_root(host_status, new_root_path)
    })?;
    Ok((new_root_path.to_owned(), mounts))
}

fn configure(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
    use_overlay: bool,
) -> Result<(), TridentError> {
    for module in modules {
        debug!("Starting stage 'Configure' for module '{}'", module.name());
        // unmount on drop
        let _etc_overlay_mount = if use_overlay {
            Some(etc_overlay::create(
                Path::new("/"),
                module.writable_etc_overlay(),
            )?)
        } else {
            None
        };
        state.try_with_host_status(|s| {
            module
                .configure(s, host_config)
                .structured(ManagementError::from(ModuleError::Configure {
                    name: module.name(),
                }))
        })?;
        debug!("Finished stage 'Configure' for module '{}'", module.name());
    }

    Ok(())
}

/// Regenerates the initrd for the host, using host-specific configuration.
fn regenerate_initrd(use_overlay: bool) -> Result<(), TridentError> {
    // We could autodetect configurations on the fly, but for more predictable
    // behavior and speedier subsequent boots, we will regenerate the host-specific initrd
    // here.

    // At the moment, this is needed for RAID, encryption, adding a root
    // password into initrd and to update the hardcoded UUID of the ESP.

    let _etc_overlay_mount = if use_overlay {
        Some(etc_overlay::create(Path::new("/"), false)?)
    } else {
        None
    };

    info!("Regenerating initrd");
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
    new_root_path: &Path,
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
    bootentries::call_set_boot_next_and_update_hs(host_status, new_root_path)?;
    //TODO - update ABUpdate state to AbUpdateFinalized
    // TASK - https://dev.azure.com/mariner-org/ECF/_workitems/edit/6625

    // TODO(6721): Re-enable kexec
    // TODO - update ABUpdate state to AbUpdateFinalized
    // TASK - https://dev.azure.com/mariner-org/ECF/_workitems/edit/6625
    // info!("Performing soft reboot");
    // storage::image::kexec(
    //     new_root_path,
    //     &format!("console=tty1 console=ttyS0 root={root_block_device_path}"),
    // )
    // .structured(ManagementError::Kexec)

    info!("Performing reboot");
    reboot()
}

#[cfg(test)]
mod test {

    use maplit::btreemap;

    use trident_api::{
        config::{self, AbUpdate, AbVolumePair, Disk, Partition, PartitionSize, PartitionType},
        constants,
        status::{BlockDeviceContents, Storage},
    };

    use super::*;

    #[test]
    fn test_get_root_block_device_path() {
        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "foo".to_owned(),
                        device: PathBuf::from("/dev/sda"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_owned(),
                                size: PartitionSize::Fixed(2),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root".to_owned(),
                                size: PartitionSize::Fixed(7),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        config::MountPoint {
                            target_id: "boot".to_owned(),
                            filesystem: "fat32".to_owned(),
                            options: vec![],
                            path: PathBuf::from("/boot"),
                        },
                        config::MountPoint {
                            target_id: "root".to_owned(),
                            filesystem: "ext4".to_owned(),
                            options: vec![],
                            path: PathBuf::from(constants::ROOT_MOUNT_POINT_PATH),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "foo".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda"),
                        size: 10,
                        contents: BlockDeviceContents::Initialized,
                    },
                    "boot".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda1"),
                        size: 2,
                        contents: BlockDeviceContents::Initialized,
                    },
                    "root".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda2"),
                        size: 6,
                        contents: BlockDeviceContents::Initialized,
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
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![
                        Disk {
                            id: "os".to_owned(),
                            device: PathBuf::from("/dev/disk/by-bus/foobar"),
                            partitions: vec![
                                Partition {
                                    id: "efi".to_owned(),
                                    size: PartitionSize::Fixed(100),
                                    partition_type: PartitionType::Esp,
                                },
                                Partition {
                                    id: "root".to_owned(),
                                    size: PartitionSize::Fixed(900),
                                    partition_type: PartitionType::Root,
                                },
                                Partition {
                                    id: "rootb".to_owned(),
                                    size: PartitionSize::Fixed(9000),
                                    partition_type: PartitionType::Root,
                                },
                            ],
                            ..Default::default()
                        },
                        Disk {
                            id: "data".to_owned(),
                            device: PathBuf::from("/dev/disk/by-bus/foobar"),
                            partitions: vec![],
                            ..Default::default()
                        },
                    ],
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "osab".to_string(),
                            volume_a_id: "root".to_string(),
                            volume_b_id: "rootb".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "efi".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 900,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "rootb".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 9000,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "data".to_owned() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 1000,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
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
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
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
            spec: HostConfiguration {
                storage: config::Storage {
                    ab_update: Some(AbUpdate {
                        volume_pairs: Vec::new(),
                    }),
                    ..Default::default()
                },
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

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);

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
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
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
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
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
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_ab_update_volume(&host_status, true),
            Some(AbVolumeSelection::VolumeB)
        );
    }
}
