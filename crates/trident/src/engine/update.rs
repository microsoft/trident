use std::time::Instant;

use log::{debug, info, warn};

use trident_api::{
    config::{HostConfiguration, Operations},
    constants::internal_params::ENABLE_UKI_SUPPORT,
    error::{InvalidInputError, TridentError, TridentResultExt},
    status::{ServicingState, ServicingType},
};

#[cfg(feature = "grpc-dangerous")]
use crate::GrpcSender;
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
        is_uki: Some(image.is_uki() || host_config.internal_params.get_flag(ENABLE_UKI_SUPPORT)),
        image: Some(image.clone()),
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
        .max()
        .unwrap_or(ServicingType::NoActiveServicing);
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
    HooksSubsystem::new_for_local_scripts().execute_pre_servicing_scripts(&ctx)?;

    engine::validate_host_config(&subsystems, &ctx)?;

    ctx.populate_filesystems()?;

    let update_start_time = Instant::now();
    tracing::info!(
        metric_name = "update_start",
        servicing_type = format!("{:?}", servicing_type),
        servicing_state = format!("{:?}", state.host_status().servicing_state),
    );

    // Stage update
    if servicing_type == ServicingType::AbUpdate {
        ab_update::stage_update(
            &mut subsystems,
            ctx,
            state,
            #[cfg(feature = "grpc-dangerous")]
            sender,
        )
        .message("Failed to stage update")?;
    } else if servicing_type == ServicingType::RuntimeUpdate {
        runtime_update::stage_update(
            &mut subsystems,
            ctx,
            state,
            #[cfg(feature = "grpc-dangerous")]
            sender,
        )
        .message("Failed to stage update")?;
    }

    match servicing_type {
        ServicingType::AbUpdate => {
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
                ab_update::finalize_update(
                    state,
                    servicing_type,
                    Some(update_start_time),
                    #[cfg(feature = "grpc-dangerous")]
                    sender,
                )
                .message("Failed to finalize update")
            }
        }
        ServicingType::RuntimeUpdate => {
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
                runtime_update::finalize_update(
                    &mut subsystems,
                    image,
                    state,
                    servicing_type,
                    Some(update_start_time),
                    #[cfg(feature = "grpc-dangerous")]
                    sender,
                )
                .message("Failed to finalize update")
            }
        }
        ServicingType::CleanInstall => Err(TridentError::new(
            InvalidInputError::CleanInstallOnProvisionedHost,
        )),
        ServicingType::NoActiveServicing => Err(TridentError::internal("No active servicing type")),
    }
}
