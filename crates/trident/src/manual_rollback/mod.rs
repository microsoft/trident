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
    cli::RollbackShowOperation,
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
/// Print whether the next manual rollback requires a reboot.
pub fn print_requires_reboot(datastore: &mut DataStore) -> Result<ExitKind, TridentError> {
    // Get all HostStatus entries from the datastore.
    let host_statuses = datastore
        .get_host_statuses()
        .message("Failed to get datastore HostStatus entries")?;
    // Create ManualRollback context from HostStatus entries.
    let context = ManualRollbackContext::new(&host_statuses)
        .message("Failed to create manual rollback context")?;

    let requires_reboot_output = context
        .get_requires_reboot_output()
        .structured(ServicingError::ManualRollback)
        .message("Failed to query for --requires-reboot")?;
    println!("{}", requires_reboot_output);
    Ok(ExitKind::Done)
}

pub fn print_show(
    datastore: &DataStore,
    show_operation: RollbackShowOperation,
) -> Result<ExitKind, TridentError> {
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

    match show_operation {
        RollbackShowOperation::Validation => {
            if let Some(first_rollback_host_status) = rollback_chain.first() {
                if first_rollback_host_status.requires_reboot {
                    info!("Next available rollback is A/B update rollback requiring reboot");
                    println!("ab");
                } else {
                    info!(
                        "Next available rollback is runtime update rollback not requiring reboot"
                    );
                    println!("runtime");
                }
            } else {
                info!("No available rollbacks to show validation for");
                println!("none");
            }
        }
        RollbackShowOperation::Target => {
            if let Some(first_rollback_host_status) = rollback_chain.first() {
                let target_output =
                    serde_json::to_string(&first_rollback_host_status.host_status.spec)
                        .structured(ServicingError::ManualRollback)
                        .message("Failed to serialize first rollback HostStatus spec")?;
                println!("{}", target_output);
            } else {
                info!("No available rollbacks to show target for");
                println!("{{}}");
            }
        }
        RollbackShowOperation::Chain => {
            let available_rollbacks_output = context
                .get_rollback_chain_json()
                .structured(ServicingError::ManualRollback)
                .message("Failed to query for --show=chain")?;
            println!("{}", available_rollbacks_output);
        }
    }
    Ok(ExitKind::Done)
}

/// Handle manual rollback operations.
pub fn execute_rollback(
    datastore: &mut DataStore,
    expected_runtime_rollback: bool,
    expected_ab_rollback: bool,
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
    if available_rollbacks.is_empty() {
        info!("No available rollbacks to perform");
        return Ok(ExitKind::Done);
    }

    let first_rollback = &available_rollbacks[0];
    if expected_runtime_rollback && first_rollback.requires_reboot {
        return Err(TridentError::new(
            InvalidInputError::InvalidRollbackExpectation {
                reason: "expected to undo a runtime update but rollback will undo an A/B update"
                    .to_string(),
            },
        ));
    }
    if expected_ab_rollback && !first_rollback.requires_reboot {
        return Err(TridentError::new(
            InvalidInputError::InvalidRollbackExpectation {
                reason: "expected to undo an A/B update but rollback will undo a runtime update"
                    .to_string(),
            },
        ));
    }

    let mut first_rollback_host_config = first_rollback.host_status.spec.clone();
    let mut skip_finalize_state_check = false;

    let mut engine_context = EngineContext {
        spec: first_rollback.host_status.spec.clone(),
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
                    reason: format!("in unexpected state: {:?}", state),
                }));
            }
        }

        stage_rollback(datastore, &engine_context, first_rollback.requires_reboot)
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
                        reason: format!("in unexpected state: {:?}", state),
                    }));
                }
            }
        }
        let finalize_result =
            finalize_rollback(datastore, &engine_context, first_rollback.requires_reboot)
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

fn stage_rollback(
    datastore: &mut DataStore,
    engine_context: &EngineContext,
    requires_reboot: bool,
) -> Result<(), TridentError> {
    if requires_reboot {
        info!("Staging rollback that requires reboot");
    } else {
        info!("Staging rollback that does not require reboot");
    }

    datastore.with_host_status(|host_status| {
        host_status.spec = engine_context.spec.clone();
        host_status.servicing_state = ServicingState::ManualRollbackStaged;
    })?;
    Ok(())
}

