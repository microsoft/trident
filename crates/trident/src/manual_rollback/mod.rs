#![allow(unused)]

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{bail, Context, Error};
use enumflags2::BitFlags;
use log::{debug, info, trace};
use maplit::hashmap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use osutils::{efivar, lsblk, pcrlock};
use trident_api::{
    config::{
        AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
        MountOptions, MountPoint, Operations, Partition, PartitionSize, PartitionTableType,
        PartitionType, VerityCorruptionOption, VerityDevice,
    },
    constants::{
        internal_params::ENABLE_UKI_SUPPORT, EFI_DEFAULT_BIN_RELATIVE_PATH, ESP_EFI_DIRECTORY,
        ESP_RELATIVE_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH,
    },
    error::{InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{decode_host_status, AbVolumeSelection, HostStatus, ServicingState, ServicingType},
    BlockDeviceId,
};

use crate::{
    cli::GetKind,
    container,
    datastore::{self, DataStore},
    engine::{
        self,
        boot::{self, uki, ESP_EXTRACTION_DIRECTORY},
        bootentries, rollback,
        storage::encryption,
        EngineContext, REQUIRES_REBOOT,
    },
    subsystems::esp,
    ExitKind, OsImage,
};

const MINIMUM_ROLLBACK_TRIDENT_VERSION: &str = "0.21.0";
/// Print rollback info for 'trident get'.
pub fn get_rollback_info(datastore: &DataStore, kind: GetKind) -> Result<String, TridentError> {
    // Get all HostStatus entries from the datastore.
    let host_statuses = datastore
        .get_host_statuses()
        .message("Failed to get datastore HostStatus entries")?;
    // Create ManualRollback context from HostStatus entries.
    let context = ManualRollbackContext::new(&host_statuses)
        .message("Failed to create manual rollback context")?;
    let rollback_chain = context
        .get_rollback_chain()
        .structured(ServicingError::ManualRollback)
        .message("Failed to get available rollbacks")?;

    match kind {
        GetKind::RollbackTarget => {
            if let Some(first_rollback_host_status) = rollback_chain.first() {
                let target_output =
                    serde_yaml::to_string(&first_rollback_host_status.host_status.spec)
                        .structured(ServicingError::ManualRollback)
                        .message("Failed to serialize first rollback HostStatus spec")?;
                return Ok(target_output);
            } else {
                info!("No available rollbacks to show target for");
                return Ok("{}".to_string());
            }
        }
        GetKind::RollbackChain => {
            return context
                .get_rollback_chain_yaml()
                .structured(ServicingError::ManualRollback)
                .message("Failed to query for 'get rollback-chain'");
        }
        _ => {}
    }
    Err(TridentError::new(ServicingError::ManualRollback))
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
    let available_rollbacks = rollback_context
        .get_rollback_chain()
        .structured(ServicingError::ManualRollback)
        .message("Failed to get available rollbacks")?;
    let (_rollback_index, check_string) = get_requested_rollback_info(
        &available_rollbacks,
        invoke_if_next_is_runtime,
        invoke_available_ab,
    )?;
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

    let available_rollbacks = rollback_context
        .get_rollback_chain()
        .structured(ServicingError::ManualRollback)
        .message("Failed to get available rollbacks")?;

    let (rollback_index, check_string) = get_requested_rollback_info(
        &available_rollbacks,
        invoke_if_next_is_runtime,
        invoke_available_ab,
    )?;

    let rollback_index = match rollback_index {
        Some(index) => index,
        None => {
            info!("No available rollbacks to perform");
            return Ok(ExitKind::Done);
        }
    };

    let mut skip_finalize_state_check = false;

    let mut engine_context = EngineContext {
        spec: available_rollbacks[rollback_index].host_status.spec.clone(),
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
            &available_rollbacks,
            rollback_index,
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
            available_rollbacks[rollback_index].requires_reboot,
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

/// Get requested rollback.
fn get_requested_rollback_info(
    available_rollbacks: &[RollbackDetail],
    invoke_if_next_is_runtime: bool,
    invoke_available_ab: bool,
) -> Result<(Option<usize>, String), TridentError> {
    if available_rollbacks.is_empty() {
        info!("No available rollbacks to perform");
        return Ok((None, "none".to_string()));
    }

    let rollback_index = match (invoke_if_next_is_runtime, invoke_available_ab) {
        (false, false) => {
            // No expectations specified, proceed with first
            0
        }
        (true, false) => {
            // Expecting runtime rollback as first
            if available_rollbacks[0].requires_reboot {
                return Err(TridentError::new(
                    InvalidInputError::InvalidRollbackExpectation {
                        reason:
                            "expected to undo a runtime update but rollback will undo an A/B update"
                                .to_string(),
                    },
                ));
            }
            0
        }
        (false, true) => {
            // Find first A/B rollback along with its index
            let Some((index, _)) = available_rollbacks
                .iter()
                .enumerate()
                .find(|(_, r)| r.requires_reboot)
            else {
                return Err(TridentError::new(
                    InvalidInputError::InvalidRollbackExpectation {
                        reason: "expected to undo an A/B update but no A/B rollback is available"
                            .to_string(),
                    },
                ));
            };
            index
        }
        (true, true) => {
            return Err(TridentError::new(
                InvalidInputError::InvalidRollbackExpectation {
                    reason: "conflicting expectations: cannot expect to undo both a runtime update and an A/B update"
                        .to_string(),
                },
            ));
        }
    };

    Ok((
        Some(rollback_index),
        if available_rollbacks[rollback_index].requires_reboot {
            "ab".to_string()
        } else {
            "runtime".to_string()
        },
    ))
}

/// Stage manual rollback.
fn stage_rollback(
    datastore: &mut DataStore,
    engine_context: &EngineContext,
    available_rollbacks: &[RollbackDetail],
    rollback_index: usize,
) -> Result<(), TridentError> {
    if available_rollbacks[rollback_index].requires_reboot {
        info!("Staging rollback that requires reboot");

        // If we have encrypted volumes and this is a UKI image, then we need to re-generate pcrlock
        // policy to include both the current boot and the rollback boot.
        if let Some(ref encryption) = engine_context.spec.storage.encryption {
            // TODO: Handle any pcr-lock encryption related changes needed
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
        info!("Staging rollback that does not require reboot");
        // TODO: Invoke subsystem runtime rollbacks if part of stage
    }

    // Mark the HostStatus as ManualRollbackStaged
    datastore.with_host_status(|host_status| {
        host_status.spec = engine_context.spec.clone();
        host_status.servicing_state = ServicingState::ManualRollbackStaged;
    })?;

    Ok(())
}

// Finalize manual rollback.
fn finalize_rollback(
    datastore: &mut DataStore,
    engine_context: &EngineContext,
    ab_rollback: bool,
) -> Result<ExitKind, TridentError> {
    if !ab_rollback {
        trace!("Manual rollback does not require reboot");

        // TODO: invoke subsystem runtime rollbacks if part of finalize

        datastore.with_host_status(|host_status| {
            host_status.spec = engine_context.spec.clone();
            host_status.spec_old = Default::default();
            host_status.servicing_state = ServicingState::Provisioned;
        })?;
        return Ok(ExitKind::Done);
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

    // In UKI, use the LoaderEntries variable to get the previous boot entry and set it as current
    if engine_context.is_uki()? {
        efivar::set_default_to_previous()
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RollbackDetail {
    requires_reboot: bool,
    host_status: HostStatus,
    #[serde(skip)]
    host_status_index: i32,
}
struct ManualRollbackContext {
    volume_a_available_rollbacks: Vec<RollbackDetail>,
    volume_b_available_rollbacks: Vec<RollbackDetail>,
    active_volume: Option<AbVolumeSelection>,
    rollback_action: Option<ServicingType>,
    rollback_volume: Option<AbVolumeSelection>,
}
impl ManualRollbackContext {
    fn new(host_statuses: &[HostStatus]) -> Result<Self, TridentError> {
        let minimum_rollback_trident_version = "0.21.0";
        let minimum_rollback_trident_semver =
            semver::Version::parse(minimum_rollback_trident_version).map_err(|e| {
                TridentError::new(InvalidInputError::InvalidRollbackExpectation {
                    reason: format!(
                        "Failed to parse minimum rollback Trident version '{minimum_rollback_trident_version}': {e}",
                    ),
                })
            })?;

        // Initialize context from HostStatus entries.
        let mut instance = ManualRollbackContext {
            volume_a_available_rollbacks: Vec::new(),
            volume_b_available_rollbacks: Vec::new(),
            active_volume: None,
            rollback_action: None,
            rollback_volume: None,
        };

        // Create special handling for offline-initialize initial state
        // where there are multiple (annecdotally: 3) consecutive Provisioned
        // host statuses.
        let mut last_initial_consecutive_provisioned_state = -1;
        for (i, hs) in host_statuses.iter().enumerate() {
            if hs.servicing_state != ServicingState::Provisioned {
                break;
            }
            last_initial_consecutive_provisioned_state = i as i32;
        }

        let mut auto_rollback = false;
        let mut last_provisioned = false;
        let mut rollback = false;
        let mut needs_reboot = false;
        let mut active_index = -1;

        for (i, hs) in host_statuses.iter().enumerate() {
            let trident_is_too_old = match hs.trident_version {
                ref v if v.is_empty() => true,
                ref v => match semver::Version::parse(v) {
                    Ok(ver) => ver < minimum_rollback_trident_semver,
                    Err(e) => {
                        return Err(TridentError::new(
                            InvalidInputError::InvalidRollbackExpectation {
                                reason: format!(
                                    "Failed to parse host status Trident version '{}': {:?}",
                                    &hs.trident_version, e
                                ),
                            },
                        ));
                    }
                },
            };
            trace!(
                "Processing HostStatus at index {}: servicing_state={:?}, ab_active_volume={:?}, old_tridnet={}",
                i,
                hs.servicing_state,
                hs.ab_active_volume,
                trident_is_too_old
            );
            // If the inactive volume is overwritten by
            // ab-update-staged, clear the available
            // rollbacks for it
            if hs.servicing_state == ServicingState::AbUpdateStaged {
                trace!("AbUpdateStaged detected at index {}: clearing available rollbacks for inactive volume {:?}: a:[{:?}] b:[{:?}]",
                    i,
                    hs.ab_active_volume,
                    instance.volume_a_available_rollbacks.len(),
                    instance.volume_b_available_rollbacks.len()
                );
                match hs.ab_active_volume {
                    Some(AbVolumeSelection::VolumeA) => {
                        instance.volume_b_available_rollbacks = Vec::new();
                    }
                    Some(AbVolumeSelection::VolumeB) => {
                        instance.volume_a_available_rollbacks = Vec::new();
                    }
                    None => {}
                }
            }

            // Update rollback context for each HostStatus.ServicingState == Provisioned
            if hs.servicing_state == ServicingState::Provisioned {
                trace!(
                    "Processing Provisioned state at index {} for active volume {:?}",
                    i,
                    hs.ab_active_volume
                );
                // If we entered a Provisioned state from a Provisioned state (so
                // ignoring the first Provisioned state, where there can be no rollback),
                // update the available rollbacks depending on whether the last action
                // was a rollback or not
                if !last_provisioned && active_index != -1 {
                    let host_status_context = RollbackDetail {
                        host_status: host_statuses[active_index as usize].clone(),
                        host_status_index: active_index,
                        requires_reboot: needs_reboot,
                    };
                    if auto_rollback {
                        trace!(
                            "Auto-rollback detected at index {} for active volume {:?}",
                            i,
                            instance.active_volume
                        );
                    } else if rollback {
                        let active_volume_changed = hs.ab_active_volume != instance.active_volume;
                        // If the active volume changed, then
                        //   1. we can remove all of the available rollbacks for the previously active volume
                        //   2. we can remove the first available rollback for the newly active volume
                        if active_volume_changed {
                            match instance.active_volume {
                                Some(AbVolumeSelection::VolumeA) => {
                                    instance.volume_a_available_rollbacks = Vec::new();
                                }
                                Some(AbVolumeSelection::VolumeB) => {
                                    instance.volume_b_available_rollbacks = Vec::new();
                                }
                                None => {}
                            }
                            match hs.ab_active_volume {
                                Some(AbVolumeSelection::VolumeA) => {
                                    if !instance.volume_a_available_rollbacks.is_empty() {
                                        instance.volume_a_available_rollbacks.remove(0);
                                    }
                                }
                                Some(AbVolumeSelection::VolumeB) => {
                                    if !instance.volume_b_available_rollbacks.is_empty() {
                                        instance.volume_b_available_rollbacks.remove(0);
                                    }
                                }
                                None => {}
                            }
                        } else {
                            // If the active volume did not change, then a runtime rollback was performed
                            // and we can remove the first available rollback for the active volume
                            match instance.active_volume {
                                Some(AbVolumeSelection::VolumeA) => {
                                    if !instance.volume_a_available_rollbacks.is_empty() {
                                        instance.volume_a_available_rollbacks.remove(0);
                                    }
                                }
                                Some(AbVolumeSelection::VolumeB) => {
                                    if !instance.volume_b_available_rollbacks.is_empty() {
                                        instance.volume_b_available_rollbacks.remove(0);
                                    }
                                }
                                None => {}
                            }
                        }
                    } else if host_status_context.host_status_index
                        >= last_initial_consecutive_provisioned_state
                    {
                        trace!(
                            "New Provisioned state detected at index {} for active volume {:?}",
                            i,
                            instance.active_volume
                        );
                        let last_error_exists = hs.last_error.is_some();
                        // Prepend the last Provisioned index to the previously active volume's available
                        // rollbacks.
                        match (
                            last_error_exists,
                            trident_is_too_old,
                            instance.active_volume,
                        ) {
                            (false, false, Some(AbVolumeSelection::VolumeA)) => {
                                instance
                                    .volume_a_available_rollbacks
                                    .insert(0, host_status_context);
                            }
                            (false, false, Some(AbVolumeSelection::VolumeB)) => {
                                instance
                                    .volume_b_available_rollbacks
                                    .insert(0, host_status_context);
                            }
                            // Do not add an available rollback if there is no active volume
                            // or if the Trident version is too old or if last_error is set
                            (true, _, _) | (false, true, _) | (false, false, None) => {}
                        }
                    }
                }
                // Update the context's active volume and index
                instance.active_volume = hs.ab_active_volume;
                active_index = i as i32;
                needs_reboot = false;
                // Reset the loop's rollback tracking
                rollback = false;
                // Reset the loop's auto-rollback tracking
                auto_rollback = false;
                // Last state seen was Provisioned: guard against sequential 'duplicate' Provisioned states
                last_provisioned = true;
            } else {
                // Check each non-Provisioned state to see if it represents a rollback action
                rollback = matches!(
                    hs.servicing_state,
                    ServicingState::ManualRollbackStaged | ServicingState::ManualRollbackFinalized
                );
                needs_reboot = matches!(
                    hs.servicing_state,
                    ServicingState::AbUpdateFinalized | ServicingState::AbUpdateFinalized
                );
                if matches!(
                    hs.servicing_state,
                    ServicingState::AbUpdateHealthCheckFailed
                ) {
                    auto_rollback = true;
                }
                last_provisioned = false;
                trace!(
                    "Detected servicing state {:?} at index {}: rollback={}, needs_reboot={}, auto_rollback={}z",
                    hs.servicing_state,
                    i,
                    rollback,
                    needs_reboot,
                    auto_rollback
                )
            }
        }

        if let Some((first_rollback, rollback_volume)) = instance.get_first_rollback() {
            trace!(
                "First available rollback at index {} for volume {:?}",
                first_rollback,
                rollback_volume
            );
            instance.rollback_volume = Some(rollback_volume);

            instance.rollback_action = None;
            if first_rollback != -1 {
                let rollback_next_state =
                    host_statuses[first_rollback as usize + 1].servicing_state;
                if matches!(
                    rollback_next_state,
                    ServicingState::AbUpdateStaged | ServicingState::AbUpdateFinalized
                ) {
                    instance.rollback_action = Some(ServicingType::AbUpdate)
                } else if matches!(rollback_next_state, ServicingState::RuntimeUpdateStaged) {
                    instance.rollback_action = Some(ServicingType::RuntimeUpdate)
                }
            }
        }

        Ok(instance)
    }

    fn get_first_rollback_host_status(&self) -> Result<Option<HostStatus>, Error> {
        self.get_rollback_chain()
            .context("Failed to get available rollbacks")?
            .into_iter()
            .next()
            .map_or_else(|| Ok(None), |detail| Ok(Some(detail.host_status.clone())))
    }

    fn get_first_rollback(&self) -> Option<(i32, AbVolumeSelection)> {
        let mut rollback_a = -1;
        let mut rollback_b = -1;
        trace!(
            "Checking for first available rollback: A=[{:?}] B:[{:?}]",
            self.volume_a_available_rollbacks.len(),
            self.volume_b_available_rollbacks.len()
        );
        if !self.volume_a_available_rollbacks.is_empty() {
            rollback_a = self.volume_a_available_rollbacks[0].host_status_index;
        }
        if !self.volume_b_available_rollbacks.is_empty() {
            rollback_b = self.volume_b_available_rollbacks[0].host_status_index;
        }
        if rollback_a > rollback_b {
            trace!("First rollback is on Volume A at index {}", rollback_a);
            return Some((rollback_a, AbVolumeSelection::VolumeA));
        }
        if rollback_b != -1 {
            trace!("First rollback is on Volume B at index {}", rollback_b);
            return Some((rollback_b, AbVolumeSelection::VolumeB));
        }
        trace!(" No available rollbacks detected");
        None
    }

    fn get_requires_reboot(&self) -> Result<bool, Error> {
        Ok(matches!(
            self.rollback_action,
            Some(ServicingType::AbUpdate)
        ))
    }

    fn get_rollback_chain(&self) -> Result<Vec<RollbackDetail>, Error> {
        let mut contexts = self
            .volume_a_available_rollbacks
            .clone()
            .into_iter()
            .chain(self.volume_b_available_rollbacks.clone())
            .collect::<Vec<_>>();
        contexts.sort_by(|a, b| b.host_status_index.cmp(&a.host_status_index));
        info!("Available rollback count: {}", contexts.len());
        Ok(contexts)
    }

    fn get_rollback_chain_yaml(&self) -> Result<String, Error> {
        let contexts = self.get_rollback_chain()?;
        let full_yaml =
            serde_yaml::to_string(&contexts).context("Failed to serialize rollback contexts")?;
        info!("Available rollbacks:\n{}", full_yaml);
        Ok(full_yaml)
    }
}

#[cfg(test)]
mod tests {
    use crate::TRIDENT_VERSION;
    use osutils::mdadm::create;

    use super::*;

    struct HostStatusTest {
        host_status: HostStatus,
        expected_requires_reboot: bool,
        expected_available_rollbacks: Vec<usize>,
    }
    fn host_status(
        active_volume: Option<AbVolumeSelection>,
        servicing_state: ServicingState,
        old_version: &str,
        error: Option<String>,
    ) -> HostStatus {
        let mut last_error: Option<serde_yaml::Value> = None;
        if let Some(error) = error {
            last_error = Some(
                serde_yaml::to_value(hashmap! {
                    "message".to_string() => error,
                })
                .unwrap(),
            );
        }
        HostStatus {
            ab_active_volume: active_volume,
            servicing_state,
            trident_version: old_version.to_string(),
            last_error,
            ..Default::default()
        }
    }
    fn prov(
        active_volume: Option<AbVolumeSelection>,
        expected_requires_reboot: bool,
        expected_available_rollbacks: Vec<usize>,
        old_version: &str,
    ) -> HostStatusTest {
        HostStatusTest {
            host_status: host_status(
                active_volume,
                ServicingState::Provisioned,
                old_version,
                None,
            ),
            expected_requires_reboot,
            expected_available_rollbacks,
        }
    }
    fn prov_e(
        active_volume: Option<AbVolumeSelection>,
        expected_requires_reboot: bool,
        expected_available_rollbacks: Vec<usize>,
        old_version: &str,
        error: Option<String>,
    ) -> HostStatusTest {
        HostStatusTest {
            host_status: host_status(
                active_volume,
                ServicingState::Provisioned,
                old_version,
                error,
            ),
            expected_requires_reboot,
            expected_available_rollbacks,
        }
    }
    fn inter(
        active_volume: Option<AbVolumeSelection>,
        servicing_state: ServicingState,
        old_version: &str,
    ) -> HostStatusTest {
        HostStatusTest {
            host_status: host_status(active_volume, servicing_state, old_version, None),
            expected_requires_reboot: false,
            expected_available_rollbacks: vec![],
        }
    }
    fn inter_e(
        active_volume: Option<AbVolumeSelection>,
        servicing_state: ServicingState,
        old_version: &str,
        error: Option<String>,
    ) -> HostStatusTest {
        HostStatusTest {
            host_status: host_status(active_volume, servicing_state, old_version, error),
            expected_requires_reboot: false,
            expected_available_rollbacks: vec![],
        }
    }

    fn create_rollback_context_for_testing(
        host_status_test_list: &[HostStatusTest],
    ) -> ManualRollbackContext {
        let final_state = host_status_test_list
            .iter()
            .filter(|hst| hst.host_status.servicing_state == ServicingState::Provisioned)
            .next_back()
            .unwrap();
        let host_statuses = host_status_test_list
            .iter()
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        ManualRollbackContext::new(&host_statuses).unwrap()
    }
    fn rollback_context_testing(host_status_test_list: &[HostStatusTest], test_description: &str) {
        let context = create_rollback_context_for_testing(host_status_test_list);
        let final_state = host_status_test_list
            .iter()
            .filter(|hst| hst.host_status.servicing_state == ServicingState::Provisioned)
            .next_back()
            .unwrap();
        rollback_context_testing_for_expected(
            host_status_test_list,
            final_state.expected_available_rollbacks.clone(),
            final_state.expected_requires_reboot,
            test_description,
        );
    }
    fn rollback_context_testing_for_expected(
        host_status_test_list: &[HostStatusTest],
        expected_available_rollbacks: Vec<usize>,
        expected_requires_reboot: bool,
        test_description: &str,
    ) {
        let context = create_rollback_context_for_testing(host_status_test_list);
        trace!(
            "{}: expected_requires_reboot: {}, expected_available_rollbacks: {:?}",
            test_description,
            expected_requires_reboot,
            expected_available_rollbacks
        );
        assert_eq!(
            context.get_requires_reboot().unwrap(),
            expected_requires_reboot
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_yaml().unwrap(),
        )
        .unwrap();
        assert_eq!(serialized_output.len(), expected_available_rollbacks.len())
    }

    const VOL_A: Option<AbVolumeSelection> = Some(AbVolumeSelection::VolumeA);
    const VOL_B: Option<AbVolumeSelection> = Some(AbVolumeSelection::VolumeB);
    const NONE: &str = "";
    const OLD: &str = "0.19.0";
    const MIN: &str = MINIMUM_ROLLBACK_TRIDENT_VERSION;
    const NEW: &str = TRIDENT_VERSION;
    const CI_FINAL: ServicingState = ServicingState::CleanInstallFinalized;
    const RU_STAGE: ServicingState = ServicingState::RuntimeUpdateStaged;
    const AB_STAGE: ServicingState = ServicingState::AbUpdateStaged;
    const AB_FINAL: ServicingState = ServicingState::AbUpdateFinalized;
    const AB_HC_FAIL: ServicingState = ServicingState::AbUpdateHealthCheckFailed;
    const MR_STAGE: ServicingState = ServicingState::ManualRollbackStaged;
    const MR_FINAL: ServicingState = ServicingState::ManualRollbackFinalized;

    #[test]
    fn test_rollback_context() {
        let host_status_list = vec![
            inter(None, CI_FINAL, MIN),
            inter(None, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
            inter(VOL_A, RU_STAGE, MIN),
            prov(VOL_A, false, vec![2], MIN),
            inter(VOL_A, RU_STAGE, MIN),
            prov(VOL_A, false, vec![4, 2], MIN),
            inter(VOL_A, AB_STAGE, MIN),
            inter(VOL_A, AB_FINAL, MIN),
            prov(VOL_B, true, vec![6, 4, 2], MIN),
            inter(VOL_B, AB_STAGE, MIN),
            inter(VOL_B, AB_FINAL, MIN),
            prov(VOL_A, true, vec![9], MIN),
            inter(VOL_A, MR_STAGE, MIN),
            inter(VOL_A, MR_FINAL, MIN),
            prov(VOL_B, false, vec![], MIN),
        ];
        for (i, hs) in host_status_list.iter().enumerate() {
            trace!(
                "HS: {:?}, expected_requires_reboot: {}, expected_available_rollbacks: {:?}",
                hs.host_status.servicing_state,
                hs.expected_requires_reboot,
                hs.expected_available_rollbacks
            );
            rollback_context_testing(&host_status_list, "Test rolling context at each step");
        }
    }

    #[test]
    fn test_runtime_rollback_context_mid_rollback() {
        let host_status_list = vec![
            inter(None, CI_FINAL, MIN),
            inter(None, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
            inter(VOL_A, RU_STAGE, MIN),
            prov(VOL_A, false, vec![2], MIN),
            inter(VOL_A, RU_STAGE, MIN),
            prov(VOL_A, false, vec![4, 2], MIN),
            inter(VOL_A, RU_STAGE, MIN),
            prov(VOL_A, false, vec![6, 4, 2], MIN),
            inter(VOL_A, MR_STAGE, MIN),
            inter(VOL_A, MR_FINAL, MIN),
        ];
        rollback_context_testing(
            &host_status_list,
            "Clean install with ab updates and mid runtime rollback",
        );
    }

    #[test]
    fn test_ab_rollback_context_mid_rollback() {
        let host_status_list = vec![
            inter(None, CI_FINAL, MIN),
            inter(None, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
            inter(VOL_A, AB_STAGE, MIN),
            inter(VOL_A, AB_FINAL, MIN),
            prov(VOL_B, true, vec![2], MIN),
            inter(VOL_B, AB_STAGE, MIN),
            inter(VOL_B, AB_FINAL, MIN),
            prov(VOL_A, true, vec![5], MIN),
            inter(VOL_A, AB_STAGE, MIN),
            inter(VOL_A, AB_FINAL, MIN),
            prov(VOL_B, true, vec![8], MIN),
            inter(VOL_A, MR_STAGE, MIN),
            inter(VOL_A, MR_FINAL, MIN),
        ];
        rollback_context_testing(
            &host_status_list,
            "Clean install with ab updates and mid runtime rollback",
        );
    }

    #[test]
    fn test_offline_init_context() {
        let host_status_list = vec![
            prov(VOL_A, false, vec![], MIN),
            prov(VOL_A, false, vec![], MIN),
            prov(VOL_A, false, vec![], MIN),
        ];
        rollback_context_testing(&host_status_list, "Offline init initial state");
    }

    #[test]
    fn test_offline_init_and_ab_update_context() {
        let host_status_list = vec![
            prov(VOL_A, false, vec![], MIN),
            prov(VOL_A, false, vec![], MIN),
            prov(VOL_A, false, vec![], MIN),
            inter(VOL_A, AB_STAGE, MIN),
            inter(VOL_A, AB_FINAL, MIN),
            prov(VOL_B, true, vec![2], MIN),
        ];
        rollback_context_testing(&host_status_list, "Offline init and a/b update");
    }

    #[test]
    fn test_clean_install_context() {
        let host_status_list = vec![
            inter(None, CI_FINAL, MIN),
            inter(None, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
        ];
        rollback_context_testing(&host_status_list, "Clean install initial state");
    }

    #[test]
    fn test_clean_install_and_ab_update_context() {
        let host_status_list = vec![
            inter(None, CI_FINAL, MIN),
            inter(None, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
            inter(VOL_A, AB_STAGE, MIN),
            inter(VOL_A, AB_FINAL, MIN),
            prov(VOL_B, true, vec![2], MIN),
        ];
        rollback_context_testing(&host_status_list, "Clean install and a/b update");
    }

    #[test]
    fn test_with_old_trident_context() {
        let host_status_list = vec![
            inter(None, CI_FINAL, OLD),
            inter(None, CI_FINAL, OLD),
            prov(VOL_A, false, vec![], OLD),
            inter(VOL_A, AB_STAGE, OLD),
            inter(VOL_A, AB_FINAL, OLD),
            prov(VOL_B, false, vec![], OLD),
        ];
        rollback_context_testing(&host_status_list, "Old Trident versions");
    }

    #[test]
    fn test_with_no_trident_context() {
        let host_status_list = vec![
            inter(None, CI_FINAL, NONE),
            inter(None, CI_FINAL, NONE),
            prov(VOL_A, false, vec![], NONE),
            inter(VOL_A, AB_STAGE, NONE),
            inter(VOL_A, AB_FINAL, NONE),
            prov(VOL_B, false, vec![], NONE),
        ];
        rollback_context_testing(&host_status_list, "No Trident versions");
    }

    #[test]
    fn test_with_mixed_trident_context() {
        let host_status_list = vec![
            inter(None, CI_FINAL, NONE),
            inter(None, CI_FINAL, NONE),
            prov(VOL_A, false, vec![], NONE),
            inter(VOL_A, RU_STAGE, NONE),
            prov(VOL_A, false, vec![], NONE),
            inter(VOL_A, RU_STAGE, OLD),
            prov(VOL_A, false, vec![], OLD),
            inter(VOL_A, RU_STAGE, MIN),
            prov(VOL_A, false, vec![6], MIN),
            inter(VOL_A, RU_STAGE, NEW),
            prov(VOL_A, false, vec![8, 6], NEW),
        ];
        rollback_context_testing(
            &host_status_list,
            "Mixed Trident versions: none, old, min, new",
        );
    }

    #[test]
    fn test_ab_rollback_skipping_runtime_rollbacks() {
        let host_status_list = vec![
            inter(None, CI_FINAL, MIN),
            inter(None, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
            inter(VOL_A, AB_STAGE, MIN),
            inter(VOL_A, AB_FINAL, MIN),
            prov(VOL_B, true, vec![2], MIN),
            inter(VOL_B, RU_STAGE, MIN),
            prov(VOL_B, false, vec![5, 2], MIN),
            inter(VOL_B, RU_STAGE, MIN),
            prov(VOL_B, false, vec![7, 5, 2], MIN),
            // Manual Rollback of the available a/b update skips
            // 2 runtime updates
            inter(VOL_B, MR_STAGE, MIN),
            inter(VOL_B, MR_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
        ];
        rollback_context_testing(
            &host_status_list,
            "Validate a/b update rollback that skips runtime rollbacks",
        );
    }

    #[test]
    fn test_ab_staged_final_state() {
        let host_status_list = vec![
            inter(None, CI_FINAL, MIN),
            inter(None, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
            inter(VOL_A, AB_STAGE, MIN),
            inter(VOL_A, AB_FINAL, MIN),
            prov(VOL_B, true, vec![2], MIN),
            inter(VOL_B, AB_STAGE, MIN),
        ];
        rollback_context_testing_for_expected(
            &host_status_list,
            vec![],
            false,
            "Validate a/b update stage as final state",
        );
    }

    #[test]
    fn test_e2e_rollback() {
        let host_status_list = vec![
            inter(VOL_A, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
            inter(VOL_A, AB_STAGE, MIN),
            inter(VOL_A, AB_FINAL, MIN),
            prov(VOL_B, true, vec![1], MIN),
            inter(VOL_B, AB_STAGE, MIN),
            inter(VOL_B, AB_FINAL, MIN),
            inter_e(VOL_B, AB_HC_FAIL, MIN, Some("failure".to_string())),
            inter(VOL_B, AB_HC_FAIL, MIN),
            prov(VOL_B, false, vec![], MIN),
            prov_e(VOL_B, false, vec![], MIN, Some("failure".to_string())),
            prov(VOL_B, false, vec![], MIN),
            inter(VOL_B, AB_STAGE, MIN),
            inter(VOL_B, AB_FINAL, MIN),
            prov(VOL_A, true, vec![10], MIN),
        ];
        rollback_context_testing(&host_status_list, "E2E rollback scenario");
    }

    #[test]
    fn test_ab_update_health_check_failed() {
        let host_status_list = vec![
            inter(None, CI_FINAL, MIN),
            inter(None, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
            inter(VOL_A, AB_STAGE, MIN),
            inter(VOL_A, AB_FINAL, MIN),
            prov(VOL_B, true, vec![2], MIN),
            inter(VOL_B, AB_STAGE, MIN),
            inter(VOL_B, AB_FINAL, MIN),
            inter(VOL_B, AB_HC_FAIL, MIN),
            prov_e(VOL_B, false, vec![], MIN, Some("failure".to_string())),
        ];
        rollback_context_testing(&host_status_list, "Validate a/b update health check failed");
    }

    #[test]
    fn test_check() {
        let mut host_status_list = vec![
            inter(None, CI_FINAL, MIN),
            inter(None, CI_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
        ];
        let context = create_rollback_context_for_testing(&host_status_list);
        // if nothing is requested and there are no rollbacks, none is returned
        let (index, check_string) =
            get_requested_rollback_info(&context.get_rollback_chain().unwrap(), false, false)
                .unwrap();
        assert!(index.is_none());
        assert_eq!(check_string, "none");
        // if both ab and runtime rollback is requested simultaneously, error is returned
        let (index, check_string) =
            get_requested_rollback_info(&context.get_rollback_chain().unwrap(), true, false)
                .unwrap();
        assert!(index.is_none());
        assert_eq!(check_string, "none");
        // if both ab and runtime rollback is requested simultaneously, error is returned
        let (index, check_string) =
            get_requested_rollback_info(&context.get_rollback_chain().unwrap(), false, true)
                .unwrap();
        assert!(index.is_none());
        assert_eq!(check_string, "none");

        // Add some operations to datastore
        host_status_list.push(inter(VOL_A, AB_STAGE, MIN));
        host_status_list.push(inter(VOL_A, AB_FINAL, MIN));
        host_status_list.push(prov(VOL_B, true, vec![2], MIN));
        host_status_list.push(inter(VOL_B, RU_STAGE, MIN));
        host_status_list.push(prov(VOL_B, false, vec![5, 2], MIN));
        let context = create_rollback_context_for_testing(&host_status_list);
        // if runtime rollback is requested and it is the next rollback, return the index of the runtime rollback and 'runtime'
        let (index, check_string) =
            get_requested_rollback_info(&context.get_rollback_chain().unwrap(), false, false)
                .unwrap();
        assert_eq!(index, Some(0));
        assert_eq!(check_string, "runtime");
        // if ab rollback is requested and it is not the next rollback, return the index of the ab rollback and 'ab'
        let (index, check_string) =
            get_requested_rollback_info(&context.get_rollback_chain().unwrap(), false, true)
                .unwrap();
        assert_eq!(index, Some(1));
        assert_eq!(check_string, "ab");
        // if both ab and runtime rollback is requested simultaneously, error is returned
        assert!(
            get_requested_rollback_info(&context.get_rollback_chain().unwrap(), true, true)
                .is_err()
        );

        // Add an A/B update to database
        host_status_list.push(inter(VOL_B, AB_STAGE, MIN));
        host_status_list.push(inter(VOL_B, AB_FINAL, MIN));
        host_status_list.push(prov(VOL_B, true, vec![2], MIN));
        let context = create_rollback_context_for_testing(&host_status_list);
        // if runtime rollback is requested and it is not the next rollback, return an error
        assert!(
            get_requested_rollback_info(&context.get_rollback_chain().unwrap(), true, false)
                .is_err()
        );
    }
}
