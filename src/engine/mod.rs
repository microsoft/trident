use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

#[cfg(feature = "grpc-dangerous")]
use tokio::sync::mpsc;

use chrono::Utc;
use log::{debug, error, info, warn};

use osutils::{block_devices, chroot, container, dependencies::Dependency, path::join_relative};
use trident_api::{
    config::HostConfiguration,
    constants::{
        self,
        internal_params::{ENABLE_UKI_SUPPORT, NO_TRANSITION},
        ESP_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH, UPDATE_ROOT_PATH,
    },
    error::{
        InitializationError, InternalError, InvalidInputError, ReportError, ServicingError,
        TridentError, TridentResultExt,
    },
    status::{AbVolumeSelection, HostStatus, ServicingState, ServicingType},
    BlockDeviceId,
};

#[cfg(feature = "grpc-dangerous")]
use crate::grpc::{self, protobufs::HostStatusState};
use crate::{
    datastore::DataStore,
    engine::{boot::BootSubsystem, storage::StorageSubsystem},
    subsystems::{
        hooks::HooksSubsystem,
        initrd::InitrdSubsystem,
        management::ManagementSubsystem,
        network::NetworkSubsystem,
        osconfig::{MosConfigSubsystem, OsConfigSubsystem},
        selinux::SelinuxSubsystem,
    },
    HostUpdateCommand, SAFETY_OVERRIDE_CHECK_PATH, TRIDENT_BACKGROUND_LOG_PATH,
    TRIDENT_METRICS_FILE_PATH,
};

// Engine functionality
pub mod bootentries;
mod context;
mod kexec;
mod newroot;
mod osimage;
pub mod provisioning_network;
pub mod rollback;

// Trident Subsystems
pub mod boot;
pub mod storage;

// Helper modules
mod etc_overlay;

pub use newroot::NewrootMount;

pub(crate) use context::EngineContext;

pub(crate) trait Subsystem: Send {
    fn name(&self) -> &'static str;

    fn writable_etc_overlay(&self) -> bool {
        true
    }

    // TODO: Implement dependencies
    // fn dependencies(&self) -> &'static [&'static str];

    /// Select the servicing type based on the host status and host config.
    fn select_servicing_type(
        &self,
        _ctx: &EngineContext,
    ) -> Result<Option<ServicingType>, TridentError> {
        Ok(None)
    }

    /// Validate the host config.
    fn validate_host_config(
        &self,
        _ctx: &EngineContext,
        _host_config: &HostConfiguration,
    ) -> Result<(), TridentError> {
        Ok(())
    }

    /// Perform non-destructive preparations for an update.
    fn prepare(&mut self, _ctx: &EngineContext) -> Result<(), TridentError> {
        Ok(())
    }

    /// Initialize state on the Runtime OS from the Provisioning OS, or migrate state from
    /// A-partition to B-partition (or vice versa).
    ///
    /// This method is called before the chroot is entered, and is used to perform any
    /// provisioning operations that need to be done before the chroot is entered.
    fn provision(&mut self, _ctx: &EngineContext, _mount_path: &Path) -> Result<(), TridentError> {
        Ok(())
    }

    /// Configure the system as specified by the host configuration, and update the host status
    /// accordingly.
    fn configure(&mut self, _ctx: &EngineContext, _exec_root: &Path) -> Result<(), TridentError> {
        Ok(())
    }
}

