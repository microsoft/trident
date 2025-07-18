use std::{path::PathBuf, time::Instant};

use log::{debug, info, warn};
#[cfg(feature = "grpc-dangerous")]
use tokio::sync::mpsc;

use osutils::{chroot, container, path::join_relative};
use trident_api::{
    config::{HostConfiguration, Operations},
    constants::{internal_params::NO_TRANSITION, ESP_MOUNT_POINT_PATH},
    error::{
        InternalError, InvalidInputError, ReportError, ServicingError, TridentError,
        TridentResultExt,
    },
    status::{HostStatus, ServicingState, ServicingType},
};

use crate::{
    datastore::DataStore,
    engine::{
        self, bootentries, rollback,
        storage::{self, verity},
        EngineContext, NewrootMount, SUBSYSTEMS,
    },
    monitor_metrics,
    osimage::OsImage,
    subsystems::hooks::HooksSubsystem,
    ExitKind,
};
#[cfg(feature = "grpc-dangerous")]
use crate::{grpc, GrpcSender};

use super::Subsystem;

#[tracing::instrument(skip_all)]
pub(crate) fn update(
    host_config: &HostConfiguration,
    state: &mut DataStore,
    allowed_operations: &Operations,
    image: OsImage,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<GrpcSender>,
) -> Result<ExitKind, TridentError> {
    info!("Starting update");
    let mut subsystems = SUBSYSTEMS.lock().unwrap();

    if state.host_status().servicing_state == ServicingState::AbUpdateStaged {
        // Need to re-set the Host Status in case another update has been previously staged
        debug!("Resetting A/B update state");
        state.with_host_status(|host_status| {
            host_status.spec = host_status.spec_old.clone();
            host_status.spec_old = Default::default();
            host_status.servicing_state = ServicingState::Provisioned;
        })?;
    }

    let mut ctx = EngineContext {
        spec: host_config.clone(),
        spec_old: state.host_status().spec.clone(),
        servicing_type: ServicingType::NoActiveServicing,
        partition_paths: state.host_status().partition_paths.clone(),
        ab_active_volume: state.host_status().ab_active_volume,
        disk_uuids: state.host_status().disk_uuids.clone(),
        install_index: state.host_status().install_index,
        is_uki: Some(image.is_uki()),
        image: Some(image),
        storage_graph: engine::build_storage_graph(&host_config.storage)?, // Build storage graph
        filesystems: Vec::new(), // Will be populated after dynamic validation
    };

    // Before starting an update servicing, need to validate that the active volume is set
    // correctly.
    if ctx.spec.storage.ab_update.is_some() {
        debug!(
            "A/B update is enabled, validating that '{:?}' is currently active",
            ctx.ab_active_volume
                .map_or("None".to_string(), |v| v.to_string())
        );
        rollback::validate_ab_active_volume(&ctx)?;
    }

    debug!("Determining servicing type of required update");
    let servicing_type = subsystems
        .iter()
        .map(|m| m.select_servicing_type(&ctx))
        .collect::<Result<Vec<_>, TridentError>>()?
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(ServicingType::NoActiveServicing); // Never None b/c select_servicing_type() returns a value
    if servicing_type == ServicingType::NoActiveServicing {
        info!("No update servicing required");
        return Ok(ExitKind::Done);
    }
    debug!(
        "Update of servicing type '{:?}' is required",
        servicing_type
    );

    ctx.servicing_type = servicing_type;

    // Execute pre-servicing scripts
    HooksSubsystem::default().execute_pre_servicing_scripts(&ctx)?;

    engine::validate_host_config(&subsystems, &ctx)?;

    ctx.populate_filesystems()?;

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
        sender,
    )
    .message("Failed to stage update")?;

    // When enabled, notify Harpoon that the installation of the update has
    // finalized.
    crate::harpoon_hc::on_harpoon_enabled_event(
        host_config,
        harpoon::EventType::Install,
        harpoon::EventResult::Success,
    );

    match servicing_type {
        ServicingType::UpdateAndReboot | ServicingType::AbUpdate => {
            if !allowed_operations.has_finalize() {
                info!("Finalizing of update not requested, skipping reboot");

                // Persist the Trident background log and metrics file to the new root. Otherwise,
                // the staging logs would be lost.
                engine::persist_background_log_and_metrics(
                    &state.host_status().spec.trident.datastore_path,
                    None,
                    state.host_status().servicing_state,
                );
                Ok(ExitKind::Done)
            } else {
                finalize_update(
                    state,
                    servicing_type,
                    Some(update_start_time),
                    #[cfg(feature = "grpc-dangerous")]
                    sender,
                )
                .message("Failed to finalize update")
            }
        }
        ServicingType::NormalUpdate | ServicingType::HotPatch => {
            state.with_host_status(|host_status| {
                host_status.servicing_state = ServicingState::Provisioned;
            })?;
            #[cfg(feature = "grpc-dangerous")]
            grpc::send_host_status_state(sender, state)?;

            // Persist the Trident background log and metrics file to the updated runtime OS
            engine::persist_background_log_and_metrics(
                &state.host_status().spec.trident.datastore_path,
                None,
                state.host_status().servicing_state,
            );

            info!("Update of servicing type '{:?}' succeeded", servicing_type);
            Ok(ExitKind::Done)
        }
        ServicingType::CleanInstall => Err(TridentError::new(
            InvalidInputError::CleanInstallOnProvisionedHost,
        )),
        ServicingType::NoActiveServicing => Err(TridentError::internal("No active servicing type")),
    }
}

