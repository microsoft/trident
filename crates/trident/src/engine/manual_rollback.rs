use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use log::{debug, info, trace};

use osutils::efivar;
use trident_api::{
    config::Operations,
    constants::{ESP_RELATIVE_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
    error::{InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{ServicingState, ServicingType},
};

use crate::{
    cli::GetKind,
    container,
    datastore::DataStore,
    engine::{self, boot::uki, bootentries, runtime_update, EngineContext, SUBSYSTEMS},
    manual_rollback_utils::ManualRollbackContext,
    subsystems::esp,
    ExitKind,
};

/// Print rollback info for 'trident get'.
pub fn get_rollback_info(datastore: &DataStore, kind: GetKind) -> Result<String, TridentError> {
    // Get all HostStatus entries from the datastore.
    let host_statuses = datastore
        .get_host_statuses()
        .message("Failed to get datastore HostStatus entries")?;
    // Create ManualRollback context from HostStatus entries.
    let context = ManualRollbackContext::new(&host_statuses)
        .message("Failed to create manual rollback context")?;
    let rollback_chain =
        context
            .get_rollback_chain()
            .structured(ServicingError::ManualRollback {
                message: "Failed to get available rollbacks",
            })?;

    match kind {
        GetKind::RollbackTarget => {
            if let Some(first_rollback_host_status) = rollback_chain.first() {
                let target_output =
                    serde_yaml::to_string(&first_rollback_host_status.host_status.spec)
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
    invoke_if_next_is_runtime: bool,
    invoke_available_ab: bool,
) -> Result<(), TridentError> {
    // Get all HostStatus entries from the datastore.
    let host_statuses = datastore
        .get_host_statuses()
        .message("Failed to get datastore HostStatus entries")?;
    // Create ManualRollback context from HostStatus entries.
    let rollback_context = ManualRollbackContext::new(&host_statuses)
        .message("Failed to create manual rollback context")?;
    let check_string = rollback_context
        .check_requested_rollback(invoke_if_next_is_runtime, invoke_available_ab)?;
    println!("{check_string}");
    Ok(())
}

/// Handle manual rollback operations.
pub fn execute_rollback(
    datastore: &mut DataStore,
    invoke_if_next_is_runtime: bool,
    invoke_available_ab: bool,
    allowed_operations: &Operations,
) -> Result<ExitKind, TridentError> {
    let current_servicing_state = datastore.host_status().servicing_state;

    // Get all HostStatus entries from the datastore.
    let host_statuses = datastore
        .get_host_statuses()
        .message("Failed to get datastore HostStatus entries")?;
    // Create ManualRollback context from HostStatus entries.
    let rollback_context = ManualRollbackContext::new(&host_statuses)
        .message("Failed to create manual rollback context")?;

    let requested_rollback =
        rollback_context.get_requested_rollback(invoke_if_next_is_runtime, invoke_available_ab)?;

    let requested_rollback = match requested_rollback {
        Some(rollback_item) => rollback_item,
        None => {
            info!("No available rollbacks to perform");
            return Ok(ExitKind::Done);
        }
    };

    let mut skip_finalize_state_check = false;

    let engine_context = EngineContext {
        spec: requested_rollback.host_status.spec.clone(),
        spec_old: datastore.host_status().spec.clone(),
        servicing_type: ServicingType::ManualRollback,
        partition_paths: datastore.host_status().partition_paths.clone(),
        ab_active_volume: datastore.host_status().ab_active_volume,
        disk_uuids: datastore.host_status().disk_uuids.clone(),
        install_index: datastore.host_status().install_index,
        is_uki: Some(efivar::current_var_is_uki()),
        image: None,
        storage_graph: engine::build_storage_graph(&datastore.host_status().spec.storage)?, // Build storage graph
        filesystems: Vec::new(), // Will be populated after dynamic validation
    };
    // Perform staging if operation is allowed
    if allowed_operations.has_stage() {
        match current_servicing_state {
            ServicingState::ManualRollbackStaged | ServicingState::Provisioned => {
                if datastore.host_status().last_error.is_some() {
                    return Err(TridentError::new(InvalidInputError::InvalidRollbackState {
                        reason: "in Provisioned state but has a last error set".to_string(),
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

        stage_rollback(
            datastore,
            &engine_context,
            requested_rollback.requires_reboot,
        )
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
        // If only staging, skip finalize state check
        skip_finalize_state_check = true;
    }
    // Perform finalize if operation is allowed
    if allowed_operations.has_finalize() {
        if !skip_finalize_state_check {
            match current_servicing_state {
                ServicingState::ManualRollbackStaged | ServicingState::ManualRollbackFinalized => {
                    // OK to proceed
                }
                state => {
                    return Err(TridentError::new(InvalidInputError::InvalidRollbackState {
                        reason: format!("in unexpected state: {state:?}"),
                    }));
                }
            }
        }
        let finalize_result = finalize_rollback(
            datastore,
            &engine_context,
            requested_rollback.requires_reboot,
        )
        .message("Failed to stage manual rollback");
        // Persist the Trident background log and metrics file. Otherwise, the
        // staging logs would be lost.
        engine::persist_background_log_and_metrics(
            &datastore.host_status().spec.trident.datastore_path,
            None,
            datastore.host_status().servicing_state,
        );

        return finalize_result;
    }
    Ok(ExitKind::Done)
}

/// Stage manual rollback.
fn stage_rollback(
    datastore: &mut DataStore,
    engine_context: &EngineContext,
    rollback_requires_reboot: bool,
) -> Result<(), TridentError> {
    if rollback_requires_reboot {
        info!("Staging rollback that requires reboot");

        // If we have encrypted volumes and this is a UKI image, then we need to re-generate pcrlock
        // policy to include both the current boot and the rollback boot.
        if let Some(ref _encryption) = engine_context.spec.storage.encryption {
            // TODO: We know how to update the pcrlock policy in the servicing OS, but are
            // not able to do so for the target OS yet.
            if engine_context.is_uki()? {
                return Err(TridentError::new(ServicingError::ManualRollback {
                    message: "Cannot update pcrlock policy for UKI images during manual rollback",
                }));
                // debug!("Regenerating pcrlock policy to include rollback boot");

                // // Get the PCRs from Host Configuration
                // let pcrs = encryption
                //     .pcrs
                //     .iter()
                //     .fold(BitFlags::empty(), |acc, &pcr| acc | BitFlags::from(pcr));

                // // Get UKI and bootloader binaries for .pcrlock file generation
                // let (uki_binaries, bootloader_binaries) =
                //     encryption::get_binary_paths_pcrlock(engine_context, pcrs, None, true)
                //         .structured(ServicingError::GetBinaryPathsForPcrlockEncryption)?;

                // // Generate a pcrlock policy
                // pcrlock::generate_pcrlock_policy(pcrs, uki_binaries, bootloader_binaries)?;

                // // Update the rollback OS pcrlock.json file
            } else {
                debug!(
                    "Rollback OS is a grub image, \
                so skipping re-generating pcrlock policy for manual rollback"
                );
            }
        }
    } else {
        info!("Staging rollback that does not require reboot");
        // noop
    }

    // Mark the HostStatus as ManualRollbackStaged
    datastore.with_host_status(|host_status| {
        host_status.spec = engine_context.spec.clone();
        host_status.spec_old = engine_context.spec_old.clone();
        host_status.servicing_state = ServicingState::ManualRollbackStaged;
    })?;
    Ok(())
}

// Finalize manual rollback.
fn finalize_rollback(
    datastore: &mut DataStore,
    engine_context: &EngineContext,
    rollback_requires_reboot: bool,
) -> Result<ExitKind, TridentError> {
    if !rollback_requires_reboot {
        trace!("Manual rollback does not require reboot");

        let mut subsystems = SUBSYSTEMS.lock().unwrap();
        let rollback_exit_kind =
            runtime_update::rollback(&mut subsystems, datastore, Some(Instant::now()))
                .message("failed to rollback runtime update")?;

        datastore.with_host_status(|host_status| {
            host_status.spec = engine_context.spec.clone();
            host_status.spec_old = Default::default();
            host_status.servicing_state = ServicingState::Provisioned;
        })?;
        return Ok(rollback_exit_kind);
    }

    trace!("Manual rollback requires reboot");

    let root_path = if container::is_running_in_container()
        .message("Failed to check if Trident is running in a container")?
    {
        container::get_host_root_path().message("Failed to get host root path")?
    } else {
        PathBuf::from(ROOT_MOUNT_POINT_PATH)
    };
    let esp_path = Path::join(&root_path, ESP_RELATIVE_MOUNT_POINT_PATH);

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
        ServicingState::ManualRollbackStaged,
        &root_path,
    )
    .structured(ServicingError::SetUpUefiFallback)?;

    datastore.with_host_status(|host_status| {
        host_status.spec = engine_context.spec.clone();
        host_status.servicing_state = ServicingState::ManualRollbackFinalized;
    })?;

    Ok(ExitKind::NeedsReboot)
}