lazy_static::lazy_static! {
    static ref SUBSYSTEMS: Mutex<Vec<Box<dyn Subsystem>>> = Mutex::new(vec![
        Box::<MosConfigSubsystem>::default(),
        Box::<StorageSubsystem>::default(),
        Box::<BootSubsystem>::default(),
        Box::<NetworkSubsystem>::default(),
        Box::<OsConfigSubsystem>::default(),
        Box::<ManagementSubsystem>::default(),
        Box::<HooksSubsystem>::default(),
        Box::<InitrdSubsystem>::default(),
        Box::<SelinuxSubsystem>::default(),
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

    info!("Starting clean install");
    tracing::info!(metric_name = "clean_install_start", value = true);
    let clean_install_start_time = Instant::now();

    if Path::new(UPDATE_ROOT_PATH).exists()
        && osutils::mountpoint::check_is_mountpoint(UPDATE_ROOT_PATH).structured(
            ServicingError::CheckIfMountPoint {
                path: UPDATE_ROOT_PATH.to_string(),
            },
        )?
    {
        debug!("Unmounting volumes from earlier runs of Trident");
        if let Err(e) = osutils::mount::umount(UPDATE_ROOT_PATH, true) {
            warn!("Attempt to unmount '{UPDATE_ROOT_PATH}' returned error: {e}",);
        }
    }

    // This is a safety check so that nobody accidentally formats their dev
    // machine.
    debug!("Performing safety check for clean install");
    clean_install_safety_check(&command.host_config)?;
    info!("Safety check passed");

    let mut subsystems = SUBSYSTEMS.lock().unwrap();

    // Stage clean install
    let root_mount = stage_clean_install(
        &mut subsystems,
        state,
        host_config,
        #[cfg(feature = "grpc-dangerous")]
        &mut sender,
    )?;

    if !allowed_operations.has_finalize() {
        info!("Finalizing of clean install not requested, skipping finalizing and reboot");
        state.close();

        debug!("Unmounting '{}'", root_mount.path().display());
        root_mount.unmount_all()?;
    } else {
        finalize_clean_install(
            state,
            Some(root_mount),
            Some(clean_install_start_time),
            #[cfg(feature = "grpc-dangerous")]
            &mut sender,
        )?;
    }

    Ok(())
}

/// Performs a safety check to ensure that the clean install can proceed.
fn clean_install_safety_check(host_config: &HostConfiguration) -> Result<(), TridentError> {
    // Check if Trident is running from a live image
    let cmdline =
        fs::read_to_string("/proc/cmdline").structured(InitializationError::ReadCmdline)?;
    if cmdline.contains("root=/dev/ram0") || cmdline.contains("root=live:LABEL=CDROM") {
        debug!("Trident is running from a live image.");
        return Ok(());
    }

    warn!("Trident is running from an OS installed on persistent storage");

    // Check if we have adopted partitions in the host config
    if host_config
        .storage
        .disks
        .iter()
        .any(|d| !d.adopted_partitions.is_empty())
    {
        debug!("Partitions are marked for adoption");
        return Ok(());
    }

    warn!("No partitions are marked for adoption");

    // Check if we are running in a container and if so, adjust the path to the safety
    // override file accordingly.
    let safety_override_path = if container::is_running_in_container()
        .message("Failed to check if Trident is running in a container.")?
    {
        container::get_host_root_path()
            .message("Failed to get host root path.")?
            .join(SAFETY_OVERRIDE_CHECK_PATH.trim_start_matches(ROOT_MOUNT_POINT_PATH))
    } else {
        PathBuf::from(SAFETY_OVERRIDE_CHECK_PATH)
    };

    if safety_override_path.exists() {
        debug!("Safety check override file is present");
        return Ok(());
    }

    error!("Safety override file is not present, aborting clean install");
    Err(TridentError::new(
        InitializationError::CleanInstallSafetyCheck,
    ))
}

/// Stages a clean install. Takes in 4 arguments:
/// - subsystems: A mutable reference to the list of subsystems.
/// - state: A mutable reference to the DataStore.
/// - host_config: A reference to the HostConfiguration.
/// - sender: Optional mutable reference to the gRPC sender.
///
/// On success, returns a NewrootMount.
#[tracing::instrument(skip_all)]
fn stage_clean_install(
    subsystems: &mut MutexGuard<Vec<Box<dyn Subsystem>>>,
    state: &mut DataStore,
    host_config: &HostConfiguration,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<NewrootMount, TridentError> {
    // Initialize a copy of the host status with the changes that are planned. We make a copy rather
    // than modifying the datastore's version so that we can wait until the clean install is staged
    // before committing the changes.
    let mut ctx = EngineContext {
        spec: host_config.clone(),
        spec_old: Default::default(),
        servicing_type: ServicingType::CleanInstall,
        ab_active_volume: None,
        block_device_paths: Default::default(), // Will be initialized later
        disks_by_uuid: Default::default(),      // Will be initialized later
        install_index: 0,                       // Will be initialized later
        os_image: osimage::load_os_image(host_config)?,
    };

    // Execute pre-servicing scripts
    HooksSubsystem::default().execute_pre_servicing_scripts(&ctx)?;

    validate_host_config(subsystems, &ctx, host_config)?;

    debug!("Clearing saved host status");
    state.with_host_status(|host_status| {
        host_status.spec = Default::default();
        host_status.servicing_type = ServicingType::NoActiveServicing;
        host_status.servicing_state = ServicingState::NotProvisioned;
    })?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    prepare(subsystems, &ctx)?;

    debug!("Preparing storage to mount new root");
    storage::create_block_devices(&mut ctx)?;
    storage::initialize_block_devices(&ctx)?;
    let newroot_mount = NewrootMount::create_and_mount(
        host_config,
        &ctx.block_device_paths,
        AbVolumeSelection::VolumeA,
    )?;
    ctx.install_index = boot::esp::next_install_index(newroot_mount.path())?;

    provision(subsystems, &ctx, newroot_mount.path())?;

    debug!("Entering '{}' chroot", newroot_mount.path().display());
    let result = chroot::enter_update_chroot(newroot_mount.path())
        .message("Failed to enter chroot")?
        .execute_and_exit(|| configure(subsystems, &ctx, newroot_mount.execroot_relative_path()));

    if let Err(original_error) = result {
        if let Err(e) = newroot_mount.unmount_all() {
            warn!("While handling an earlier error: {e:?}");
        }
        return Err(original_error).message("Failed to execute in chroot");
    }

    // At this point, clean install has been staged, so update host status
    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::Staged
    );
    state.with_host_status(|hs| {
        *hs = HostStatus {
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staged,
            spec: host_config.clone(),
            spec_old: Default::default(),
            ab_active_volume: None,
            block_device_paths: ctx.block_device_paths,
            disks_by_uuid: ctx.disks_by_uuid,
            install_index: ctx.install_index,
            last_error: None,
            is_management_os: true,
        }
    })?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    info!("Staging of clean install succeeded");
    Ok(newroot_mount)
}

/// Finalizes a clean install. Takes in 4 arguments:
/// - state: A mutable reference to the DataStore.
/// - new_root_path: New root device path. If None, a new root is created and mounted.
/// - clean_install_start_time: Optional instant when clean install started.
/// - sender: Optional mutable reference to the gRPC sender.
#[tracing::instrument(skip_all)]
pub(super) fn finalize_clean_install(
    state: &mut DataStore,
    new_root: Option<NewrootMount>,
    clean_install_start_time: Option<Instant>,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(), TridentError> {
    info!("Finalizing clean install");

    let ctx = EngineContext {
        spec: state.host_status().spec.clone(),
        spec_old: state.host_status().spec_old.clone(),
        servicing_type: state.host_status().servicing_type,
        ab_active_volume: state.host_status().ab_active_volume,
        block_device_paths: state.host_status().block_device_paths.clone(),
        disks_by_uuid: state.host_status().disks_by_uuid.clone(),
        install_index: state.host_status().install_index,
        os_image: None, // Not used in finalize_clean_install
    };

    let new_root = match new_root {
        Some(new_root) => new_root,
        None => NewrootMount::create_and_mount(
            &ctx.spec,
            &ctx.block_device_paths,
            ctx.get_ab_update_volume()
                .structured(InternalError::Internal(
                    "No update volume despite there being an update in prgoress",
                ))?,
        )?,
    };

    // On clean install, need to verify that AZLA entry exists in /mnt/newroot/boot/efi
    let esp_path = join_relative(new_root.path(), ESP_MOUNT_POINT_PATH);
    bootentries::set_boot_next_and_update_boot_order(&ctx, &esp_path)?;

    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::Finalized
    );
    state.with_host_status(|status| status.servicing_state = ServicingState::Finalized)?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    // Persist the datastore to the new root
    state.persist(&join_relative(
        new_root.path(),
        &state.host_status().spec.trident.datastore_path,
    ))?;
    state.close();

    // Metric for clean install provisioning time in seconds
    if let Some(start_time) = clean_install_start_time {
        tracing::info!(
            metric_name = "clean_install_provisioning_secs",
            value = start_time.elapsed().as_secs_f64()
        );
    }

    // Persist the Trident background log and metrics file to the new root
    persist_background_log_and_metrics(
        &state.host_status().spec.trident.datastore_path,
        Some(new_root.path()),
        ServicingType::CleanInstall,
    );

    if let Err(e) = new_root.unmount_all() {
        error!("Failed to unmount new root: {e:?}");
    }

    storage::check_block_devices(state.host_status());

    if !state
        .host_status()
        .spec
        .internal_params
        .get_flag(NO_TRANSITION)
    {
        reboot()
    } else {
        warn!(
            "Skipping reboot as requested by internal parameter '{}'",
            NO_TRANSITION
        );
        Ok(())
    }
}

#[tracing::instrument(skip_all)]
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

    info!("Starting update");
    let mut subsystems = SUBSYSTEMS.lock().unwrap();

    if state.host_status().servicing_type == ServicingType::AbUpdate {
        debug!("Resetting A/B update state");
        state.with_host_status(|host_status| {
            host_status.spec = host_status.spec_old.clone();
            host_status.spec_old = Default::default();
            host_status.servicing_type = ServicingType::NoActiveServicing;
            host_status.servicing_state = ServicingState::Provisioned;
        })?;
    }

    let mut ctx = EngineContext {
        spec: command.host_config.clone(),
        spec_old: state.host_status().spec.clone(),
        servicing_type: ServicingType::NoActiveServicing,
        block_device_paths: state.host_status().block_device_paths.clone(),
        ab_active_volume: state.host_status().ab_active_volume,
        disks_by_uuid: state.host_status().disks_by_uuid.clone(),
        install_index: state.host_status().install_index,
        os_image: osimage::load_os_image(&command.host_config)?,
    };

    // Before starting an update servicing, need to validate that the active volume is set
    // correctly.
    if ctx.spec.storage.ab_update.is_some() {
        debug!(
            "A/B update is enabled, validating that '{:?}' is currently active",
            ctx.ab_active_volume
                .map_or("None".to_string(), |v| v.to_string())
        );
        let root_device_path = block_devices::get_root_device_path()?;
        rollback::validate_active_volume(&ctx, root_device_path)
            .structured(ServicingError::ValidateAbActiveVolume)?;
    }

    debug!("Determining servicing type");
    let servicing_type = subsystems
        .iter()
        .map(|m| m.select_servicing_type(&ctx))
        .collect::<Result<Vec<_>, TridentError>>()?
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(ServicingType::NoActiveServicing); // Never None b/c select_servicing_type() returns a value
    if servicing_type == ServicingType::NoActiveServicing {
        info!("No updates required");
        return Ok(());
    }
    debug!(
        "Selected servicing type for the required update: {:?}",
        servicing_type
    );

    ctx.servicing_type = servicing_type;

    // Execute pre-servicing scripts
    HooksSubsystem::default().execute_pre_servicing_scripts(&ctx)?;

    validate_host_config(&subsystems, &ctx, host_config)?;

    let update_start_time = Instant::now();
    tracing::info!(
        metric_name = "update_start",
        servicing_type = format!("{:?}", servicing_type),
        servicing_state = format!("{:?}", state.host_status().servicing_state),
    );

    // Stage update
    stage_update(
        &mut subsystems,
        ctx,
        state,
        #[cfg(feature = "grpc-dangerous")]
        &mut sender,
    )
    .message("Failed to stage update")?;

    match servicing_type {
        ServicingType::UpdateAndReboot | ServicingType::AbUpdate => {
            if !allowed_operations.has_finalize() {
                info!("Finalizing of update not requested, skipping reboot");
            } else {
                finalize_update(
                    state,
                    Some(update_start_time),
                    #[cfg(feature = "grpc-dangerous")]
                    &mut sender,
                )
                .message("Failed to finalize update")?;
            }

            Ok(())
        }
        ServicingType::NormalUpdate | ServicingType::HotPatch => {
            state.with_host_status(|host_status| {
                host_status.servicing_type = ServicingType::NoActiveServicing;
                host_status.servicing_state = ServicingState::Provisioned;
            })?;
            #[cfg(feature = "grpc-dangerous")]
            send_host_status_state(&mut sender, state)?;

            // Persist the Trident background log and metrics file to the updated runtime OS
            persist_background_log_and_metrics(
                &state.host_status().spec.trident.datastore_path,
                None,
                servicing_type,
            );

            info!("Update complete");
            Ok(())
        }
        ServicingType::CleanInstall => Err(TridentError::new(
            InvalidInputError::CleanInstallOnProvisionedHost,
        )),
        ServicingType::NoActiveServicing => Err(TridentError::internal("No active servicing type")),
    }
}

/// Stages an update. Takes in 5 arguments:
/// - subsystems: A mutable reference to the list of subsystems.
/// - state: A mutable reference to the DataStore.
/// - host_config: Updated host configuration.
/// - servicing_type: Servicing type of the update that Trident will now stage, based on host
/// config.
/// - sender: Optional mutable reference to the gRPC sender.
///
/// On success, returns an Option<NewrootMount>; This is not null only for A/B updates.
#[tracing::instrument(skip_all, fields(servicing_type = format!("{:?}", ctx.servicing_type)))]
fn stage_update(
    subsystems: &mut [Box<dyn Subsystem>],
    ctx: EngineContext,
    state: &mut DataStore,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(), TridentError> {
    match ctx.servicing_type {
        ServicingType::HotPatch => info!("Performing hot patch update"),
        ServicingType::NormalUpdate => info!("Performing normal update"),
        ServicingType::UpdateAndReboot => info!("Performing update and reboot"),
        ServicingType::AbUpdate => info!("Performing A/B update"),
        ServicingType::CleanInstall => {
            return Err(TridentError::new(
                InvalidInputError::CleanInstallOnProvisionedHost,
            ));
        }
        ServicingType::NoActiveServicing => unreachable!(),
    }

    prepare(subsystems, &ctx)?;

    if let ServicingType::AbUpdate = ctx.servicing_type {
        debug!("Preparing storage to mount new root");
        storage::initialize_block_devices(&ctx)?;
        let newroot_mount = NewrootMount::create_and_mount(
            &ctx.spec,
            &ctx.block_device_paths,
            ctx.get_ab_update_volume()
                .structured(InternalError::Internal(
                    "No update volume despite there being an update in progress",
                ))?,
        )?;

        provision(subsystems, &ctx, newroot_mount.path())?;

        debug!("Entering '{}' chroot", newroot_mount.path().display());
        let result = chroot::enter_update_chroot(newroot_mount.path())
            .message("Failed to enter chroot")?
            .execute_and_exit(|| {
                configure(subsystems, &ctx, newroot_mount.execroot_relative_path())
            });

        if let Err(original_error) = result {
            if let Err(e) = newroot_mount.unmount_all() {
                warn!("While handling an earlier error: {e:?}");
            }
            return Err(original_error).message("Failed to execute in chroot");
        }

        newroot_mount.unmount_all()?;
    } else {
        configure(subsystems, &ctx, Path::new(ROOT_MOUNT_POINT_PATH))?;
    };

    // At this point, deployment has been staged, so update servicing state
    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::Staged
    );
    state.with_host_status(|hs| {
        *hs = HostStatus {
            spec: ctx.spec,
            spec_old: ctx.spec_old,
            servicing_state: ServicingState::Staged,
            servicing_type: ctx.servicing_type,
            ab_active_volume: ctx.ab_active_volume,
            block_device_paths: ctx.block_device_paths,
            disks_by_uuid: ctx.disks_by_uuid,
            install_index: ctx.install_index,
            last_error: None,
            is_management_os: false,
        };
    })?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;

    info!("Staging of update '{:?}' succeeded", ctx.servicing_type);

    Ok(())
}

/// Finalizes an update. Takes in 2 arguments:
/// - state: A mutable reference to the DataStore.
/// - sender: Optional mutable reference to the gRPC sender.
#[tracing::instrument(skip_all, fields(servicing_type = format!("{:?}", state.host_status().servicing_type)))]
pub(super) fn finalize_update(
    state: &mut DataStore,
    update_start_time: Option<Instant>,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(), TridentError> {
    info!("Finalizing update");

    let ctx = EngineContext {
        spec: state.host_status().spec.clone(),
        spec_old: state.host_status().spec_old.clone(),
        servicing_type: state.host_status().servicing_type,
        ab_active_volume: state.host_status().ab_active_volume,
        block_device_paths: state.host_status().block_device_paths.clone(),
        disks_by_uuid: state.host_status().disks_by_uuid.clone(),
        install_index: state.host_status().install_index,
        os_image: None, // Not used in finalize_update
    };

    let esp_path = if container::is_running_in_container()
        .message("Failed to check if Trident is running in a container.")?
    {
        let host_root = container::get_host_root_path().message("Failed to get host root path.")?;
        join_relative(host_root, ESP_MOUNT_POINT_PATH)
    } else {
        PathBuf::from(ESP_MOUNT_POINT_PATH)
    };
    bootentries::set_boot_next_and_update_boot_order(&ctx, &esp_path)?;

    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::Finalized
    );
    state.with_host_status(|status| status.servicing_state = ServicingState::Finalized)?;
    #[cfg(feature = "grpc-dangerous")]
    send_host_status_state(sender, state)?;
    state.close();

    // Metric for update time in seconds
    if let Some(start_time) = update_start_time {
        tracing::info!(
            metric_name = "update_time_secs",
            value = start_time.elapsed().as_secs_f64(),
            servicing_type = format!("{:?}", state.host_status().servicing_type)
        );
    }

    // Persist the Trident background log and metrics file to the updated runtime OS
    persist_background_log_and_metrics(
        &state.host_status().spec.trident.datastore_path,
        None,
        state.host_status().servicing_type,
    );

    if !state
        .host_status()
        .spec
        .internal_params
        .get_flag(NO_TRANSITION)
    {
        reboot()
    } else {
        warn!(
            "Skipping reboot as requested by internal parameter '{}'",
            NO_TRANSITION
        );
        Ok(())
    }
}

/// Persists the Trident background log and metrics files to the updated runtime
/// OS, by copying the files at TRIDENT_BACKGROUND_LOG_PATH and
/// TRIDENT_METRICS_FILE_PATH to the directory adjacent to the datastore. On
/// failure, only prints out an error message.
///
/// In case of clean install, the files are persisted to the datastore path in
/// the new root, so newroot_path is provided.
fn persist_background_log_and_metrics(
    datastore_path: &Path,
    newroot_path: Option<&Path>,
    servicing_type: ServicingType,
) {
    // Generate the new log filename based on the servicing type and the current timestamp
    let new_background_log_filename = format!(
        "trident-{:?}-{}.log",
        servicing_type,
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );

    // Generate the new metrics filename based on the servicing type and the current timestamp
    let new_metrics_filename = format!(
        "trident-metrics-{:?}-{}.jsonl",
        servicing_type,
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );

    // Fetch the directory path from the full datastore path
    let Some(datastore_dir) = datastore_path.parent() else {
        warn!(
            "Failed to get parent directory for datastore path '{}'",
            datastore_path.display()
        );
        return;
    };

    // Create the full path for the new background log file
    let new_background_log_path: PathBuf = if let Some(new_root) = newroot_path {
        join_relative(new_root, datastore_dir).join(new_background_log_filename)
    } else {
        datastore_dir.join(new_background_log_filename)
    };

    debug!(
        "Persisting Trident background log from '{}' to '{}' ",
        TRIDENT_BACKGROUND_LOG_PATH,
        new_background_log_path.display()
    );

    // Create the full path for the new metrics file
    let new_metrics_path: PathBuf = if let Some(new_root) = newroot_path {
        join_relative(new_root, datastore_dir).join(new_metrics_filename)
    } else {
        datastore_dir.join(new_metrics_filename)
    };

    debug!(
        "Persisting Trident metrics from '{}' to '{}' ",
        TRIDENT_METRICS_FILE_PATH,
        new_metrics_path.display()
    );

    // Copy the background log file to the new location
    if let Err(log_error) = fs::copy(TRIDENT_BACKGROUND_LOG_PATH, &new_background_log_path) {
        warn!(
            "Failed to persist Trident background log from '{}' to '{}': {}",
            TRIDENT_BACKGROUND_LOG_PATH,
            new_background_log_path.display(),
            log_error
        );
    } else {
        debug!(
            "Successfully persisted Trident background log from '{}' to '{}'",
            TRIDENT_BACKGROUND_LOG_PATH,
            new_background_log_path.display()
        );
    }

    // Copy the metrics file to the new location
    if let Err(e) = fs::copy(TRIDENT_METRICS_FILE_PATH, &new_metrics_path) {
        warn!(
            "Failed to persist Trident metrics file from '{}' to '{}': {}",
            TRIDENT_METRICS_FILE_PATH,
            new_metrics_path.display(),
            e
        );
    } else {
        debug!(
            "Successfully persisted Trident metrics from '{}' to '{}' ",
            TRIDENT_METRICS_FILE_PATH,
            new_metrics_path.display()
        );
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
pub(super) fn get_root_block_device_path(ctx: &EngineContext) -> Option<PathBuf> {
    ctx.spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
        .and_then(|m| get_block_device_path(ctx, &m.target_id))
}

/// Returns the path of the block device with id `block_device_id`.
///
/// If the volume is part of an A/B Volume Pair this returns the update volume (i.e. the one that
/// isn't active).
pub(super) fn get_block_device_path(
    ctx: &EngineContext,
    block_device_id: &BlockDeviceId,
) -> Option<PathBuf> {
    if let Some(partition_path) = ctx.block_device_paths.get(block_device_id) {
        return Some(partition_path.clone());
    }

    if let Some(raid) = ctx
        .spec
        .storage
        .raid
        .software
        .iter()
        .find(|r| &r.id == block_device_id)
    {
        return Some(raid.device_path());
    }

    if let Some(encryption) = &ctx.spec.storage.encryption {
        if let Some(encrypted) = encryption.volumes.iter().find(|e| &e.id == block_device_id) {
            return Some(encrypted.device_path());
        }
    }

    if let Some(verity) = ctx
        .spec
        .storage
        .internal_verity
        .iter()
        .find(|v| &v.id == block_device_id)
    {
        return Some(verity.device_path());
    }

    get_ab_volume_block_device_id(ctx, block_device_id)
        .and_then(|child_block_device_id| get_block_device_path(ctx, child_block_device_id))
}

/// Returns the block device id for the update volume from the given A/B Volume Pair.
fn get_ab_volume_block_device_id<'a>(
    ctx: &'a EngineContext,
    block_device_id: &BlockDeviceId,
) -> Option<&'a BlockDeviceId> {
    if let Some(ab_update) = &ctx.spec.storage.ab_update {
        let ab_volume = ab_update
            .volume_pairs
            .iter()
            .find(|v| &v.id == block_device_id);
        if let Some(v) = ab_volume {
            let selection = ctx.get_ab_update_volume();
            // Return the appropriate BlockDeviceId based on the selection
            return selection.map(|sel| match sel {
                AbVolumeSelection::VolumeA => &v.volume_a_id,
                AbVolumeSelection::VolumeB => &v.volume_b_id,
            });
        };
    }
    None
}

#[tracing::instrument(skip_all)]
fn validate_host_config(
    subsystems: &[Box<dyn Subsystem>],
    ctx: &EngineContext,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    info!("Starting step 'Validate'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Validate' for subsystem '{}'",
            subsystem.name()
        );
        subsystem
            .validate_host_config(ctx, host_config)
            .message(format!(
                "Step 'Validate' failed for subsystem '{}'",
                subsystem.name()
            ))?;
    }
    debug!("Finished step 'Validate'");
    Ok(())
}

