use std::time::Instant;

use log::{debug, info, warn};
#[cfg(feature = "grpc-dangerous")]
use tokio::sync::mpsc;

use trident_api::{
    error::{InvalidInputError, TridentError},
    status::{HostStatus, ServicingState, ServicingType},
};

#[cfg(feature = "grpc-dangerous")]
use crate::grpc;
use crate::{
    datastore::DataStore,
    engine::{self, EngineContext},
    monitor_metrics, ExitKind,
};

use super::Subsystem;

/// Stages a runtime update. Takes in 3-4 arguments:
/// - subsystems: A mutable reference to the list of subsystems.
/// - ctx: EngineContext.
/// - state: A mutable reference to the DataStore.
/// - sender: Optional mutable reference to the gRPC sender.
///
/// On success, returns an Option<NewrootMount>; This is not null only for A/B updates.
#[tracing::instrument(skip_all, fields(servicing_type = format!("{:?}", ctx.servicing_type)))]
pub(crate) fn stage_update(
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
    subsystems: &mut [Box<dyn Subsystem>],
    state: &mut DataStore,
    servicing_type: ServicingType,
    update_start_time: Option<Instant>,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<ExitKind, TridentError> {
    info!("Finalizing runtime update");

    if servicing_type != ServicingType::RuntimeUpdate {
        return Err(TridentError::internal(
            "Unimplemented servicing type for finalize",
        ));
    }

    let mut ctx = EngineContext {
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

    // Note: provision() is not called during runtime updates.
    engine::configure(subsystems, &ctx)?;

    // Update the Host Configuration with information produced and stored in the
    // subsystems. Currently, this step is used only to update the final paths
    // of sysexts and confexts configured in the extensions subsystem.
    engine::update_host_configuration(subsystems, &mut ctx)?;
    // Turn ctx into an immutable variable.
    let ctx = ctx;

    engine::clean_up(subsystems, &ctx)?;

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

    // Persist the Trident background log and metrics file to the updated target OS
    engine::persist_background_log_and_metrics(
        &state.host_status().spec.trident.datastore_path,
        None,
        state.host_status().servicing_state,
    );

    Ok(ExitKind::Done)
}
