use std::time::Instant;

use log::{debug, info, warn};

use trident_api::{
    config::{HostConfiguration, Operations},
    constants::internal_params::ENABLE_UKI_SUPPORT,
    error::{InternalError, InvalidInputError, TridentError, TridentResultExt},
    status::{ServicingState, ServicingType},
};

use crate::{
    datastore::DataStore,
    engine::{self, ab_update, rollback, runtime_update, EngineContext, SUBSYSTEMS},
    osimage::OsImage,
    subsystems::hooks::HooksSubsystem,
    ExitKind,
};

#[tracing::instrument(skip_all)]
pub(crate) fn update(
    host_config: &HostConfiguration,
    state: &mut DataStore,
    allowed_operations: &Operations,
    image: OsImage,
) -> Result<ExitKind, TridentError> {
    info!("Starting update");
    let mut subsystems = SUBSYSTEMS.lock().unwrap();

    // Need to re-set the Host Status in case another update has been previously staged.
    if matches!(
        state.host_status().servicing_state,
        ServicingState::AbUpdateStaged | ServicingState::RuntimeUpdateStaged
    ) {
        debug!(
            "Resetting '{:?}' state",
            state.host_status().servicing_state
        );
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
        is_uki: Some(image.is_uki() || host_config.internal_params.get_flag(ENABLE_UKI_SUPPORT)),
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
    let servicing_type = engine::select_servicing_type(&subsystems, &ctx)?;
    match servicing_type {
        ServicingType::NoActiveServicing => {
            info!("No update servicing required");
            return Ok(ExitKind::Done);
        }
        ServicingType::RuntimeUpdate => {}
        ServicingType::AbUpdate => {
            // Execute pre-servicing scripts
            HooksSubsystem::new_for_local_scripts().execute_pre_servicing_scripts(&ctx)?;
        }
        ServicingType::ManualRollback => {
            return Err(TridentError::new(InternalError::Internal(
                "Subsystem reported manual rollback servicing type",
            )));
        }
        ServicingType::CleanInstall => {
            return Err(TridentError::new(InternalError::Internal(
                "Subsystem reported clean install servicing type",
            )));
        }
    }

    debug!(
        "Update of servicing type '{:?}' is required",
        servicing_type
    );

    ctx.servicing_type = servicing_type;

    engine::validate_host_config(&subsystems, &ctx)?;

    ctx.populate_filesystems()?;

    let update_start_time = Instant::now();
    tracing::info!(
        metric_name = "update_start",
        servicing_type = format!("{:?}", servicing_type),
        servicing_state = format!("{:?}", state.host_status().servicing_state),
    );

    match servicing_type {
        ServicingType::AbUpdate => {
            // Stage update.
            ab_update::stage_update(&mut subsystems, ctx, state)
                .message("Failed to stage A/B update")?;

            // Determine if finalize is required or not.
            if !allowed_operations.has_finalize() {
                info!("Finalizing of A/B update not requested, skipping reboot");

                // Persist the Trident background log and metrics file to the
                // target OS. Otherwise, the staging logs would be lost.
                engine::persist_background_log_and_metrics(
                    &state.host_status().spec.trident.datastore_path,
                    None,
                    state.host_status().servicing_state,
                );
                Ok(ExitKind::Done)
            } else {
                ab_update::finalize_update(state, Some(update_start_time))
                    .message("Failed to finalize A/B update")
            }
        }
        ServicingType::RuntimeUpdate => {
            // Stage update.
            runtime_update::stage_update(&mut subsystems, ctx, state)
                .message("Failed to stage runtime update")?;

            // Determine if finalize is required or not.
            if !allowed_operations.has_finalize() {
                info!("Finalizing of runtime update not requested.");
                // Persist the Trident background log and metrics file to the target OS. Otherwise,
                // the staging logs would be lost.
                engine::persist_background_log_and_metrics(
                    &state.host_status().spec.trident.datastore_path,
                    None,
                    state.host_status().servicing_state,
                );
                Ok(ExitKind::Done)
            } else {
                runtime_update::finalize_update(&mut subsystems, state, Some(update_start_time))
            }
        }
        ServicingType::CleanInstall => Err(TridentError::new(
            InvalidInputError::CleanInstallOnProvisionedHost,
        )),
        ServicingType::ManualRollback => Err(TridentError::new(InternalError::Internal(
            "Cannot update during manual rollback",
        ))),
        ServicingType::NoActiveServicing => Err(TridentError::new(InternalError::Internal(
            "No active servicing type",
        ))),
    }
}
