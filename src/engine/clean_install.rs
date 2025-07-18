use std::{
    fs,
    path::{Path, PathBuf},
    sync::MutexGuard,
    time::Instant,
};

use log::{debug, error, info, warn};
#[cfg(feature = "grpc-dangerous")]
use tokio::sync::mpsc;

use osutils::{chroot, container, mount, mountpoint, path::join_relative};
use trident_api::{
    config::{HostConfiguration, Operations},
    constants::{
        internal_params::NO_TRANSITION, ESP_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH,
        UPDATE_ROOT_PATH,
    },
    error::{
        InitializationError, InternalError, InvalidInputError, ReportError, ServicingError,
        TridentError, TridentResultExt,
    },
    status::{AbVolumeSelection, HostStatus, ServicingState, ServicingType},
};

use crate::{
    datastore::DataStore,
    engine::{self, bootentries, install_index, storage, EngineContext, SUBSYSTEMS},
    monitor_metrics,
    osimage::OsImage,
    subsystems::hooks::HooksSubsystem,
    ExitKind, SAFETY_OVERRIDE_CHECK_PATH,
};
#[cfg(feature = "grpc-dangerous")]
use crate::{grpc, GrpcSender};

use super::{NewrootMount, Subsystem};

#[tracing::instrument(skip_all)]
pub(crate) fn clean_install(
    host_config: &HostConfiguration,
    state: &mut DataStore,
    allowed_operations: &Operations,
    multiboot: bool,
    image: OsImage,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<GrpcSender>,
) -> Result<ExitKind, TridentError> {
    info!("Starting clean install");
    tracing::info!(metric_name = "clean_install_start", value = true);
    let clean_install_start_time = Instant::now();

    if Path::new(UPDATE_ROOT_PATH).exists()
        && mountpoint::check_is_mountpoint(UPDATE_ROOT_PATH).structured(
            ServicingError::CheckIfMountPoint {
                path: UPDATE_ROOT_PATH.to_string(),
            },
        )?
    {
        debug!("Unmounting volumes from earlier runs of Trident");
        if let Err(e) = mount::umount(UPDATE_ROOT_PATH, true) {
            warn!("Attempt to unmount '{UPDATE_ROOT_PATH}' returned error: {e}",);
        }
    }

    // This is a safety check so that nobody accidentally formats their dev
    // machine.
    debug!("Performing safety check for clean install");
    clean_install_safety_check(host_config, multiboot)?;
    info!("Safety check passed");

    let mut subsystems = SUBSYSTEMS.lock().unwrap();

    // Stage clean install
    let root_mount = stage_clean_install(
        &mut subsystems,
        state,
        host_config,
        image,
        #[cfg(feature = "grpc-dangerous")]
        sender,
    )?;

    if !allowed_operations.has_finalize() {
        info!("Finalizing of clean install not requested, skipping finalizing and reboot");
        state.close();

        // Persist the Trident background log and metrics file to the new root. Otherwise, the
        // staging logs would be lost.
        engine::persist_background_log_and_metrics(
            &state.host_status().spec.trident.datastore_path,
            Some(root_mount.path()),
            state.host_status().servicing_state,
        );

        debug!("Unmounting '{}'", root_mount.path().display());
        root_mount.unmount_all()?;
        Ok(ExitKind::Done)
    } else {
        finalize_clean_install(
            state,
            Some(root_mount),
            Some(clean_install_start_time),
            #[cfg(feature = "grpc-dangerous")]
            sender,
        )
    }
}