fn prepare(subsystems: &mut [Box<dyn Subsystem>], ctx: &EngineContext) -> Result<(), TridentError> {
    info!("Starting step 'Prepare'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Prepare' for subsystem '{}'",
            subsystem.name()
        );
        subsystem.prepare(ctx).message(format!(
            "Step 'Prepare' failed for subsystem '{}'",
            subsystem.name()
        ))?;
    }
    debug!("Finished step 'Prepare'");
    Ok(())
}

fn provision(
    subsystems: &mut [Box<dyn Subsystem>],
    ctx: &EngineContext,
    new_root_path: &Path,
) -> Result<(), TridentError> {
    // If verity is present, it means that we are currently doing root
    // verity. For now, we can assume that /etc is readonly, so we setup
    // a writable overlay for it.
    let use_overlay = !ctx.spec.storage.internal_verity.is_empty();

    info!("Starting step 'Provision'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Provision' for subsystem '{}'",
            subsystem.name()
        );
        let _etc_overlay_mount = if use_overlay {
            Some(etc_overlay::create(
                Path::new(new_root_path),
                subsystem.writable_etc_overlay(),
            )?)
        } else {
            None
        };
        subsystem.provision(ctx, new_root_path).message(format!(
            "Step 'Provision' failed for subsystem '{}'",
            subsystem.name()
        ))?;
    }
    debug!("Finished step 'Provision'");
    Ok(())
}

