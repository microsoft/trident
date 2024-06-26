#[cfg(feature = "grpc-dangerous")]
use crate::grpc;
use std::{
    fs::{self},
    path::{Path, PathBuf},
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
    config::{HostConfiguration, HostConfigurationDynamicValidationError},
    constants::{
        self, ESP_MOUNT_POINT_PATH, EXEC_ROOT_PATH, ROOT_MOUNT_POINT_PATH,
        UPDATE_ROOT_FALLBACK_PATH, UPDATE_ROOT_PATH,
    },
    error::{
        InitializationError, InternalError, InvalidInputError, ManagementError, ModuleError,
        ReportError, TridentError, TridentResultExt,
    },
    status::{AbVolumeSelection, BlockDeviceInfo, HostStatus, ServicingState, ServicingType},
    BlockDeviceId,
};

use osutils::{chroot, container, exe::RunAndCheck, mkinitrd, mount, path::join_relative};

use crate::{
    datastore::DataStore,
    modules::{
        boot::BootModule, hooks::HooksModule, management::ManagementModule, network::NetworkModule,
        osconfig::OsConfigModule, storage::StorageModule,
    },
    HostUpdateCommand,
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
pub mod selinux;

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

    // TODO: Implement dependencies
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
    ) -> Result<(), HostConfigurationDynamicValidationError> {
        Ok(())
    }

    /// Perform non-destructive preparations for an update.
    fn prepare(&mut self, _host_status: &HostStatus) -> Result<(), Error> {
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
        _mount_path: &Path,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Configure the system as specified by the host configuration, and update the host status
    /// accordingly.
    fn configure(&mut self, _host_status: &mut HostStatus, _exec_root: &Path) -> Result<(), Error> {
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

#[tracing::instrument(skip_all)]
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

    info!("Starting clean_install()");
    tracing::info!(metric_name = "clean_install_start", value = true);
    let clean_install_start_time = Instant::now();
    let mut modules = MODULES.lock().unwrap();

    refresh_host_status(&mut modules, state, true)?;
    validate_host_config(&modules, state, host_config, ServicingType::CleanInstall)?;

    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(&mut sender, state)?;

    // Stage clean install
    let (new_root_path, mounts) = stage_clean_install(
        &mut modules,
        state,
        host_config,
        #[cfg(feature = "grpc-dangerous")]
        &mut sender,
    )?;

    // Switch datastore back to the old path
    state.switch_datastore_to_path(Path::new(ROOT_MOUNT_POINT_PATH))?;

    if !allowed_operations.has_finalize() {
        info!("Finalizing of clean install not requested, skipping finalizing and reboot");
        state.close();

        info!("Unmounting '{}'", new_root_path.display());
        mount_root::unmount_new_root(mounts, &new_root_path)?;
    } else {
        finalize_clean_install(
            state,
            &new_root_path,
            Some(clean_install_start_time),
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
/// - sender: Optional mutable reference to the gRPC sender.
///
/// On success, returns a tuple with 3 elements:
/// - The current root device path.
/// - The new root device path.
/// - A vector of paths to custom mounts for the new root.
#[tracing::instrument(skip_all)]
fn stage_clean_install(
    modules: &mut MutexGuard<Vec<Box<dyn Module>>>,
    state: &mut DataStore,
    host_config: &HostConfiguration,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(PathBuf, Vec<PathBuf>), TridentError> {
    debug!("Setting host's servicing type to CleanInstall");
    debug!("Updating host's servicing state to Staging");
    state.with_host_status(|host_status| {
        host_status.servicing_type = Some(ServicingType::CleanInstall);
        host_status.servicing_state = ServicingState::Staging;
        host_status.spec = host_config.clone();
    })?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    prepare(modules, state)?;

    info!("Preparing storage to mount new root");
    let (new_root_path, exec_root_path, mounts) = initialize_new_root(state, host_config)?;

    provision(modules, state, &new_root_path)?;

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
            configure(modules, state, &exec_root_path, use_overlay)?;

            regenerate_initrd(use_overlay)?;
            selinux::execute_setfiles(host_config)?;
            root_device_path = Some(
                get_root_block_device_path(state.host_status())
                    .structured(InternalError::GetRootBlockDevice)?,
            );

            // At this point, clean install has been staged, so update servicing state
            debug!("Updating host's servicing state to Staged");
            state.with_host_status(|host_status| {
                host_status.servicing_state = ServicingState::Staged
            })?;
            #[cfg(feature = "grpc-dangerous")]
            send_host_status_state(sender, state)?;

            Ok(())
        })
        .message("Failed to execute in chroot")?;

    let root_device_path =
        root_device_path.structured(InternalError::Internal("Failed to get root block device"))?;
    debug!("Root device path: {:#?}", root_device_path);

    info!("Staging of clean install succeeded");

    // Return the new root device path and the list of mounts
    Ok((new_root_path, mounts))
}

/// Finalizes a clean install. Takes in 4 arguments:
/// - state: A mutable reference to the DataStore.
/// - new_root_path: New root device path.
/// - clean_install_start_time: Optional instant when clean install started.
/// - sender: Optional mutable reference to the gRPC sender.
#[tracing::instrument(skip_all)]
pub(super) fn finalize_clean_install(
    state: &mut DataStore,
    new_root_path: &Path,
    clean_install_start_time: Option<Instant>,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(), TridentError> {
    debug!("Updating host's servicing state to Finalizing");
    state
        .with_host_status(|host_status| host_status.servicing_state = ServicingState::Finalizing)?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    // Persist and close the datastore
    let datastore_path = state
        .host_status()
        .trident
        .datastore_path
        .clone()
        .unwrap_or_else(|| PathBuf::from("/tmp/datastore.sqlite"));
    state.persist(&join_relative(new_root_path, datastore_path))?;

    info!("Finalizing clean install");
    // On clean install, need to verify that AZLA entry exists in /mnt/newroot/boot/efi
    let esp_path = join_relative(new_root_path, ESP_MOUNT_POINT_PATH);
    finalize_deployment(state, &esp_path)?;

    // Metric for clean install provisioning time in seconds
    if let Some(start_time) = clean_install_start_time {
        tracing::info!(
            metric_name = "clean_install_provisioning_secs",
            value = start_time.elapsed().as_secs_f64()
        );
    }
    perform_reboot()?;

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

    info!("Starting update()");
    let mut modules = MODULES.lock().unwrap();

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

    validate_host_config(&modules, state, host_config, servicing_type)?;

    // Stage update
    let (new_root_path, mounts) = stage_update(
        &mut modules,
        state,
        host_config,
        servicing_type,
        #[cfg(feature = "grpc-dangerous")]
        &mut sender,
    )?;

    match servicing_type {
        ServicingType::UpdateAndReboot | ServicingType::AbUpdate => {
            if !allowed_operations.has_finalize() {
                info!("Finalizing of update not requested, skipping reboot");
                if let Some(mounts) = mounts {
                    mount_root::unmount_new_root(mounts, &new_root_path)?;
                }
                return Ok(());
            }

            // Otherwise, finalize update
            finalize_update(
                state,
                #[cfg(feature = "grpc-dangerous")]
                &mut sender,
            )?;

            Ok(())
        }
        ServicingType::NormalUpdate | ServicingType::HotPatch => {
            state.with_host_status(|host_status| {
                host_status.servicing_type = None;
                host_status.servicing_state = ServicingState::Provisioned;
            })?;
            #[cfg(feature = "grpc-dangerous")]
            send_host_status_state(&mut sender, state)?;

            info!("Update complete");
            Ok(())
        }
        ServicingType::Incompatible | ServicingType::CleanInstall => {
            unreachable!()
        }
    }
}

/// Stages an update. Takes in 5 arguments:
/// - modules: A mutable reference to the list of modules.
/// - state: A mutable reference to the DataStore.
/// - host_config: Updated host configuration.
/// - servicing_type: Servicing type of the update that Trident will now stage, based on host config.
/// - sender: Optional mutable reference to the gRPC sender.
///
/// On success, returns a tuple with 2 elements:
/// - New root device path.
/// - Vector of paths to custom mounts for the new root. This is not null only for A/B updates.
fn stage_update(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
    servicing_type: ServicingType,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(PathBuf, Option<Vec<PathBuf>>), TridentError> {
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

    // Update host status and copy new host config to the spec field
    debug!("Setting host's servicing type to {:?}", servicing_type);
    debug!("Updating host's servicing state to Staging");
    state.with_host_status(|host_status| {
        host_status.servicing_type = Some(servicing_type);
        host_status.servicing_state = ServicingState::Staging;
        host_status.spec = host_config.clone();
    })?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    prepare(modules, state)?;

    let (new_root_path, mounts) = if let ServicingType::AbUpdate = servicing_type {
        info!("Preparing storage to mount new root");
        let (new_root_path, exec_root_path, mounts) = initialize_new_root(state, host_config)?;

        provision(modules, state, &new_root_path)?;

        // If verity is present, it means that we are currently doing root
        // verity. For now, we can assume that /etc is readonly, so we setup
        // a writable overlay for it.
        let use_overlay = !host_config.storage.internal_verity.is_empty();

        info!("Entering '{}' chroot", new_root_path.display());
        chroot::enter_update_chroot(&new_root_path)
            .message("Failed to enter chroot")?
            .execute_and_exit(|| {
                configure(modules, state, &exec_root_path, use_overlay)?;

                regenerate_initrd(use_overlay)?;
                selinux::execute_setfiles(host_config)
            })
            .message("Failed to execute in chroot")?;

        (new_root_path, Some(mounts))
    } else {
        info!("Running configure");
        configure(modules, state, Path::new(ROOT_MOUNT_POINT_PATH), false)?;

        regenerate_initrd(false)?;

        (PathBuf::from(ROOT_MOUNT_POINT_PATH), None)
    };

    // At this point, deployment has been staged, so update servicing state
    debug!("Updating host's servicing state to Staged");
    state.with_host_status(|host_status| host_status.servicing_state = ServicingState::Staged)?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    info!("Staging of update '{:?}' succeeded", servicing_type);

    Ok((new_root_path, mounts))
}

/// Finalizes an update. Takes in 2 arguments:
/// - state: A mutable reference to the DataStore.
/// - sender: Optional mutable reference to the gRPC sender.
pub(super) fn finalize_update(
    state: &mut DataStore,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(), TridentError> {
    debug!("Updating host's servicing state to Finalizing");
    state
        .with_host_status(|host_status| host_status.servicing_state = ServicingState::Finalizing)?;

    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    info!("Finalizing update");
    finalize_deployment(state, Path::new(ESP_MOUNT_POINT_PATH))?;

    perform_reboot()?;

    Ok(())
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
pub(super) fn get_root_block_device_path(host_status: &HostStatus) -> Option<PathBuf> {
    host_status
        .spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
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
                host_status.get_ab_active_volume()
            } else {
                host_status.get_ab_update_volume()
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

#[tracing::instrument(skip_all)]
fn refresh_host_status(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    clean_install: bool,
) -> Result<(), TridentError> {
    info!("Starting stage 'Refresh'");
    for module in modules {
        debug!("Starting stage 'Refresh' for module '{}'", module.name());
        state.try_with_host_status(|s| {
            module
                .refresh_host_status(s, clean_install)
                .structured(ManagementError::from(ModuleError::RefreshHostStatus {
                    name: module.name(),
                }))
        })?;
    }
    debug!("Finished stage 'Refresh'");
    Ok(())
}

#[tracing::instrument(skip_all)]
fn validate_host_config(
    modules: &[Box<dyn Module>],
    state: &DataStore,
    host_config: &HostConfiguration,
    planned_servicing_type: ServicingType,
) -> Result<(), TridentError> {
    info!("Starting stage 'Validate'");
    for module in modules {
        debug!("Starting stage 'Validate' for module '{}'", module.name());
        module
            .validate_host_config(state.host_status(), host_config, planned_servicing_type)
            .map_err(|e| {
                TridentError::new(ManagementError::from(
                    ModuleError::ValidateHostConfiguration {
                        name: module.name(),
                        inner: e,
                    },
                ))
            })?;
    }
    debug!("Finished stage 'Validate'");
    Ok(())
}

fn prepare(modules: &mut [Box<dyn Module>], state: &mut DataStore) -> Result<(), TridentError> {
    info!("Starting stage 'Prepare'");
    for module in modules {
        debug!("Starting stage 'Prepare' for module '{}'", module.name());
        state.try_with_host_status(|s| {
            module
                .prepare(s)
                .structured(ManagementError::from(ModuleError::Prepare {
                    name: module.name(),
                }))
        })?;
    }
    debug!("Finished stage 'Prepare'");
    Ok(())
}

fn provision(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    new_root_path: &Path,
) -> Result<(), TridentError> {
    // If verity is present, it means that we are currently doing root
    // verity. For now, we can assume that /etc is readonly, so we setup
    // a writable overlay for it.
    let use_overlay = !state.host_status().spec.storage.internal_verity.is_empty();

    info!("Starting stage 'Provision'");
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
                .provision(host_status, new_root_path)
                .structured(ManagementError::from(ModuleError::Provision {
                    name: module.name(),
                }))
        })?;
    }
    debug!("Finished stage 'Provision'");
    Ok(())
}

/// Returns the path to the new root.
pub(super) fn get_new_root_path() -> PathBuf {
    let mut new_root_path = Path::new(UPDATE_ROOT_PATH);
    if mount::ensure_mount_directory(new_root_path).is_err() {
        new_root_path = Path::new(UPDATE_ROOT_FALLBACK_PATH);
    }
    new_root_path.to_owned()
}

#[tracing::instrument(skip_all)]
pub(super) fn initialize_new_root(
    state: &mut DataStore,
    host_config: &HostConfiguration,
) -> Result<(PathBuf, PathBuf, Vec<PathBuf>), TridentError> {
    let new_root_path = get_new_root_path();

    // Only initialize block devices if currently staging a deployment
    if state.host_status().servicing_state == ServicingState::Staging {
        state.try_with_host_status(|host_status| {
            storage::initialize_block_devices(host_status, host_config, &new_root_path)
        })?;
    }

    // Mount new root while staging a new deployment OR while finalizing a previous deployment
    let mut mounts = state.try_with_host_status(|host_status| {
        mount_root::mount_new_root(host_status, &new_root_path)
    })?;

    let tmp_mount = Mount::builder()
        .fstype("tmpfs")
        .flags(MountFlags::empty())
        .mount("tmpfs", new_root_path.join("tmp"))
        .structured(ManagementError::ChrootMountSpecial { dir: "/tmp" })?;
    // Insert tmp_mount at the end of the mounts vector
    mounts.push(tmp_mount.target_path().to_owned());

    let exec_root_path = Path::new(EXEC_ROOT_PATH);
    let full_exec_root_path = join_relative(&new_root_path, exec_root_path);
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
    exec_root: &Path,
    use_overlay: bool,
) -> Result<(), TridentError> {
    info!("Starting stage 'Configure'");
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
                .configure(s, exec_root)
                .structured(ManagementError::from(ModuleError::Configure {
                    name: module.name(),
                }))
        })?;
    }
    debug!("Finished stage 'Configure'");

    Ok(())
}

/// Regenerates the initrd for the host, using host-specific configuration.
#[tracing::instrument(skip_all)]
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
fn perform_reboot() -> Result<(), TridentError> {
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
/// to Finalized.
#[tracing::instrument(skip_all)]
fn finalize_deployment(datastore: &mut DataStore, esp_path: &Path) -> Result<(), TridentError> {
    // TODO: Delete boot entries. Related ADO task:
    // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6807/

    info!("Setting boot entries");
    datastore.try_with_host_status(|host_status| {
        bootentries::call_set_boot_next_and_update_hs(host_status, esp_path)
    })?;

    debug!("Updating host's servicing state to Finalized");
    datastore.with_host_status(|status| status.servicing_state = ServicingState::Finalized)?;

    info!("Closing datastore");
    datastore.close();

    Ok(())
}

#[cfg(test)]
mod test {

    use maplit::btreemap;

    use trident_api::{
        config::{self, AbUpdate, AbVolumePair, Disk, FileSystemType, Partition, PartitionType},
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
                                size: 2.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root".to_owned(),
                                size: 7.into(),
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
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
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
                                    size: 100.into(),
                                    partition_type: PartitionType::Esp,
                                },
                                Partition {
                                    id: "root".to_owned(),
                                    size: 900.into(),
                                    partition_type: PartitionType::Root,
                                },
                                Partition {
                                    id: "rootb".to_owned(),
                                    size: 9000.into(),
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
        host_status.servicing_state = ServicingState::Staging;
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
}
