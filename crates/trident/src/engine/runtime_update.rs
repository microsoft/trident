use std::time::Instant;

use log::{debug, info, trace, warn};
#[cfg(feature = "grpc-dangerous")]
use tokio::sync::mpsc;

use osutils::efivar;
use trident_api::{
    error::TridentError,
    status::{ServicingState, ServicingType},
};

#[cfg(feature = "grpc-dangerous")]
use crate::grpc;
use crate::{
    datastore::DataStore,
    engine::{self, EngineContext},
    health, monitor_metrics, ExitKind,
};

use super::Subsystem;

/// Stages a runtime update. Takes in 3-4 arguments:
/// - subsystems: A mutable reference to the list of subsystems.
/// - ctx: EngineContext.
/// - state: A mutable reference to the DataStore.
/// - sender: Optional mutable reference to the gRPC sender.
#[tracing::instrument(skip_all, fields(servicing_type = format!("{:?}", ServicingType::RuntimeUpdate)))]
pub(crate) fn stage_update(
    subsystems: &mut [Box<dyn Subsystem>],
    ctx: EngineContext,
    state: &mut DataStore,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<(), TridentError> {
    if ctx.servicing_type != ServicingType::RuntimeUpdate {
        return Err(TridentError::internal(
            "Runtime update staging called for unsupported servicing type",
        ));
    }
    info!("Staging runtime update");

    // Best effort to measure memory, CPU, and network usage during execution
    let monitor = match monitor_metrics::MonitorMetrics::new("stage_runtime_update".to_string()) {
        Ok(monitor) => Some(monitor),
        Err(e) => {
            warn!("Failed to create metrics monitor: {e:?}");
            None
        }
    };

    engine::prepare(subsystems, &ctx)?;

    // At this point, the runtime update has been staged, so update servicing state
    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::RuntimeUpdateStaged
    );
    state.with_host_status(|hs| {
        hs.servicing_state = ServicingState::RuntimeUpdateStaged;
        // Update spec inside the Host Status with the new Host Configuration
        // (stored in ctx.spec).
        hs.spec = ctx.spec;
        hs.spec_old = ctx.spec_old;
    })?;
    #[cfg(feature = "grpc-dangerous")]
    grpc::send_host_status_state(sender, state)?;

    if let Some(mut monitor) = monitor {
        // If the monitor was created successfully, stop it after execution
        if let Err(e) = monitor.stop() {
            warn!("Failed to stop metrics monitor: {e:?}");
        }
    }

    info!("Staging of runtime update succeeded");

    Ok(())
}

/// Finalizes a runtime update. Takes in 5-6 arguments:
/// - subsystems: A mutable reference to the list of subsystems.
/// - state: A mutable reference to the DataStore.
/// - reverse_specs: A boolean indicating whether spec and spec_old in the
///   EngineContext should be reversed. This is used for auto-rollback of
///   runtime updates.
/// - run_health_checks: A boolean indicating whether health checks should be
///   performed before exiting.
/// - update_start_time: Optional, the time at which the update staging began.
/// - sender: Optional mutable reference to the gRPC sender.
#[tracing::instrument(skip_all, fields(servicing_type = format!("{:?}", ServicingType::RuntimeUpdate)))]
pub(crate) fn finalize_update(
    subsystems: &mut [Box<dyn Subsystem>],
    state: &mut DataStore,
    reverse_specs: bool,
    run_health_checks: bool,
    update_start_time: Option<Instant>,
    #[cfg(feature = "grpc-dangerous")] sender: &mut Option<
        mpsc::UnboundedSender<Result<grpc::HostStatusState, tonic::Status>>,
    >,
) -> Result<ExitKind, TridentError> {
    let target_spec;
    let old_spec;
    if !reverse_specs {
        info!("Finalizing runtime update");
        target_spec = state.host_status().spec.clone();
        old_spec = state.host_status().spec_old.clone();
    } else {
        info!("Starting rollback of runtime update");
        trace!("Reversing spec and spec_old");
        target_spec = state.host_status().spec_old.clone();
        old_spec = state.host_status().spec.clone();
    }

    if state.host_status().servicing_state != ServicingState::RuntimeUpdateStaged {
        return Err(TridentError::internal(
            "Runtime update must be staged before calling finalize",
        ));
    }

    let mut ctx = EngineContext {
        spec: target_spec,
        spec_old: old_spec,
        servicing_type: ServicingType::RuntimeUpdate,
        ab_active_volume: state.host_status().ab_active_volume,
        partition_paths: state.host_status().partition_paths.clone(),
        disk_uuids: state.host_status().disk_uuids.clone(),
        install_index: state.host_status().install_index,
        is_uki: Some(efivar::current_var_is_uki()),
        image: None,
        storage_graph: engine::build_storage_graph(&state.host_status().spec.storage)?, // Build storage graph
        filesystems: Vec::new(), // Left empty since not needed for finalizing runtime update.
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

    // Run health checks if we are performing a runtime update (skip if we are
    // rolling back)
    if run_health_checks {
        health::execute_health_checks(&ctx)?;
    }

    debug!(
        "Updating host's servicing state to '{:?}'",
        ServicingState::Provisioned
    );
    state.with_host_status(|hs| {
        hs.servicing_state = ServicingState::Provisioned;
        hs.spec = ctx.spec; // Update spec after call to engine::update_host_configuration()
        hs.spec_old = Default::default(); // Clear spec_old now that state is Provisioned
    })?;
    #[cfg(feature = "grpc-dangerous")]
    grpc::send_host_status_state(sender, state)?;
    state.close();

    // Metric for update time in seconds
    if let Some(start_time) = update_start_time {
        tracing::info!(
            metric_name = "update_time_secs",
            value = start_time.elapsed().as_secs_f64(),
            servicing_type = format!("{:?}", ServicingType::RuntimeUpdate)
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