/// Performs a safety check to ensure that the clean install can proceed.
fn clean_install_safety_check(
    host_config: &HostConfiguration,
    multiboot: bool,
) -> Result<(), TridentError> {
    // Check if Trident is running from a live image
    let cmdline =
        fs::read_to_string("/proc/cmdline").structured(InitializationError::ReadCmdline)?;
    if cmdline.contains("root=/dev/ram0") || cmdline.contains("root=live:LABEL=CDROM") {
        debug!("Trident is running from a live image.");
        return Ok(());
    }

    warn!("Trident is running from an OS installed on persistent storage");

    // To go past this point in the safety check we NEED multiboot
    if !multiboot {
        return Err(TridentError::new(
            InvalidInputError::CleanInstallOnProvisionedHost,
        ))
        .message("Running Trident from persistent storage without multiboot flag");
    }

    // Check if we have adopted partitions in the host config
    if host_config.has_adopted_partitions() {
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
    image: OsImage,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<NewrootMount, TridentError> {
    // Best effort to measure memory, CPU, and network usage during execution
    let monitor = match monitor_metrics::MonitorMetrics::new("stage_clean_install".to_string()) {
        Ok(monitor) => Some(monitor),
        Err(e) => {
            warn!("Failed to create metrics monitor: {e:?}");
            None
        }
    };

    // Initialize a copy of the Host Status with the changes that are planned. We make a copy
    // rather than modifying the datastore's version so that we can wait until the clean install is
    // staged before committing the changes.
    let mut ctx = EngineContext {
        spec: host_config.clone(),
        spec_old: Default::default(),
        servicing_type: ServicingType::CleanInstall,
        ab_active_volume: None,
        partition_paths: Default::default(), // Will be initialized later
        disk_uuids: Default::default(),      // Will be initialized later
        install_index: 0,                    // Will be initialized later
        is_uki: Some(image.is_uki()),
        image: Some(image),
        storage_graph: engine::build_storage_graph(&host_config.storage)?, // Build storage graph
        filesystems: Vec::new(), // Will be populated after dynamic validation
    };

    // Execute pre-servicing scripts
    HooksSubsystem::default().execute_pre_servicing_scripts(&ctx)?;

    engine::validate_host_config(subsystems, &ctx)?;

    ctx.populate_filesystems()?;

    // Need to re-set saved Host Status in case another clean install has been previously staged
    debug!("Clearing saved Host Status");
    state.with_host_status(|host_status| {
        host_status.spec = Default::default();
        host_status.servicing_state = ServicingState::NotProvisioned;
    })?;
    #[cfg(feature = "grpc-dangerous")]
    grpc::send_host_status_state(sender, state)?;

    engine::prepare(subsystems, &ctx)?;

    debug!("Preparing storage to mount new root");
    storage::create_block_devices(&mut ctx)?;
    storage::initialize_block_devices(&ctx)?;
    let newroot_mount = NewrootMount::create_and_mount(
        host_config,
        &ctx.partition_paths,
        AbVolumeSelection::VolumeA,
    )?;
    ctx.install_index = install_index::next_install_index(newroot_mount.path())?;

    engine::provision(subsystems, &ctx, newroot_mount.path())?;

    debug!("Entering '{}' chroot", newroot_mount.path().display());
    let result = chroot::enter_update_chroot(newroot_mount.path())
        .message("Failed to enter chroot")?
        .execute_and_exit(|| engine::configure(subsystems, &ctx));

    if let Some(mut monitor) = monitor {
        // If the monitor was created successfully, stop it after execution
        if let Err(e) = monitor.stop() {
            warn!("Failed to stop metrics monitor: {e:?}");
        }
    }

    if let Err(original_error) = result {
        if let Err(e) = newroot_mount.unmount_all() {
            warn!("While handling an earlier error: {e:?}");
        }
        return Err(original_error).message("Failed to execute in chroot");
    }

    // At this point, clean install has been staged, so update Host Status
    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::CleanInstallStaged
    );
    state.with_host_status(|hs| {
        *hs = HostStatus {
            servicing_state: ServicingState::CleanInstallStaged,
            spec: host_config.clone(),
            spec_old: Default::default(),
            ab_active_volume: None,
            partition_paths: ctx.partition_paths,
            disk_uuids: ctx.disk_uuids,
            install_index: ctx.install_index,
            last_error: None,
            is_management_os: true,
        }
    })?;
    #[cfg(feature = "grpc-dangerous")]
    grpc::send_host_status_state(sender, state)?;

    info!("Staging of clean install succeeded");
    Ok(newroot_mount)
}

/// Finalizes a clean install. Takes in 4 arguments:
/// - state: A mutable reference to the DataStore.
/// - new_root_path: New root device path. If None, a new root is created and mounted.
/// - clean_install_start_time: Optional instant when clean install started.
/// - sender: Optional mutable reference to the gRPC sender.
#[tracing::instrument(skip_all)]
pub(crate) fn finalize_clean_install(
    state: &mut DataStore,
    new_root: Option<NewrootMount>,
    clean_install_start_time: Option<Instant>,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<GrpcSender>,
) -> Result<ExitKind, TridentError> {
    info!("Finalizing clean install");

    let ctx = EngineContext {
        spec: state.host_status().spec.clone(),
        spec_old: state.host_status().spec_old.clone(),
        servicing_type: ServicingType::CleanInstall,
        ab_active_volume: state.host_status().ab_active_volume,
        partition_paths: state.host_status().partition_paths.clone(),
        disk_uuids: state.host_status().disk_uuids.clone(),
        install_index: state.host_status().install_index,
        image: None, // Not used in finalize_clean_install
        storage_graph: engine::build_storage_graph(&state.host_status().spec.storage)?, // Build storage graph
        filesystems: Vec::new(), // Left empty since context does not have image
        is_uki: None,
    };

    let new_root = match new_root {
        Some(new_root) => new_root,
        None => NewrootMount::create_and_mount(
            &ctx.spec,
            &ctx.partition_paths,
            ctx.get_ab_update_volume()
                .structured(InternalError::Internal(
                    "No update volume despite there being a clean install in progress",
                ))?,
        )?,
    };

    // On clean install, need to verify that AZLA entry exists in /mnt/newroot/boot/efi
    let esp_path = join_relative(new_root.path(), ESP_MOUNT_POINT_PATH);
    bootentries::create_and_update_boot_variables(&ctx, &esp_path)?;

    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::CleanInstallFinalized
    );
    state.with_host_status(|status| {
        status.servicing_state = ServicingState::CleanInstallFinalized
    })?;
    #[cfg(feature = "grpc-dangerous")]
    grpc::send_host_status_state(sender, state)?;

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
    engine::persist_background_log_and_metrics(
        &state.host_status().spec.trident.datastore_path,
        Some(new_root.path()),
        state.host_status().servicing_state,
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
        Ok(ExitKind::NeedsReboot)
    } else {
        warn!(
            "Skipping reboot as requested by internal parameter '{}'",
            NO_TRANSITION
        );
        Ok(ExitKind::Done)
    }
}