/// Stages an update. Takes in 3-4 arguments:
/// - subsystems: A mutable reference to the list of subsystems.
/// - ctx: EngineContext.
/// - state: A mutable reference to the DataStore.
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
        ServicingType::CleanInstall => {
            return Err(TridentError::new(
                InvalidInputError::CleanInstallOnProvisionedHost,
            ));
        }
        ServicingType::NoActiveServicing => {
            return Err(TridentError::internal("No active servicing type"))
        }
        _ => {
            info!(
                "Staging update of servicing type '{:?}'",
                ctx.servicing_type
            )
        }
    }

    // Best effort to measure memory, CPU, and network usage during execution
    let monitor = match monitor_metrics::MonitorMetrics::new("stage_update".to_string()) {
        Ok(monitor) => Some(monitor),
        Err(e) => {
            warn!("Failed to create metrics monitor: {e:?}");
            None
        }
    };

    engine::prepare(subsystems, &ctx)?;

    if let ServicingType::AbUpdate = ctx.servicing_type {
        debug!("Preparing storage to mount new root");

        // Close any pre-existing verity devices
        verity::stop_trident_servicing_devices(&ctx.spec)
            .structured(ServicingError::CleanupVerity)?;

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
    } else {
        engine::configure(subsystems, &ctx)?;
    };

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
    #[cfg(feature = "grpc-dangerous")]
    grpc::send_host_status_state(sender, state)?;

    if let Some(mut monitor) = monitor {
        // If the monitor was created successfully, stop it after execution
        if let Err(e) = monitor.stop() {
            warn!("Failed to stop metrics monitor: {e:?}");
        }
    }

    info!("Staging of update '{:?}' succeeded", ctx.servicing_type);

    Ok(())
}

/// Finalizes an update. Takes in 2 arguments:
/// - state: A mutable reference to the DataStore.
/// - sender: Optional mutable reference to the gRPC sender.
#[tracing::instrument(skip_all, fields(servicing_type = format!("{:?}", servicing_type)))]
pub(crate) fn finalize_update(
    state: &mut DataStore,
    servicing_type: ServicingType,
    update_start_time: Option<Instant>,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<ExitKind, TridentError> {
    info!("Finalizing update");

    if servicing_type != ServicingType::AbUpdate {
        return Err(TridentError::internal(
            "Unimplemented servicing type for finalize",
        ));
    }

    let ctx = EngineContext {
        spec: state.host_status().spec.clone(),
        spec_old: state.host_status().spec_old.clone(),
        servicing_type,
        ab_active_volume: state.host_status().ab_active_volume,
        partition_paths: state.host_status().partition_paths.clone(),
        disk_uuids: state.host_status().disk_uuids.clone(),
        install_index: state.host_status().install_index,
        image: None, // Not used in finalize_update
        storage_graph: engine::build_storage_graph(&state.host_status().spec.storage)?, // Build storage graph
        filesystems: Vec::new(), // Left empty since context does not have image
        is_uki: None,
    };

    let esp_path = if container::is_running_in_container()
        .message("Failed to check if Trident is running in a container")?
    {
        let host_root = container::get_host_root_path().message("Failed to get host root path")?;
        join_relative(host_root, ESP_MOUNT_POINT_PATH)
    } else {
        PathBuf::from(ESP_MOUNT_POINT_PATH)
    };
    bootentries::create_and_update_boot_variables(&ctx, &esp_path)?;

    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::AbUpdateFinalized
    );
    state.with_host_status(|status| status.servicing_state = ServicingState::AbUpdateFinalized)?;
    #[cfg(feature = "grpc-dangerous")]
    grpc::send_host_status_state(sender, state)?;
    state.close();

    // Metric for update time in seconds
    if let Some(start_time) = update_start_time {
        tracing::info!(
            metric_name = "update_time_secs",
            value = start_time.elapsed().as_secs_f64(),
            servicing_type = format!("{:?}", servicing_type)
        );
    }

    // Persist the Trident background log and metrics file to the updated runtime OS
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
