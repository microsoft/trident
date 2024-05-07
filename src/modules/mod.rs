#[cfg(feature = "grpc-dangerous")]
use crate::grpc;
use std::{
    fs::{self},
    path::{Path, PathBuf, MAIN_SEPARATOR},
    process::Command,
    sync::{Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};
#[cfg(feature = "grpc-dangerous")]
use tokio::sync::mpsc;

use anyhow::Error;
use log::{debug, error, info};

use sys_mount::{Mount, MountFlags};
use trident_api::{
    config::{HostConfiguration, Operations},
    constants::{
        self, EXEC_ROOT_PATH, ROOT_MOUNT_POINT_PATH, UPDATE_ROOT_FALLBACK_PATH, UPDATE_ROOT_PATH,
    },
    error::{
        InitializationError, InternalError, InvalidInputError, ManagementError, ModuleError,
        ReportError, TridentError, TridentResultExt,
    },
    status::{AbVolumeSelection, BlockDeviceInfo, HostStatus, ServicingState, ServicingType},
    BlockDeviceId,
};

use osutils::{
    chroot, container,
    exe::RunAndCheck,
    mkinitrd, mount,
    path::{self, join_relative},
};

use crate::{
    datastore::DataStore,
    modules::{
        boot::BootModule, hooks::HooksModule, management::ManagementModule, network::NetworkModule,
        osconfig::OsConfigModule, storage::StorageModule,
    },
    HostUpdateCommand, TRIDENT_DATASTORE_PATH,
};

#[cfg(feature = "grpc-dangerous")]
use crate::grpc::protobufs::HostStatusState;

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
    fn refresh_host_status(
        &mut self,
        _host_status: &mut HostStatus,
        _clean_install: bool,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Select the servicing type based on the host status and host config.
    fn select_servicing_type(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<ServicingType> {
        None
    }

    /// Validate the host config.
    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
        _planned_servicing_type: ServicingType,
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
        _exec_root: &Path,
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
        #[cfg(feature = "grpc-dangerous")]
        mut sender,
    } = command;

    {
        // TODO: Currently, we're assuming we are either in servicing state NotProvisioned,
        // servicing type None OR servicing state CleanInstallFailed, servicing type None at this
        // point. In the follow up PR, we can also be in servicing type CleanInstall, servicing
        // state DeploymentStaged here.

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

    info!("Starting clean_install");
    let clean_install_start_time = Instant::now();
    let mut modules = MODULES.lock().unwrap();

    info!("Refreshing host status");
    refresh_host_status(&mut modules, state, true)?;

    info!("Validating host configuration against system state");
    // Since we're in clean_install(), the only possible servicing type is CleanInstall.
    validate_host_config(&modules, state, host_config, ServicingType::CleanInstall)?;

    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(&mut sender, state)?;

    // TODO: If StageDeployment is not requested but we're in DeploymentStaged AND allowed
    // operations include FinalizeDeployment, finalize deployment and reboot.
    if !allowed_operations.contains(Operations::StageDeployment) {
        info!("Staging of clean install not requested, skipping staging");
        return Ok(());
    }

    // Otherwise, stage clean install
    let (root_device_path, new_root_path, mounts) = stage_clean_install(
        &mut modules,
        state,
        host_config,
        #[cfg(feature = "grpc-dangerous")]
        &mut sender,
    )?;

    // Switch datastore back to the old path
    state.switch_datastore_to_path(Path::new(ROOT_MOUNT_POINT_PATH))?;

    // If FinalizeDeployment is not requested, close the datastore and unmount the new root
    if !allowed_operations.contains(Operations::FinalizeDeployment) {
        info!("Finalizing of clean install not requested, skipping finalizing and reboot");
        state.close();

        info!("Unmounting '{}'", new_root_path.display());
        mount_root::unmount_new_root(mounts, &new_root_path)?;
    } else {
        finalize_clean_install(
            state,
            &root_device_path,
            &new_root_path,
            clean_install_start_time,
            #[cfg(feature = "grpc-dangerous")]
            &mut sender,
        )?;
    }

    Ok(())
}

/// Stages a clean install. Takes in 4 arguments:
/// - modules: A mutable reference to the list of modules.
/// - state: A mutable reference to the DataStore.
/// - host_config: A reference to the HostConfiguration.
/// - sender: A mutable reference to the gRPC sender.
///
/// On success, returns a tuple with 3 elements:
/// - The current root device path.
/// - The new root device path.
/// - A vector of paths to custom mounts for the new root.
fn stage_clean_install(
    modules: &mut MutexGuard<Vec<Box<dyn Module>>>,
    state: &mut DataStore,
    host_config: &HostConfiguration,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(PathBuf, PathBuf, Vec<PathBuf>), TridentError> {
    debug!("Setting host's servicing type to CleanInstall");
    debug!("Updating host's servicing state to StagingDeployment");
    state.with_host_status(|host_status| {
        host_status.servicing_type = Some(ServicingType::CleanInstall);
        host_status.servicing_state = ServicingState::StagingDeployment;
    })?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    info!("Running prepare");
    prepare(modules, state)?;

    info!("Preparing storage to mount new root");
    let (new_root_path, exec_root_path, mounts) = initialize_new_root(state, host_config)?;

    info!("Running provision");
    provision(modules, state, host_config, &new_root_path)?;

    info!("Entering '{}' chroot", new_root_path.display());
    let chroot = chroot::enter_update_chroot(&new_root_path).message("Failed to enter chroot")?;
    let mut root_device_path = None;

    chroot
        .execute_and_exit(|| {
            info!("Entered chroot");
            state.switch_datastore_to_path(&exec_root_path)?;

            // If verity is present, it means that we are currently doing root
            // verity. For now, we can assume that /etc is readonly, so we setup
            // a writable overlay for it.
            let use_overlay = !host_config.storage.internal_verity.is_empty();

            info!("Running configure");
            configure(modules, state, host_config, &exec_root_path, use_overlay)?;

            regenerate_initrd(use_overlay)?;

            root_device_path = Some(
                get_root_block_device_path(state.host_status())
                    .structured(InternalError::GetRootBlockDevice)?,
            );

            // At this point, clean install has been staged, so update servicing state
            debug!("Updating host's servicing state to DeploymentStaged");
            state.with_host_status(|host_status| {
                host_status.servicing_state = ServicingState::DeploymentStaged
            })?;
            #[cfg(feature = "grpc-dangerous")]
            send_host_status_state(sender, state)?;

            Ok(())
        })
        .message("Failed to execute in chroot")?;

    let root_device_path =
        root_device_path.structured(InternalError::Internal("Failed to get root block device"))?;
    debug!("Root device path: {:#?}", root_device_path);

    // Return the current root device path, new root device path, and the list of mounts
    Ok((root_device_path, new_root_path, mounts))
}

/// Finalizes a clean install. Takes in 5 arguments:
/// - state: A mutable reference to the DataStore.
/// - root_device_path: Current root device path.
/// - new_root_path: New root device path.
/// - clean_install_start_time: Instant when clean install started.
/// - sender: A mutable reference to the gRPC sender.
fn finalize_clean_install(
    state: &mut DataStore,
    root_device_path: &Path,
    new_root_path: &Path,
    clean_install_start_time: Instant,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(), TridentError> {
    debug!("Updating host's servicing state to FinalizingDeployment");
    state.with_host_status(|host_status| {
        host_status.servicing_state = ServicingState::FinalizingDeployment
    })?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    // Persist and close the datastore
    let datastore_path = state
        .host_status()
        .trident
        .datastore_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("/tmp/datastore.sqlite"));
    state.persist(&path::join_relative(new_root_path, datastore_path))?;
    state.close();

    info!("Finalizing clean install");
    state.try_with_host_status(|host_status| finalize_deployment(host_status, new_root_path))?;

    // Metric for clean install provisioning time in seconds
    tracing::info!(
        metric_name = "clean_install_provisioning_secs",
        value = clean_install_start_time.elapsed().as_secs_f64()
    );

    state.try_with_host_status(|host_status| perform_reboot(root_device_path, host_status))?;

    Ok(())
}

pub(super) fn update(
    command: HostUpdateCommand,
    state: &mut DataStore,
) -> Result<(), TridentError> {
    let HostUpdateCommand {
        ref host_config,
        allowed_operations,
        #[cfg(feature = "grpc-dangerous")]
        mut sender,
    } = command;

    let mut modules = MODULES.lock().unwrap();

    // TODO: Currently, we're assuming we're in servicing state Provisioned, servicing type None OR
    // servicing state AbUpdateFailed, servicing type AbUpdate at this point. In the follow up PR,
    // we can also be in servicing state DeploymentStaged, servicing type AbUpdate here.

    info!("Refreshing host status");
    refresh_host_status(&mut modules, state, false)?;

    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(&mut sender, state)?;

    info!("Determining servicing type");
    let servicing_type = modules
        .iter()
        .filter_map(|m| {
            let servicing_type = m.select_servicing_type(state.host_status(), host_config);
            if let Some(servicing_type) = servicing_type {
                info!(
                    "Module '{}' selected servicing type: {:?}",
                    m.name(),
                    servicing_type
                );
            }
            servicing_type
        })
        .max();
    let Some(servicing_type) = servicing_type else {
        info!("No updates required");
        return Ok(());
    };

    info!(
        "Selected servicing type for the required update: {:?}",
        servicing_type
    );

    info!("Validating host configuration against system state");
    validate_host_config(&modules, state, host_config, servicing_type)?;

    // TODO: In the follow up PR, we'll check if current servicing state is DeploymentStaged,
    // servicing type is AbUpdate, and allowed operations include FinalizeDeployment. If that's the
    // case, finalize the A/B update and reboot.
    if !allowed_operations.contains(Operations::StageDeployment) {
        info!("Staging of update not requested, skipping staging");
        return Ok(());
    }

    match servicing_type {
        ServicingType::HotPatch => info!("Performing hot patch update"),
        ServicingType::NormalUpdate => info!("Performing normal update"),
        ServicingType::UpdateAndReboot => info!("Performing update and reboot"),
        ServicingType::AbUpdate => info!("Performing A/B update"),
        ServicingType::Incompatible => {
            return Err(TridentError::new(
                InvalidInputError::IncompatibleHostConfiguration,
            ));
        }
        ServicingType::CleanInstall => {
            return Err(TridentError::new(
                InvalidInputError::CleanInstallRequestedForProvisionedHost,
            ));
        }
    }

    // Set servicing type to the selected servicing type; servicing state to StagingDeployment.
    // Update spec by copying the current host config.
    debug!("Setting host's servicing type to {:?}", servicing_type);
    debug!("Updating host's servicing state to StagingDeployment");
    state.with_host_status(|host_status| {
        host_status.servicing_type = Some(servicing_type);
        host_status.servicing_state = ServicingState::StagingDeployment;
        host_status.spec = host_config.clone();
    })?;

    info!("Running prepare");
    prepare(&mut modules, state)?;

    let (new_root_path, mounts) = if let ServicingType::AbUpdate = servicing_type {
        info!("Preparing storage to mount new root");
        let (new_root_path, exec_root_path, mounts) = initialize_new_root(state, host_config)?;

        info!("Running provision");
        provision(&mut modules, state, host_config, &new_root_path)?;

        // If verity is present, it means that we are currently doing root
        // verity. For now, we can assume that /etc is readonly, so we setup
        // a writable overlay for it.
        let use_overlay = !host_config.storage.internal_verity.is_empty();

        info!("Entering '{}' chroot", new_root_path.display());
        chroot::enter_update_chroot(&new_root_path)
            .message("Failed to enter chroot")?
            .execute_and_exit(|| {
                info!("Running configure");
                configure(
                    &mut modules,
                    state,
                    host_config,
                    &exec_root_path,
                    use_overlay,
                )?;

                regenerate_initrd(use_overlay)
            })
            .message("Failed to execute in chroot")?;

        // At this point, deployment has been staged, so update servicing state
        debug!("Updating host's servicing state to DeploymentStaged");
        state.with_host_status(|host_status| {
            host_status.servicing_state = ServicingState::DeploymentStaged
        })?;

        (new_root_path, Some(mounts))
    } else {
        info!("Running configure");
        configure(
            &mut modules,
            state,
            host_config,
            Path::new(ROOT_MOUNT_POINT_PATH),
            false,
        )?;

        regenerate_initrd(false)?;

        (PathBuf::from(ROOT_MOUNT_POINT_PATH), None)
    };

    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(&mut sender, state)?;

    match servicing_type {
        ServicingType::UpdateAndReboot | ServicingType::AbUpdate => {
            let root_block_device_path = get_root_block_device_path(state.host_status())
                .structured(InternalError::GetRootBlockDevice)?;

            if !allowed_operations.contains(Operations::FinalizeDeployment) {
                info!("Finalizing of update not requested, skipping reboot");
                if let Some(mounts) = mounts {
                    mount_root::unmount_new_root(mounts, &new_root_path)?;
                }
                return Ok(());
            }

            // Otherwise, finalize deployment
            debug!("Updating host's servicing state to FinalizingDeployment");
            state.with_host_status(|host_status| {
                host_status.servicing_state = ServicingState::FinalizingDeployment
            })?;
            #[cfg(feature = "grpc-dangerous")]
            send_host_status_state(&mut sender, state)?;

            info!("Closing datastore");
            state.close();

            info!("Finalizing update");
            state.try_with_host_status(|host_status| {
                finalize_deployment(host_status, &new_root_path)
            })?;

            state.try_with_host_status(|host_status| {
                perform_reboot(&root_block_device_path, host_status)
            })?;

            Ok(())
        }
        ServicingType::NormalUpdate | ServicingType::HotPatch => {
            state.with_host_status(|host_status| {
                host_status.servicing_type = None;
                host_status.servicing_state = ServicingState::Provisioned;
            })?;
            info!("Update complete");
            Ok(())
        }
        ServicingType::Incompatible | ServicingType::CleanInstall => {
            unreachable!()
        }
    }
}

#[cfg(feature = "grpc-dangerous")]
fn send_host_status_state(
    sender: &mut Option<mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>>,
    state: &DataStore,
) -> Result<(), TridentError> {
    if let Some(ref mut sender) = sender {
        sender
            .send(Ok(HostStatusState {
                status: serde_yaml::to_string(state.host_status())
                    .structured(InternalError::SerializeHostStatus)?,
            }))
            .structured(InternalError::SendHostStatus)?;
    }
    Ok(())
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
/// `block_device_id`. If the volume is part of an A/B Volume Pair and active is
/// true it returns the active volume, and if active is false it returns the
/// update volume (i.e. the one that isn't active).
pub(super) fn get_block_device(
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

/// Returns a block device info for a volume from the given A/B Volume Pair. If
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

/// Returns a block device id for a volume from the given A/B Volume Pair. If
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
            // Determine which func to use based on 'active' flag
            let selection = if active {
                get_ab_active_volume(host_status)
            } else {
                get_ab_update_volume(host_status)
            };
            // Return the appropriate BlockDeviceId based on the selection
            return selection.map(|sel| match sel {
                AbVolumeSelection::VolumeA => v.volume_a_id.clone(),
                AbVolumeSelection::VolumeB => v.volume_b_id.clone(),
            });
        };
    }
    None
}

/// Returns the update volume selection for all A/B volume pairs. The update volume is the one that
/// is meant to be updated, based on the ongoing servicing type and state.
fn get_ab_update_volume(host_status: &HostStatus) -> Option<AbVolumeSelection> {
    match &host_status.servicing_state {
        // If host is in NotProvisioned, CleanInstallFailed, Provisioned, or AbUpdateFailed,
        // update volume is None, since Trident is not executing any servicing
        ServicingState::NotProvisioned
        | ServicingState::CleanInstallFailed
        | ServicingState::Provisioned
        | ServicingState::AbUpdateFailed => None,
        // If host is in any different servicing state, determine based on servicing type
        ServicingState::StagingDeployment
        | ServicingState::DeploymentStaged
        | ServicingState::FinalizingDeployment
        | ServicingState::DeploymentFinalized => {
            match host_status.servicing_type {
                Some(ServicingType::HotPatch)
                | Some(ServicingType::NormalUpdate)
                | Some(ServicingType::UpdateAndReboot) => host_status.storage.ab_active_volume,
                // If host executing A/B update, update volume is the opposite of active volume
                // as specified in the storage status
                Some(ServicingType::AbUpdate) => {
                    if host_status.storage.ab_active_volume == Some(AbVolumeSelection::VolumeA) {
                        Some(AbVolumeSelection::VolumeB)
                    } else {
                        Some(AbVolumeSelection::VolumeA)
                    }
                }
                // If host is executing a clean install, update volume is always A
                Some(ServicingType::CleanInstall) => Some(AbVolumeSelection::VolumeA),
                // In host status, servicing type will never be set to Incompatible OR be None if
                // servicing state is one of the above.
                Some(ServicingType::Incompatible) | None => None,
            }
        }
    }
}

/// Returns the active volume selection for all A/B volume pairs. The active volume is the one that
/// the host is currently running from.
fn get_ab_active_volume(host_status: &HostStatus) -> Option<AbVolumeSelection> {
    match host_status.servicing_state {
        // If host is in NotProvisioned or CleanInstallFailed, there is no active volume, as
        // we're still booted from the provisioning OS
        ServicingState::NotProvisioned | ServicingState::CleanInstallFailed => None,
        // If host is in Provisioned OR AbUpdateFailed, active volume is the current one
        ServicingState::Provisioned | ServicingState::AbUpdateFailed => {
            host_status.storage.ab_active_volume
        }
        ServicingState::StagingDeployment
        | ServicingState::DeploymentStaged
        | ServicingState::FinalizingDeployment
        | ServicingState::DeploymentFinalized => {
            match host_status.servicing_type {
                // If host is executing a deployment of any type, active volume is in host status.
                Some(ServicingType::HotPatch)
                | Some(ServicingType::NormalUpdate)
                | Some(ServicingType::UpdateAndReboot)
                | Some(ServicingType::AbUpdate) => host_status.storage.ab_active_volume,
                // If host is executing a clean install, there is no active volume yet.
                Some(ServicingType::CleanInstall) => None,
                // In host status, servicing type will never be set to Incompatible OR be None if
                // servicing state is one of the above.
                Some(ServicingType::Incompatible) | None => unreachable!(),
            }
        }
    }
}

fn refresh_host_status(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    clean_install: bool,
) -> Result<(), TridentError> {
    for module in modules {
        debug!("Starting stage 'Refresh' for module '{}'", module.name());
        state.try_with_host_status(|s| {
            module
                .refresh_host_status(s, clean_install)
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
    planned_servicing_type: ServicingType,
) -> Result<(), TridentError> {
    for module in modules {
        debug!("Starting stage 'Validate' for module '{}'", module.name());
        module
            .validate_host_config(state.host_status(), host_config, planned_servicing_type)
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
    let use_overlay = !host_config.storage.internal_verity.is_empty();

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
) -> Result<(PathBuf, PathBuf, Vec<PathBuf>), TridentError> {
    let mut new_root_path = Path::new(UPDATE_ROOT_PATH);
    if mount::ensure_mount_directory(new_root_path).is_err() {
        new_root_path = Path::new(UPDATE_ROOT_FALLBACK_PATH);
    }

    state.try_with_host_status(|host_status| {
        storage::initialize_block_devices(host_status, host_config, new_root_path)
    })?;
    let mut mounts = state.try_with_host_status(|host_status| {
        mount_root::mount_new_root(host_status, new_root_path)
    })?;

    let tmp_mount = Mount::builder()
        .fstype("tmpfs")
        .flags(MountFlags::empty())
        .mount("tmpfs", new_root_path.join("tmp"))
        .structured(ManagementError::ChrootMountSpecial { dir: "/tmp" })?;
    // Insert tmp_mount at the end of the mounts vector
    mounts.push(tmp_mount.target_path().to_owned());

    let exec_root_path = Path::new(EXEC_ROOT_PATH);
    let full_exec_root_path = join_relative(new_root_path, exec_root_path);
    std::fs::create_dir_all(&full_exec_root_path)
        .structured(ManagementError::CreateExecrootDirectory)?;
    mount::bind_mount(ROOT_MOUNT_POINT_PATH, &full_exec_root_path)
        .structured(ManagementError::MountExecroot)?;
    // Insert full_exec_root_path at the end of the mounts vector
    mounts.push(full_exec_root_path);

    let run_mount = Mount::builder()
        .fstype("tmpfs")
        .flags(MountFlags::empty())
        .mount("tmpfs", new_root_path.join("run"))
        .structured(ManagementError::ChrootMountSpecial { dir: "/run" })?;
    // Insert run_mount at the end of the mounts vector
    mounts.push(run_mount.target_path().to_owned());

    Ok((new_root_path.to_owned(), exec_root_path.to_owned(), mounts))
}

fn configure(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
    exec_root: &Path,
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
                .configure(s, host_config, exec_root)
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

/// Triggers a reboot. Currently, this defaults to firmware reboot.
fn perform_reboot(
    _root_block_device_path: &Path,
    _host_status: &HostStatus,
) -> Result<(), TridentError> {
    // TODO(6721): Re-enable kexec
    // let root_block_device_path = root_block_device_path
    //     .to_str()
    //     .structured(ManagementError::SetKernelCmdline)
    //     .message(format!(
    //         "Failed to convert root device path {:?} to string",
    //         root_block_device_path
    //     ))?;
    //
    // info!("Performing soft reboot");
    // storage::image::kexec(
    //     new_root_path,
    //     &format!("console=tty1 console=ttyS0 root={root_block_device_path}"),
    // )
    // .structured(ManagementError::Kexec)

    info!("Performing reboot");
    reboot()
}

/// Finalizes deployment by setting bootNext and updating host status. Changes host's servicing state
/// to DeploymentFinalized.
fn finalize_deployment(
    host_status: &mut HostStatus,
    new_root_path: &Path,
) -> Result<(), TridentError> {
    // TODO: Delete boot entries. Related ADO task:
    // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6807/

    info!("Re-opening datastore");
    let datastore_path = host_status
        .trident
        .datastore_path
        .as_deref()
        .unwrap_or(Path::new(TRIDENT_DATASTORE_PATH));

    let new_datastore_path = new_root_path.join(
        datastore_path
            .to_str()
            .unwrap()
            .trim_start_matches(MAIN_SEPARATOR),
    );
    debug!(
        "Opening datastore in finalize_deployment at path: {}",
        new_datastore_path.display()
    );
    let mut datastore = DataStore::open(&new_datastore_path)?;

    info!("Setting boot entries");
    datastore.try_with_host_status(|host_status| {
        bootentries::call_set_boot_next_and_update_hs(host_status, new_root_path)
    })?;

    debug!("Updating host's servicing state to DeploymentFinalized");
    datastore
        .with_host_status(|status| status.servicing_state = ServicingState::DeploymentFinalized)?;

    info!("Closing datastore");
    datastore.close();

    Ok(())
}

#[cfg(test)]
mod test {

    use maplit::btreemap;

    use trident_api::{
        config::{
            self, AbUpdate, AbVolumePair, Disk, FileSystemType, Partition, PartitionSize,
            PartitionType,
        },
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
                    internal_mount_points: vec![
                        config::InternalMountPoint {
                            target_id: "boot".to_owned(),
                            filesystem: FileSystemType::Vfat,
                            options: vec![],
                            path: PathBuf::from("/boot"),
                        },
                        config::InternalMountPoint {
                            target_id: "root".to_owned(),
                            filesystem: FileSystemType::Ext4,
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
            servicing_state: ServicingState::Provisioned,
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
            servicing_state: ServicingState::Provisioned,
            servicing_type: None,
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

        // If servicing state is Provisioned, get_block_device() should return the active volume
        // when active=true and None when active=false, for A/B volume pair.
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), true),
            None
        );
        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"osab".to_owned(), true),
            None
        );
        // Now, set ab_active_volume to VolumeA.
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), true).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"osab".to_owned(), true),
            Some("root".to_owned())
        );

        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), false),
            None
        );
        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"osab".to_owned(), false),
            None
        );

        // Now, set servicing type to AbUpdate; servicing state to Staging Deployment.
        host_status.servicing_type = Some(ServicingType::AbUpdate);
        host_status.servicing_state = ServicingState::StagingDeployment;
        // When active=true, should return VolumeA; when active=false, return VolumeB.
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), true).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"osab".to_owned(), true),
            Some("root".to_owned())
        );

        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                size: 9000,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"osab".to_owned(), false),
            Some("rootb".to_owned())
        );

        // When active volume is VolumeB, should return VolumeB when active=true; VolumeA when
        // active=false.
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), true).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                size: 9000,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"osab".to_owned(), true),
            Some("rootb".to_owned())
        );

        assert_eq!(
            super::get_block_device(&host_status, &"osab".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"osab".to_owned(), false),
            Some("root".to_owned())
        );

        // If target block device id does not exist, should return None.
        assert_eq!(
            get_ab_volume_block_device_id(&host_status, &"non-existent".to_owned(), false),
            None
        );
    }

    /// Validates logic in get_ab_update_volume() function
    #[test]
    fn test_get_ab_update_volume() {
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
            servicing_type: None,
            servicing_state: ServicingState::NotProvisioned,
            ..Default::default()
        };

        // 1. If host is in NotProvisioned, update volume is None b/c Trident is not executing any
        // servicing
        assert_eq!(get_ab_update_volume(&host_status), None);

        // 2. If host is in CleanInstallFailed, update volume is None b/c Trident is not executing any
        // servicing
        host_status.servicing_state = ServicingState::CleanInstallFailed;
        assert_eq!(get_ab_update_volume(&host_status), None);

        // 3. If host is in Provisioned, update volume is None b/c Trident is not executing any
        // servicing
        host_status.servicing_state = ServicingState::Provisioned;
        assert_eq!(get_ab_update_volume(&host_status), None);

        // 4. If host is in AbUpdateFailed, update volume is None b/c Trident is not executing any
        // servicing
        host_status.servicing_state = ServicingState::AbUpdateFailed;
        assert_eq!(get_ab_update_volume(&host_status), None);

        // 5. If host is doing CleanInstall, update volume is always A
        host_status.servicing_type = Some(ServicingType::CleanInstall);
        host_status.servicing_state = ServicingState::StagingDeployment;
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status.servicing_state = ServicingState::DeploymentStaged;
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status.servicing_state = ServicingState::FinalizingDeployment;
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status.servicing_state = ServicingState::DeploymentFinalized;
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        // 6. If host is doing HotPatch, NormalUpdate, or UpdateAndReboot, update volume is always
        // the currently active volume
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        host_status.servicing_state = ServicingState::StagingDeployment;
        host_status.servicing_type = Some(ServicingType::HotPatch);
        assert_eq!(
            get_ab_update_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::NormalUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::UpdateAndReboot);
        assert_eq!(
            get_ab_update_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        host_status.servicing_type = Some(ServicingType::HotPatch);
        assert_eq!(
            get_ab_update_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::NormalUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::UpdateAndReboot);
        assert_eq!(
            get_ab_update_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        // 7. If host is doing A/B update, update volume is the opposite of the active volume
        host_status.servicing_type = Some(ServicingType::AbUpdate);
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeB)
        );

        // If servicing state changes, the update volume should not change
        host_status.servicing_state = ServicingState::DeploymentStaged;
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeB)
        );

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );

        // If servicing state changes, the update volume should not change
        host_status.servicing_state = ServicingState::FinalizingDeployment;
        assert_eq!(
            get_ab_update_volume(&host_status),
            Some(AbVolumeSelection::VolumeA)
        );
    }

    /// Validates logic in get_ab_active_volume() function
    #[test]
    fn test_get_ab_active_volume() {
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
            servicing_type: None,
            servicing_state: ServicingState::NotProvisioned,
            ..Default::default()
        };

        // 1. If host is in NotProvisioned, there is no active volume, as we're still booted from
        // the provisioning OS
        assert_eq!(get_ab_active_volume(&host_status), None);

        // 2. If host is in CleanInstallFailed, there is no active volume either
        host_status.servicing_state = ServicingState::CleanInstallFailed;
        assert_eq!(get_ab_active_volume(&host_status), None);

        // 3. If host is in Provisioned, active volume is the current one
        host_status.servicing_state = ServicingState::Provisioned;
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        // 4. If host is in AbUpdateFailed, active volume is the current one
        host_status.servicing_state = ServicingState::AbUpdateFailed;
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        // 5. If host is doing CleanInstall, active volume is always None
        host_status.servicing_type = Some(ServicingType::CleanInstall);
        host_status.servicing_state = ServicingState::StagingDeployment;
        assert_eq!(get_ab_active_volume(&host_status), None);

        host_status.servicing_state = ServicingState::DeploymentStaged;
        assert_eq!(get_ab_active_volume(&host_status), None);

        // 6. If host is doing HotPatch, NormalUpdate, UpdateAndReboot, or AbUpdate, the active
        // volume is in host status
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        host_status.servicing_type = Some(ServicingType::HotPatch);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::NormalUpdate);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::UpdateAndReboot);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::AbUpdate);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        host_status.servicing_state = ServicingState::FinalizingDeployment;
        host_status.servicing_type = Some(ServicingType::HotPatch);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::NormalUpdate);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::UpdateAndReboot);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );

        host_status.servicing_type = Some(ServicingType::AbUpdate);
        assert_eq!(
            get_ab_active_volume(&host_status),
            host_status.storage.ab_active_volume
        );
    }
}