fn configure(
    subsystems: &mut [Box<dyn Subsystem>],
    ctx: &EngineContext,
    exec_root: &Path,
) -> Result<(), TridentError> {
    // UKI support currently assumes root verity without a writable overlay. Many module's configure
    // methods would fail in this case, so we skip all of them.
    //
    // TODO: More granular logic for which configure operations to skip. At a minimum,
    // post-configuration scripts should still run. Additionally, errors should be generated for any
    // customizations requested in the Host Configuration that would be skipped.
    if ctx.spec.internal_params.get_flag(ENABLE_UKI_SUPPORT) {
        return Ok(());
    }

    // If verity is present, it means that we are currently doing root
    // verity. For now, we can assume that /etc is readonly, so we setup
    // a writable overlay for it.
    let use_overlay = (ctx.servicing_type == ServicingType::CleanInstall
        || ctx.servicing_type == ServicingType::AbUpdate)
        && !ctx.spec.storage.internal_verity.is_empty();

    info!("Starting step 'Configure'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Configure' for subsystem '{}'",
            subsystem.name()
        );
        // unmount on drop
        let _etc_overlay_mount = if use_overlay {
            Some(etc_overlay::create(
                Path::new("/"),
                subsystem.writable_etc_overlay(),
            )?)
        } else {
            None
        };
        subsystem.configure(ctx, exec_root).message(format!(
            "Step 'Configure' failed for subsystem '{}'",
            subsystem.name()
        ))?;
    }
    debug!("Finished step 'Configure'");

    Ok(())
}