fn finalize_rollback(
    datastore: &mut DataStore,
    engine_context: &EngineContext,
    requires_reboot: bool,
) -> Result<ExitKind, TridentError> {
    if !requires_reboot {
        trace!("Manual rollback does not require reboot");
        // TODO: implement runtime update rollback

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
                encryption::get_binary_paths_pcrlock(engine_context, pcrs, None)
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
                        "Failed to parse minimum rollback Trident version '{}': {}",
                        minimum_rollback_trident_version, e
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
                if active_index != -1 {
                    let host_status_context = RollbackDetail {
                        host_status: host_statuses[active_index as usize].clone(),
                        host_status_index: active_index,
                        requires_reboot: needs_reboot,
                    };
                    if rollback {
                        if let Some((first_rollback, first_rollback_volume)) =
                            instance.get_first_rollback()
                        {
                            trace!(
                                "Rollback detected at index {} for active volume {:?}",
                                i,
                                instance.active_volume
                            );
                            match first_rollback_volume {
                                AbVolumeSelection::VolumeA => {
                                    if !instance.volume_a_available_rollbacks.is_empty() {
                                        instance.volume_a_available_rollbacks.remove(0);
                                    }
                                }
                                AbVolumeSelection::VolumeB => {
                                    if !instance.volume_b_available_rollbacks.is_empty() {
                                        instance.volume_b_available_rollbacks.remove(0);
                                    }
                                }
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
                        // Prepend the last Provisioned index to the previously active volume's available
                        // rollbacks.
                        match (trident_is_too_old, instance.active_volume) {
                            (false, Some(AbVolumeSelection::VolumeA)) => {
                                instance
                                    .volume_a_available_rollbacks
                                    .insert(0, host_status_context);
                            }
                            (false, Some(AbVolumeSelection::VolumeB)) => {
                                instance
                                    .volume_b_available_rollbacks
                                    .insert(0, host_status_context);
                            }
                            // Do not add an available rollback if there is no active volume
                            // or if the Trident version is too old
                            (true, _) | (false, None) => {}
                        }
                    }
                }
                // Update the context's active volume and index
                instance.active_volume = hs.ab_active_volume;
                active_index = i as i32;
                needs_reboot = false;
                // Reset the loop's rollback tracking
                rollback = false
            } else {
                // Check each non-Provisioned state to see if it represents a rollback action
                rollback = matches!(
                    hs.servicing_state,
                    ServicingState::ManualRollbackStaged | ServicingState::ManualRollbackFinalized
                );
                needs_reboot = matches!(
                    hs.servicing_state,
                    ServicingState::AbUpdateFinalized
                        | ServicingState::AbUpdateFinalized
                        | ServicingState::AbUpdateHealthCheckFailed
                );
                trace!(
                    "Detected servicing state {:?} at index {}: rollback={}, needs_reboot={}",
                    hs.servicing_state,
                    i,
                    rollback,
                    needs_reboot
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

    fn get_requires_reboot_output(&self) -> Result<String, Error> {
        let requires_reboot = self.get_requires_reboot()?;
        info!("Rollback requires reboot: {}", requires_reboot);
        Ok(requires_reboot.to_string())
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

    fn get_rollback_chain_json(&self) -> Result<String, Error> {
        let contexts = self.get_rollback_chain()?;
        let full_json =
            serde_json::to_string(&contexts).context("Failed to serialize rollback contexts")?;
        info!("Available rollbacks:\n{}", full_json);
        Ok(full_json)
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
    ) -> HostStatus {
        HostStatus {
            ab_active_volume: active_volume,
            servicing_state,
            trident_version: old_version.to_string(),
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
            host_status: host_status(active_volume, ServicingState::Provisioned, old_version),
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
            host_status: host_status(active_volume, servicing_state, old_version),
            expected_requires_reboot: false,
            expected_available_rollbacks: vec![],
        }
    }

    #[test]
    fn test_rollback_context() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let min = MINIMUM_ROLLBACK_TRIDENT_VERSION;
        let host_status_list = vec![
            inter(None, ServicingState::CleanInstallFinalized, min),
            inter(None, ServicingState::CleanInstallFinalized, min),
            prov(volume_a, false, vec![], min),
            inter(volume_a, ServicingState::RuntimeUpdateStaged, min),
            prov(volume_a, false, vec![2], min),
            inter(volume_a, ServicingState::RuntimeUpdateStaged, min),
            prov(volume_a, false, vec![4, 2], min),
            inter(volume_a, ServicingState::AbUpdateStaged, min),
            inter(volume_a, ServicingState::AbUpdateFinalized, min),
            prov(volume_b, true, vec![6, 4, 2], min),
            inter(volume_b, ServicingState::AbUpdateStaged, min),
            inter(volume_b, ServicingState::AbUpdateFinalized, min),
            prov(volume_a, true, vec![9], min),
            inter(volume_a, ServicingState::ManualRollbackStaged, min),
            inter(volume_a, ServicingState::ManualRollbackFinalized, min),
            prov(volume_b, false, vec![], min),
        ];
        for (i, hs) in host_status_list.iter().enumerate() {
            if hs.host_status.servicing_state != ServicingState::Provisioned {
                continue;
            }
            let host_status_list = host_status_list
                .iter()
                .take(i + 1)
                .map(|hst| hst.host_status.clone())
                .collect::<Vec<_>>();
            let context = ManualRollbackContext::new(&host_status_list).unwrap();
            trace!(
                "HS: {:?}, expected_requires_reboot: {}, expected_available_rollbacks: {:?}",
                hs.host_status.servicing_state,
                hs.expected_requires_reboot,
                hs.expected_available_rollbacks
            );
            assert_eq!(
                context.get_requires_reboot_output().unwrap(),
                hs.expected_requires_reboot.to_string()
            );
            let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
                &context.get_rollback_chain_json().unwrap(),
            )
            .unwrap();
            assert_eq!(
                serialized_output.len(),
                hs.expected_available_rollbacks.len()
            )
        }
    }

    #[test]
    fn test_runtime_rollback_context_mid_rollback() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let min = MINIMUM_ROLLBACK_TRIDENT_VERSION;
        let host_status_list = vec![
            inter(None, ServicingState::CleanInstallFinalized, min),
            inter(None, ServicingState::CleanInstallFinalized, min),
            prov(volume_a, false, vec![], min),
            inter(volume_a, ServicingState::RuntimeUpdateStaged, min),
            prov(volume_a, false, vec![2], min),
            inter(volume_a, ServicingState::RuntimeUpdateStaged, min),
            prov(volume_a, false, vec![4, 2], min),
            inter(volume_a, ServicingState::RuntimeUpdateStaged, min),
            prov(volume_a, false, vec![6, 4, 2], min),
            inter(volume_a, ServicingState::ManualRollbackStaged, min),
            inter(volume_a, ServicingState::ManualRollbackFinalized, min),
        ];
        let host_status_list = host_status_list
            .iter()
            .take(host_status_list.len() + 1)
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        let context = ManualRollbackContext::new(&host_status_list).unwrap();
        trace!(
            "Validate runtime update rollback, create context in manual-rollback-finalized state"
        );
        // Manual rollback undoing a runtime update does not require reboot
        assert_eq!(
            context.get_requires_reboot_output().unwrap(),
            false.to_string()
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_json().unwrap(),
        )
        .unwrap();
        // Pre manual rollback, there were 3 runtime updates to rollback
        assert_eq!(serialized_output.len(), 3)
    }

    #[test]
    fn test_ab_rollback_context_mid_rollback() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let min = MINIMUM_ROLLBACK_TRIDENT_VERSION;
        let host_status_list = vec![
            inter(None, ServicingState::CleanInstallFinalized, min),
            inter(None, ServicingState::CleanInstallFinalized, min),
            prov(volume_a, false, vec![], min),
            inter(volume_a, ServicingState::AbUpdateStaged, min),
            inter(volume_a, ServicingState::AbUpdateFinalized, min),
            prov(volume_b, true, vec![2], min),
            inter(volume_b, ServicingState::AbUpdateStaged, min),
            inter(volume_b, ServicingState::AbUpdateFinalized, min),
            prov(volume_a, true, vec![5], min),
            inter(volume_a, ServicingState::AbUpdateStaged, min),
            inter(volume_a, ServicingState::AbUpdateFinalized, min),
            prov(volume_b, true, vec![8], min),
            inter(volume_a, ServicingState::ManualRollbackStaged, min),
            inter(volume_a, ServicingState::ManualRollbackFinalized, min),
        ];
        let host_status_list = host_status_list
            .iter()
            .take(host_status_list.len() + 1)
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        let context = ManualRollbackContext::new(&host_status_list).unwrap();
        trace!(
            "Validate runtime update rollback, create context in manual-rollback-finalized state"
        );
        // Manual rollback undoing a runtime update does not require reboot
        assert_eq!(
            context.get_requires_reboot_output().unwrap(),
            true.to_string()
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_json().unwrap(),
        )
        .unwrap();
        // Pre a/b rollback, there was 1 runtime update to rollback
        assert_eq!(serialized_output.len(), 1)
    }

    #[test]
    fn test_offline_init_context() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let min = MINIMUM_ROLLBACK_TRIDENT_VERSION;
        let host_status_list = vec![
            prov(volume_a, false, vec![], min),
            prov(volume_a, false, vec![], min),
            prov(volume_a, false, vec![], min),
        ];
        let host_status_list = host_status_list
            .iter()
            .take(host_status_list.len() + 1)
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        let context = ManualRollbackContext::new(&host_status_list).unwrap();
        trace!("Validate create context for offline-init initial state");
        // There should be NO available rollbacks, as there hasn't been any updates yet
        assert_eq!(
            context.get_requires_reboot_output().unwrap(),
            false.to_string()
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_json().unwrap(),
        )
        .unwrap();
        // Only offline-init has run, so there should be 0 updates to rollback
        assert_eq!(serialized_output.len(), 0)
    }

    #[test]
    fn test_offline_init_and_ab_update_context() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let min = MINIMUM_ROLLBACK_TRIDENT_VERSION;
        let host_status_list = vec![
            prov(volume_a, false, vec![], min),
            prov(volume_a, false, vec![], min),
            prov(volume_a, false, vec![], min),
            inter(volume_a, ServicingState::AbUpdateStaged, min),
            inter(volume_a, ServicingState::AbUpdateFinalized, min),
            prov(volume_b, true, vec![2], min),
        ];
        let host_status_list = host_status_list
            .iter()
            .take(host_status_list.len() + 1)
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        let context = ManualRollbackContext::new(&host_status_list).unwrap();
        trace!("Validate create context for offline-init initial state");
        // There should be 1 available rollback
        assert_eq!(
            context.get_requires_reboot_output().unwrap(),
            true.to_string()
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_json().unwrap(),
        )
        .unwrap();
        assert_eq!(serialized_output.len(), 1)
    }

    #[test]
    fn test_clean_install_context() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let min = MINIMUM_ROLLBACK_TRIDENT_VERSION;
        let host_status_list = vec![
            inter(None, ServicingState::CleanInstallFinalized, min),
            inter(None, ServicingState::CleanInstallFinalized, min),
            prov(volume_a, false, vec![], min),
        ];
        let host_status_list = host_status_list
            .iter()
            .take(host_status_list.len() + 1)
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        let context = ManualRollbackContext::new(&host_status_list).unwrap();
        // There should be 0 available rollbacks
        assert_eq!(
            context.get_requires_reboot_output().unwrap(),
            false.to_string()
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_json().unwrap(),
        )
        .unwrap();
        assert_eq!(serialized_output.len(), 0)
    }

    #[test]
    fn test_clean_install_and_ab_update_context() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let min = MINIMUM_ROLLBACK_TRIDENT_VERSION;
        let host_status_list = vec![
            inter(None, ServicingState::CleanInstallFinalized, min),
            inter(None, ServicingState::CleanInstallFinalized, min),
            prov(volume_a, false, vec![], min),
            inter(volume_a, ServicingState::AbUpdateStaged, min),
            inter(volume_a, ServicingState::AbUpdateFinalized, min),
            prov(volume_b, true, vec![2], min),
        ];
        let host_status_list = host_status_list
            .iter()
            .take(host_status_list.len() + 1)
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        let context = ManualRollbackContext::new(&host_status_list).unwrap();
        // There should be 1 available rollbacks for an ab update
        assert_eq!(
            context.get_requires_reboot_output().unwrap(),
            true.to_string()
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_json().unwrap(),
        )
        .unwrap();
        assert_eq!(serialized_output.len(), 1)
    }

    #[test]
    fn test_with_old_trident_context() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let old = "0.19.0";
        let host_status_list = vec![
            inter(None, ServicingState::CleanInstallFinalized, old),
            inter(None, ServicingState::CleanInstallFinalized, old),
            prov(volume_a, false, vec![], old),
            inter(volume_a, ServicingState::AbUpdateStaged, old),
            inter(volume_a, ServicingState::AbUpdateFinalized, old),
            prov(volume_b, true, vec![2], old),
        ];
        let host_status_list = host_status_list
            .iter()
            .take(host_status_list.len() + 1)
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        let context = ManualRollbackContext::new(&host_status_list).unwrap();
        // There should be 0 available rollbacks
        assert_eq!(
            context.get_requires_reboot_output().unwrap(),
            false.to_string()
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_json().unwrap(),
        )
        .unwrap();
        assert_eq!(serialized_output.len(), 0)
    }

    #[test]
    fn test_with_no_trident_context() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let none = "";
        let host_status_list = vec![
            inter(None, ServicingState::CleanInstallFinalized, none),
            inter(None, ServicingState::CleanInstallFinalized, none),
            prov(volume_a, false, vec![], none),
            inter(volume_a, ServicingState::AbUpdateStaged, none),
            inter(volume_a, ServicingState::AbUpdateFinalized, none),
            prov(volume_b, true, vec![2], none),
        ];
        let host_status_list = host_status_list
            .iter()
            .take(host_status_list.len() + 1)
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        let context = ManualRollbackContext::new(&host_status_list).unwrap();
        // There should be 0 available rollbacks
        assert_eq!(
            context.get_requires_reboot_output().unwrap(),
            false.to_string()
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_json().unwrap(),
        )
        .unwrap();
        assert_eq!(serialized_output.len(), 0)
    }

    #[test]
    fn test_with_mixed_trident_context() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let new = TRIDENT_VERSION;
        let min = MINIMUM_ROLLBACK_TRIDENT_VERSION;
        let old = "0.19.0";
        let none = "";

        let host_status_list = vec![
            inter(None, ServicingState::CleanInstallFinalized, none),
            inter(None, ServicingState::CleanInstallFinalized, none),
            prov(volume_a, false, vec![], none),
            inter(volume_a, ServicingState::RuntimeUpdateStaged, none),
            prov(volume_a, false, vec![], none),
            inter(volume_a, ServicingState::RuntimeUpdateStaged, old),
            prov(volume_a, false, vec![], old),
            inter(volume_a, ServicingState::RuntimeUpdateStaged, min),
            prov(volume_a, false, vec![6], min),
            inter(volume_a, ServicingState::RuntimeUpdateStaged, new),
            prov(volume_a, false, vec![8], new),
        ];
        let host_status_list = host_status_list
            .iter()
            .take(host_status_list.len() + 1)
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        let context = ManualRollbackContext::new(&host_status_list).unwrap();
        // There should be 2 available rollbacks, both runtime updates
        assert_eq!(
            context.get_requires_reboot_output().unwrap(),
            false.to_string()
        );
        let serialized_output = serde_yaml::from_str::<Vec<RollbackDetail>>(
            &context.get_rollback_chain_json().unwrap(),
        )
        .unwrap();
        assert_eq!(serialized_output.len(), 2)
    }
}
