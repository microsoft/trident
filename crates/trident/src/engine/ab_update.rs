use std::{path::PathBuf, time::Instant};

use log::{debug, info, warn};

use osutils::{chroot, container};
use trident_api::{
    constants::{internal_params::NO_TRANSITION, ESP_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{HostStatus, ServicingState, ServicingType},
};

use crate::{
    datastore::DataStore,
    engine::{
        self, bootentries,
        storage::{self, verity},
        EngineContext, NewrootMount,
    },
    monitor_metrics,
    subsystems::esp,
    ExitKind,
};

use super::Subsystem;

/// Stages an update. Takes in 3-4 arguments:
/// - subsystems: A mutable reference to the list of subsystems.
/// - ctx: EngineContext.
/// - state: A mutable reference to the DataStore.
///
/// On success, returns an Option<NewrootMount>; This is not null only for A/B updates.
#[tracing::instrument(skip_all, fields(servicing_type = format!("{:?}", ctx.servicing_type)))]
pub(super) fn stage_update(
    subsystems: &mut [Box<dyn Subsystem>],
    mut ctx: EngineContext,
    state: &mut DataStore,
) -> Result<(), TridentError> {
    if ctx.servicing_type != ServicingType::AbUpdate {
        return Err(TridentError::internal(
            "A/B update staging called for unsupported servicing type",
        ));
    }
    info!("Staging A/B update");

    // Best effort to measure memory, CPU, and network usage during execution
    let monitor = match monitor_metrics::MonitorMetrics::new("stage_ab_update".to_string()) {
        Ok(monitor) => Some(monitor),
        Err(e) => {
            warn!("Failed to create metrics monitor: {e:?}");
            None
        }
    };

    engine::prepare(subsystems, &ctx)?;

    debug!("Preparing storage to mount new root");

    // Close any pre-existing verity devices
    verity::stop_trident_servicing_devices(&ctx.spec).structured(ServicingError::CleanupVerity)?;

    storage::initialize_block_devices(&ctx)?;
    let newroot_mount = NewrootMount::create_and_mount(
        &ctx.spec,
        &ctx.partition_paths,
        ctx.get_ab_update_volume()
            .structured(InternalError::Internal(
                "No update volume despite there being an A/B update in progress",
            ))?,
    )?;

    engine::provision(subsystems, &ctx, newroot_mount.path())?;

    debug!("Entering '{}' chroot", newroot_mount.path().display());
    let result = chroot::enter_update_chroot(newroot_mount.path())
        .message("Failed to enter chroot")?
        .execute_and_exit(|| engine::configure(subsystems, &ctx));

    if let Err(original_error) = result {
        if let Err(e) = newroot_mount.unmount_all() {
            warn!("While handling an earlier error: {e:?}");
        }
        return Err(original_error).message("Failed to execute in chroot");
    }

    newroot_mount.unmount_all()?;

    // Update the Host Configuration with information produced and stored in the
    // subsystems. Currently, this step is used only to update the final paths
    // of sysexts and confexts configured in the extensions subsystem.
    engine::update_host_configuration(subsystems, &mut ctx)?;
    // Turn ctx into an immutable variable.
    let ctx = ctx;

    engine::clean_up(subsystems, &ctx)?;

    // At this point, deployment has been staged, so update servicing state
    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::AbUpdateStaged
    );
    state.with_host_status(|hs| {
        *hs = HostStatus {
            spec: ctx.spec,
            spec_old: ctx.spec_old,
            servicing_state: ServicingState::AbUpdateStaged,
            ab_active_volume: ctx.ab_active_volume,
            partition_paths: ctx.partition_paths,
            disk_uuids: ctx.disk_uuids,
            install_index: ctx.install_index,
            last_error: None,
            is_management_os: false,
        };
    })?;

    if let Some(mut monitor) = monitor {
        // If the monitor was created successfully, stop it after execution
        if let Err(e) = monitor.stop() {
            warn!("Failed to stop metrics monitor: {e:?}");
        }
    }

    info!("Staging of A/B update succeeded");

    Ok(())
}

/// Finalizes an update. Takes in 2-3 arguments:
/// - state: A mutable reference to the DataStore.
/// - update_start_time: The time at which the update began staging.
#[tracing::instrument(skip_all, fields(servicing_type = format!("{:?}", ServicingType::AbUpdate)))]
pub(crate) fn finalize_update(
    state: &mut DataStore,
    update_start_time: Option<Instant>,
) -> Result<ExitKind, TridentError> {
    info!("Finalizing A/B update");

    if state.host_status().servicing_state != ServicingState::AbUpdateStaged {
        return Err(TridentError::internal(
            "A/B update must be staged before calling finalize",
        ));
    }

    let ctx = EngineContext {
        spec: state.host_status().spec.clone(),
        spec_old: state.host_status().spec_old.clone(),
        servicing_type: ServicingType::AbUpdate,
        ab_active_volume: state.host_status().ab_active_volume,
        partition_paths: state.host_status().partition_paths.clone(),
        disk_uuids: state.host_status().disk_uuids.clone(),
        install_index: state.host_status().install_index,
        image: None, // Not used in finalize_update
        storage_graph: engine::build_storage_graph(&state.host_status().spec.storage)?, // Build storage graph
        filesystems: Vec::new(), // Left empty since context does not have image
        is_uki: None,
    };

    let root_path = container::get_host_relative_path(PathBuf::from(ROOT_MOUNT_POINT_PATH))?;
    let esp_path = container::get_host_relative_path(PathBuf::from(ESP_MOUNT_POINT_PATH))?;
    bootentries::create_and_update_boot_variables(&ctx, &esp_path)?;
    // Analogous to how UEFI variables are configured, finalize must start configuring
    // UEFI fallback, and a successful commit will finish it.
    esp::set_uefi_fallback_contents(&ctx, ServicingState::AbUpdateStaged, &root_path)
        .structured(ServicingError::SetUpUefiFallback)?;

    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::AbUpdateFinalized
    );
    state.with_host_status(|status| status.servicing_state = ServicingState::AbUpdateFinalized)?;
    state.close();

    // Metric for update time in seconds
    if let Some(start_time) = update_start_time {
        tracing::info!(
            metric_name = "update_time_secs",
            value = start_time.elapsed().as_secs_f64(),
            servicing_type = format!("{:?}", ServicingType::AbUpdate)
        );
    }

    // Persist the Trident background log and metrics file to the updated target OS
    engine::persist_background_log_and_metrics(
        &state.host_status().spec.trident.datastore_path,
        None,
        state.host_status().servicing_state,
    );

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