pub fn reboot() -> Result<(), TridentError> {
    // Sync all writes to the filesystem.
    info!("Syncing filesystem");
    nix::unistd::sync();

    // This trace event will be used with the trident_start event to track the
    // total time taken for the reboot
    tracing::info!(metric_name = "trident_system_reboot");
    info!("Rebooting system");
    Dependency::Systemctl
        .cmd()
        .env("SYSTEMD_IGNORE_CHROOT", "true")
        .arg("reboot")
        .run_and_check()
        .structured(ServicingError::Reboot)?;

    thread::sleep(Duration::from_secs(600));

    error!("Waited for reboot for 10 minutes, but nothing happened, aborting");
    Err(TridentError::new(ServicingError::RebootTimeout))
}

#[cfg(test)]
mod tests {
    use super::*;

    use maplit::btreemap;

    use trident_api::config::{
        self, AbUpdate, AbVolumePair, Disk, FileSystemType, Partition, PartitionType,
    };

    #[test]
    fn test_get_root_block_device_path() {
        let ctx = EngineContext {
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
            block_device_paths: btreemap! {
                "foo".to_owned() => PathBuf::from("/dev/sda"),
                "boot".to_owned() => PathBuf::from("/dev/sda1"),
                "root".to_owned() => PathBuf::from("/dev/sda2"),
            },
            ..Default::default()
        };

        assert_eq!(
            get_root_block_device_path(&ctx),
            Some(PathBuf::from("/dev/sda2"))
        );
    }

