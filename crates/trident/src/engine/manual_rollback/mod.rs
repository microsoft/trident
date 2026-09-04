use std::{path::PathBuf, time::Instant};

use enumflags2::BitFlags;
use log::{debug, info, trace, warn};

use osutils::{efivar, path, pcrlock};
use trident_api::{
    config::Operations,
    constants::{internal_params::NO_TRANSITION, ROOT_MOUNT_POINT_PATH},
    error::{InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{ServicingState, ServicingType},
};

use crate::{
    cli::GetKind,
    container,
    datastore::DataStore,
    engine::{
        self, boot::uki, bootentries, runtime_update, storage::encryption, EngineContext,
        EngineContextParams, SUBSYSTEMS,
    },
    logging::operation_context::set_servicing_id,
    subsystems::esp,
    ExitKind,
};

// Manual Rollback util
pub mod utils;
use utils::{ManualRollbackContext, ManualRollbackKind, ManualRollbackRequestKind};

/// Print rollback info for 'trident get'.
pub fn get_rollback_info(datastore: &DataStore, kind: GetKind) -> Result<String, TridentError> {
    // Get all HostStatus entries from the datastore.
    let host_statuses = datastore
        .get_host_statuses()
        .message("Failed to get datastore HostStatus entries")?;
    // Create ManualRollback context from HostStatus entries.
    let context = ManualRollbackContext::new(&host_statuses)
        .message("Failed to create manual rollback context")?;
    let rollback_chain = context.get_rollback_chain();

    match kind {
        GetKind::RollbackTarget => {
            if let Some(first_rollback_host_status) = rollback_chain.first() {
                let target_output = serde_yaml::to_string(&first_rollback_host_status.spec)
                    .structured(ServicingError::ManualRollback {
                        message: "Failed to serialize first rollback HostStatus spec",
                    })?;
                Ok(target_output)
            } else {
                info!("No available rollbacks to show target for");
                Ok("{}".to_string())
            }
        }
        GetKind::RollbackChain => {
            context
                .get_rollback_chain_yaml()
                .structured(ServicingError::ManualRollback {
                    message: "Failed to query rollback chain",
                })
        }
        _ => {
            info!("Unsupported GetKind for manual rollback query: {:?}", kind);
            Err(TridentError::new(ServicingError::ManualRollback {
                message: "unsupported get kind for manual rollback",
            }))
        }
    }
}

/// Check rollback availability and type.
pub fn check_rollback(
    datastore: &DataStore,
    rollback_request_kind: ManualRollbackRequestKind,
) -> Result<(), TridentError> {
    // Get all HostStatus entries from the datastore.
    let host_statuses = datastore
        .get_host_statuses()
        .message("Failed to get datastore HostStatus entries")?;
    // Create ManualRollback context from HostStatus entries.
    let rollback_context = ManualRollbackContext::new(&host_statuses)
        .message("Failed to create manual rollback context")?;
    let check_string = rollback_context.check_requested_rollback(rollback_request_kind)?;
    println!("{check_string}");
    Ok(())
}

/// Handle manual rollback operations.
///
/// Mirrors `engine::update::update()`'s `(ExitKind, ServicingType)` return
/// shape: a no-op ("nothing to roll back") reports
/// `(ExitKind::Done, ServicingType::NoActiveServicing)` rather than a bare
/// `ExitKind::Done` that's indistinguishable from a real rollback at the
/// gRPC layer (see `ServicingResponse.servicing_kind` in servicing.proto).
pub fn execute_rollback(
    datastore: &mut DataStore,
    requested_rollback_kind: ManualRollbackRequestKind,
    allowed_operations: &Operations,
) -> Result<(ExitKind, ServicingType), TridentError> {
    // Determine which servicing ID this invocation's telemetry (starting
    // with the manual_rollback_start event fired below) should carry,
    // *before* firing it -- set_servicing_id only affects events emitted
    // after it runs (see the fix for the analogous update_start ordering
    // issue), so this must happen first, not after staging logic below.
    //
    // If staging is requested, mint a fresh servicing ID now: this call is
    // either starting a brand new rollback operation, or (rarely) will
    // find nothing available to roll back and return early below without
    // ever actually staging -- either way, a fresh ID for "this attempt"
    // is correct, and a later genuine stage overwrites it again. If
    // staging is not requested (finalize-only), read back whatever was
    // persisted by an earlier, separate stage call instead -- see
    // `finalize_rollback`, which does the same read (redundantly, but
    // harmlessly, if this same call both stages and finalizes).
    if allowed_operations.has_stage() {
        let servicing_id = datastore
            .new_servicing_id()
            .message("Failed to create servicing ID")?;
        info!("Servicing ID: {servicing_id}");
        set_servicing_id(servicing_id.to_string());
    } else if let Ok(Some(servicing_id)) = datastore.servicing_id() {
        set_servicing_id(servicing_id.to_string());
    }

    tracing::info!(
        metric_name = "manual_rollback_start",
        requested_rollback_kind = format!("{:?}", requested_rollback_kind),
        servicing_state = format!("{:?}", datastore.host_status().servicing_state),
        stage = allowed_operations.has_stage(),
        finalize = allowed_operations.has_finalize(),
    );

    // Tracks the rollback kind actually staged this call, so the trailing
    // "stage completed, finalize not requested this call" return below can
    // report it instead of a generic NoActiveServicing. Stays None when
    // has_stage() didn't run this call (finalize-only), or when it exited
    // early via the no-rollback-available branch below (which returns its
    // own explicit NoActiveServicing directly).
    let mut staged_rollback_type = None;

    // Perform staging if operation is allowed
    if allowed_operations.has_stage() {
        match datastore.host_status().servicing_state {
            ServicingState::ManualRollbackAbStaged
            | ServicingState::ManualRollbackRuntimeStaged
            | ServicingState::Provisioned => {
                if datastore.host_status().last_error.is_some() {
                    return Err(TridentError::new(InvalidInputError::InvalidRollbackState {
                        reason: "in required state but has a last error set, use install or update rather than rollback".to_string(),
                    }));
                }
                // OK to proceed
            }
            state => {
                return Err(TridentError::new(InvalidInputError::InvalidRollbackState {
                    reason: format!("in unexpected state: {state:?}"),
                }));
            }
        }

        // Get all HostStatus entries from the datastore.
        let host_statuses = datastore
            .get_host_statuses()
            .message("Failed to get datastore HostStatus entries")?;
        // Create ManualRollback context from HostStatus entries.
        let rollback_context = ManualRollbackContext::new(&host_statuses)
            .message("Failed to create manual rollback context")?;

        let requested_rollback =
            rollback_context.get_requested_rollback(requested_rollback_kind)?;

        let requested_rollback = match requested_rollback {
            Some(rollback_item) => rollback_item,
            None => {
                info!("No available rollbacks to perform");
                return Ok((ExitKind::Done, ServicingType::NoActiveServicing));
            }
        };

        let rollback_type = match requested_rollback.kind {
            ManualRollbackKind::Ab => ServicingType::ManualRollbackAb,
            ManualRollbackKind::Runtime => ServicingType::ManualRollbackRuntime,
        };
        staged_rollback_type = Some(rollback_type);

        let engine_context = EngineContext::new(EngineContextParams {
            spec: requested_rollback.spec.clone(),
            spec_old: datastore.host_status().spec.clone(),
            servicing_type: rollback_type,
            is_stream_image: false,
            partition_paths: datastore.host_status().partition_paths.clone(),
            ab_active_volume: datastore.host_status().ab_active_volume,
            disk_uuids: datastore.host_status().disk_uuids.clone(),
            install_index: datastore.host_status().install_index,
            is_uki: Some(efivar::current_var_is_uki()),
            image: None,
        })?;

        let staging_state = match requested_rollback.kind {
            ManualRollbackKind::Ab => ServicingState::ManualRollbackAbStaged,
            ManualRollbackKind::Runtime => ServicingState::ManualRollbackRuntimeStaged,
        };

        stage_rollback(datastore, &engine_context, staging_state)
            .message("Failed to stage manual rollback")?;

        if !allowed_operations.has_finalize() {
            // Persist the Trident background log and metrics file. Otherwise, the
            // staging logs would be lost.
            engine::persist_background_log_and_metrics(
                &datastore.host_status().spec.trident.datastore_path,
                None,
                datastore.host_status().servicing_state,
            );
        }
    }

    // Perform finalize if operation is allowed
    if allowed_operations.has_finalize() {
        let current_servicing_type = match datastore.host_status().servicing_state {
            ServicingState::ManualRollbackAbStaged => ServicingType::ManualRollbackAb,
            ServicingState::ManualRollbackRuntimeStaged => ServicingType::ManualRollbackRuntime,
            state => {
                return Err(TridentError::new(InvalidInputError::InvalidRollbackState {
                    reason: format!("in unexpected state: {state:?}"),
                }));
            }
        };
        let engine_context = EngineContext::new(EngineContextParams {
            spec: datastore.host_status().spec.clone(),
            spec_old: datastore.host_status().spec_old.clone(),
            servicing_type: current_servicing_type,
            is_stream_image: false,
            partition_paths: datastore.host_status().partition_paths.clone(),
            ab_active_volume: datastore.host_status().ab_active_volume,
            disk_uuids: datastore.host_status().disk_uuids.clone(),
            install_index: datastore.host_status().install_index,
            is_uki: Some(efivar::current_var_is_uki()),
            image: None,
        })?;
        let finalize_result = finalize_rollback(
            datastore,
            &engine_context,
            datastore.host_status().servicing_state,
        )
        .message("Failed to finalize manual rollback");
        // Persist the Trident background log and metrics file. Otherwise, the
        // finalize logs would be lost.
        engine::persist_background_log_and_metrics(
            &datastore.host_status().spec.trident.datastore_path,
            None,
            datastore.host_status().servicing_state,
        );

        return finalize_result.map(|exit_kind| (exit_kind, current_servicing_type));
    }
    Ok((
        ExitKind::Done,
        staged_rollback_type.unwrap_or(ServicingType::NoActiveServicing),
    ))
}

/// Stage manual rollback.
fn stage_rollback(
    datastore: &mut DataStore,
    engine_context: &EngineContext,
    staging_state: ServicingState,
) -> Result<(), TridentError> {
    if matches!(staging_state, ServicingState::ManualRollbackAbStaged) {
        info!("Staging rollback of A/B update that requires reboot");

        // If we have encrypted volumes and this is a UKI image, then we need to re-generate
        // pcrlock policy to include both current boot and rollback boots.
        if let Some(encryption) = &engine_context.spec.storage.encryption {
            if engine_context.is_uki()? {
                debug!("Regenerating pcrlock policy to include rollback boot");

                // Get the PCRs from Host Configuration
                let pcrs = encryption
                    .pcrs
                    .iter()
                    .fold(BitFlags::empty(), |acc, &pcr| acc | BitFlags::from(pcr));

                // Get UKI and bootloader binaries for .pcrlock file generation
                let (uki_binaries, bootloader_binaries) =
                    encryption::get_binary_paths_pcrlock(engine_context, pcrs, None, true)
                        .structured(ServicingError::GetBinaryPathsForPcrlockEncryption)?;

                // Generate a pcrlock policy
                pcrlock::generate_pcrlock_policy(pcrs, uki_binaries, bootloader_binaries)?;
            } else {
                debug!(
                    "Rollback OS is a grub image, \
                so skipping re-generating pcrlock policy for manual rollback"
                );
            }
        }
    } else {
        info!("Staging rollback of runtime update that does not require reboot");
        // noop
    }

    // Mark the HostStatus as `staging_state`
    datastore.with_host_status(|host_status| {
        host_status.spec = engine_context.spec.clone();
        host_status.spec_old = engine_context.spec_old.clone();
        host_status.servicing_state = staging_state;
    })?;
    Ok(())
}

// Finalize manual rollback.
fn finalize_rollback(
    datastore: &mut DataStore,
    engine_context: &EngineContext,
    staging_state: ServicingState,
) -> Result<ExitKind, TridentError> {
    // Attach whatever servicing ID is currently persisted: set moments ago
    // by execute_rollback's own staging branch above if this is a combined
    // stage+finalize call, or read back here for the first time if this
    // call is finalizing a rollback staged by a separate, earlier
    // invocation. Best-effort: a missing/unreadable ID just means this
    // invocation's telemetry won't carry one, which never blocks the
    // actual rollback.
    if let Ok(Some(servicing_id)) = datastore.servicing_id() {
        set_servicing_id(servicing_id.to_string());
    }

    if matches!(staging_state, ServicingState::ManualRollbackRuntimeStaged) {
        trace!("Finalizing rollback of runtime update that does not require reboot");

        let mut subsystems = SUBSYSTEMS.lock().unwrap();
        let rollback_exit_kind =
            runtime_update::rollback(&mut subsystems, datastore, Some(Instant::now()))
                .message("failed to rollback runtime update")?;

        datastore.with_host_status(|host_status| {
            host_status.spec = engine_context.spec.clone();
            host_status.spec_old = Default::default();
            host_status.servicing_state = ServicingState::Provisioned;
        })?;

        // Unlike the A/B rollback case below, a runtime rollback requires no
        // reboot, so this never reaches `engine::rollback`'s post-reboot
        // boot-validation flow -- the only place `manual_rollback_success`
        // is otherwise fired (and only for the `ManualRollbackAbFinalized`
        // state). Without this, a runtime rollback would emit
        // `manual_rollback_start` but no matching success signal at all.
        info!("Manual rollback of runtime update succeeded");
        tracing::info!(
            metric_name = "manual_rollback_runtime_success",
            value = true
        );

        // Persistence happens in the caller (execute_rollback), after this
        // function returns -- not here. This function's own outcome metric
        // has already fired above by the time that happens, so the
        // archived metrics file still includes it; persisting again here
        // as well would just archive the same (or, with second-resolution
        // filenames, a second) copy.
        return Ok(rollback_exit_kind);
    }

    trace!("Finalizing rollback of A/B update that requires reboot");

    let root_path = container::get_host_relative_path(PathBuf::from(ROOT_MOUNT_POINT_PATH))
        .message("Failed to get host root path")?;
    let esp_path = path::join_relative(&root_path, engine_context.esp_mount_path.as_path());

    // In UKI, find the previous UKI and set it as default boot entry
    if engine_context.is_uki()? {
        uki::use_previous_uki_as_default(&esp_path)
            .message("Failed to set default boot entry to previous")?;
    }
    // Reconfigure UEFI boot-order to point at inactive volume
    bootentries::create_and_update_boot_variables(engine_context, &esp_path)?;
    // Analogous to how UEFI variables are configured.
    esp::set_uefi_fallback_contents(
        engine_context,
        ServicingState::ManualRollbackAbStaged,
        &root_path,
    )
    .structured(ServicingError::SetUpUefiFallback)?;

    datastore.with_host_status(|host_status| {
        host_status.spec = engine_context.spec.clone();
        host_status.servicing_state = ServicingState::ManualRollbackAbFinalized;
    })?;

    if !datastore
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