    /// Validates that the `get_block_device_for_update` function works as expected for
    /// disks, partitions and ab volumes.
    #[test]
    fn test_get_block_device_for_update() {
        let mut ctx = EngineContext {
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
            block_device_paths: btreemap! {
                "os".to_owned() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "rootb".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "data".to_owned() => PathBuf::from("/dev/disk/by-bus/foobar"),
            },
            servicing_type: ServicingType::NoActiveServicing,
            ..Default::default()
        };

        assert_eq!(
            get_block_device_path(&ctx, &"os".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-bus/foobar")
        );
        assert_eq!(
            get_block_device_path(&ctx, &"efi".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp1")
        );
        assert_eq!(
            get_block_device_path(&ctx, &"root".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp2")
        );
        assert_eq!(get_block_device_path(&ctx, &"foobar".to_owned()), None);
        assert_eq!(
            get_block_device_path(&ctx, &"data".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-bus/foobar")
        );

        // Now, set ab_active_volume to VolumeA.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(get_block_device_path(&ctx, &"osab".to_owned()), None);
        assert_eq!(
            get_ab_volume_block_device_id(&ctx, &"osab".to_owned()),
            None
        );

        // Now, set servicing type to AbUpdate.
        ctx.servicing_type = ServicingType::AbUpdate;
        assert_eq!(
            get_block_device_path(&ctx, &"osab".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp3")
        );
        assert_eq!(
            get_ab_volume_block_device_id(&ctx, &"osab".to_owned()),
            Some(&"rootb".to_owned())
        );

        // When active volume is VolumeB, should return VolumeA
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_block_device_path(&ctx, &"osab".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp2")
        );
        assert_eq!(
            get_ab_volume_block_device_id(&ctx, &"osab".to_owned()),
            Some(&"root".to_owned())
        );

        // If target block device id does not exist, should return None.
        assert_eq!(
            get_ab_volume_block_device_id(&ctx, &"non-existent".to_owned()),
            None
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use tempfile::tempdir;

    /// Helper function to check if the persisted background log and metrics
    /// file, i.e. 'trident-<servicingType>-<timeStamp>.log' and
    /// `trident-metrics-<servicingType>-<timeStamp>.jsonl`, exists in the log
    /// directory.
    fn persisted_log_and_metrics_exists(dir: &Path, servicing_type: ServicingType) -> bool {
        let files = fs::read_dir(dir).unwrap();
        let log_prefix = format!("trident-{:?}-", servicing_type);
        let metrics_prefix = format!("trident-metrics-{:?}-", servicing_type);
        let (mut log_found, mut metrics_found) = (false, false);
        for entry in files {
            let entry = entry.unwrap();
            let file_name = entry.file_name().into_string().unwrap();

            // Check if any file starts with the correct prefix
            if file_name.starts_with(&log_prefix) {
                log_found = true;
            } else if file_name.starts_with(&metrics_prefix) {
                metrics_found = true;
            }
            if log_found && metrics_found {
                return true;
            }
        }
        false
    }

    #[functional_test]
    fn test_persist_background_log_and_metrics_success() {
        // Create a tempdir for mock datastore path
        let temp_dir_datastore = tempdir().unwrap();
        let datastore_dir = temp_dir_datastore.path();
        let datastore_path = datastore_dir.join("datastore");

        // Create a tempdir for mock new root path
        let temp_dir_newroot = tempdir().unwrap();
        let newroot_path = temp_dir_newroot.path();

        // Create mock datastore directory and log file
        fs::create_dir_all(&datastore_path).unwrap();

        // Compose the log dir
        let log_dir = join_relative(newroot_path, datastore_dir);
        fs::create_dir_all(&log_dir).unwrap();

        // Persist the background log and metrics file
        let servicing_type = ServicingType::CleanInstall;
        persist_background_log_and_metrics(&datastore_path, Some(newroot_path), servicing_type);

        assert!(
            persisted_log_and_metrics_exists(&log_dir, servicing_type),
            "Trident background log and metrics should be persisted successfully."
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_persist_background_log_and_metrics_failure() {
        // Create a tempdir for mock datastore path
        let temp_dir_datastore = tempdir().unwrap();
        let datastore_dir = temp_dir_datastore.path();
        let datastore_path = datastore_dir.join("datastore");

        // Create mock datastore directory and log file
        fs::create_dir_all(&datastore_path).unwrap();

        // Create a temp copy of TRIDENT_BACKGROUND_LOG_PATH
        let temp_log_path = TRIDENT_BACKGROUND_LOG_PATH.to_owned() + ".temp";
        fs::copy(TRIDENT_BACKGROUND_LOG_PATH, &temp_log_path).unwrap();
        // Remove TRIDENT_BACKGROUND_LOG_PATH
        fs::remove_file(TRIDENT_BACKGROUND_LOG_PATH).unwrap();

        // Create a temp copy of TRIDENT_METRICS_FILE_PATH
        let temp_metrics_path = TRIDENT_METRICS_FILE_PATH.to_owned() + ".temp";
        fs::copy(TRIDENT_METRICS_FILE_PATH, &temp_metrics_path).unwrap();
        // Remove TRIDENT_METRICS_FILE_PATH
        fs::remove_file(TRIDENT_METRICS_FILE_PATH).unwrap();

        // Persist the background log and metrics file
        let servicing_type = ServicingType::AbUpdate;
        persist_background_log_and_metrics(&datastore_path, None, servicing_type);

        assert!(
            !persisted_log_and_metrics_exists(datastore_dir, servicing_type),
            "Trident background log and metrics should not be persisted."
        );

        // Re-create TRIDENT_BACKGROUND_LOG_PATH by copying from the temp file
        fs::copy(&temp_log_path, TRIDENT_BACKGROUND_LOG_PATH).unwrap();

        // Re-create TRIDENT_METRICS_FILE_PATH by copying from the temp file
        fs::copy(&temp_metrics_path, TRIDENT_METRICS_FILE_PATH).unwrap();
    }
}
