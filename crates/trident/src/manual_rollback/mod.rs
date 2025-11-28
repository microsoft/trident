#![allow(unused)]

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{bail, Context, Error};
use log::{debug, info, trace};

use maplit::hashmap;
use osutils::lsblk;
use trident_api::{
    config::{
        AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
        MountOptions, MountPoint, Partition, PartitionSize, PartitionTableType, PartitionType,
        VerityCorruptionOption, VerityDevice,
    },
    constants::internal_params::ENABLE_UKI_SUPPORT,
    error::{InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{decode_host_status, AbVolumeSelection, HostStatus, ServicingState, ServicingType},
    BlockDeviceId,
};
use uuid::Uuid;

use crate::{
    datastore::{self, DataStore},
    ExitKind,
};

/// Handle manual rollback operations.
pub fn execute(
    datastore: &mut DataStore,
    expected_runtime_rollback: bool,
    expected_ab_rollback: bool,
    query_requires_reboot: bool,
    show_available_rollbacks: bool,
) -> Result<ExitKind, TridentError> {
    // Get all HostStatus entries from the datastore.
    let host_statuses = datastore
        .get_host_statuses()
        .message("Failed to get datastore HostStatus entries")?;
    // Create ManualRollback context from HostStatus entries.
    let context = ManualRollbackContext::new(&host_statuses)
        .message("Failed to create manual rollback context")?;

    if query_requires_reboot {
        let requires_reboot_output = context
            .get_requires_reboot_output()
            .structured(ServicingError::ManualRollback)
            .message("Failed to query for --requires-reboot")?;
        println!("{}", requires_reboot_output);
        return Ok(ExitKind::Done);
    }

    if show_available_rollbacks {
        let available_rollbacks_output = context
            .get_available_rollbacks_output()
            .structured(ServicingError::ManualRollback)
            .message("Failed to query for --show-available-rollbacks")?;
        println!("{}", available_rollbacks_output);
        return Ok(ExitKind::Done);
    }

    Ok(ExitKind::Done)
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct RollbackDetail {
    host_status: HostStatus,
    host_status_index: i32,
    requires_reboot: bool,
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
        // Initialize context from HostStatus entries.
        let mut instance = ManualRollbackContext {
            volume_a_available_rollbacks: Vec::new(),
            volume_b_available_rollbacks: Vec::new(),
            active_volume: None,
            rollback_action: None,
            rollback_volume: None,
        };

        let mut rollback = false;
        let mut needs_reboot = false;
        let mut active_index = -1;
        for (i, hs) in host_statuses.iter().enumerate() {
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
                                active_index,
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
                    } else {
                        trace!(
                            "New Provisioned state detected at index {} for active volume {:?}",
                            active_index,
                            instance.active_volume
                        );
                        // Prepend the last Provisioned index to the previously active volume's available
                        // rollbacks.
                        match instance.active_volume {
                            Some(AbVolumeSelection::VolumeA) => {
                                instance
                                    .volume_a_available_rollbacks
                                    .insert(0, host_status_context);
                            }
                            Some(AbVolumeSelection::VolumeB) => {
                                instance
                                    .volume_b_available_rollbacks
                                    .insert(0, host_status_context);
                            }
                            None => {}
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

    fn get_first_rollback(&self) -> Option<(i32, AbVolumeSelection)> {
        let mut rollback_a = -1;
        let mut rollback_b = -1;
        if !self.volume_a_available_rollbacks.is_empty() {
            rollback_a = self.volume_a_available_rollbacks[0].host_status_index;
        }
        if !self.volume_b_available_rollbacks.is_empty() {
            rollback_b = self.volume_b_available_rollbacks[0].host_status_index;
        }
        if rollback_a > rollback_b {
            return Some((rollback_a, AbVolumeSelection::VolumeA));
        }
        if rollback_b != -1 {
            return Some((rollback_b, AbVolumeSelection::VolumeB));
        }
        None
    }

    fn get_requires_reboot_output(&self) -> Result<String, Error> {
        let requires_reboot = match self.rollback_action {
            Some(ServicingType::AbUpdate) => "true",
            _ => "false",
        };
        info!("Rollback requires reboot: {}", requires_reboot);
        Ok(requires_reboot.to_string())
    }

    fn get_available_rollbacks_output(&self) -> Result<String, Error> {
        let mut contexts = self
            .volume_a_available_rollbacks
            .clone()
            .into_iter()
            .chain(self.volume_b_available_rollbacks.clone())
            .collect::<Vec<_>>();
        contexts.sort_by(|a, b| b.host_status_index.cmp(&a.host_status_index));
        let full_yaml =
            serde_yaml::to_string(&contexts).context("Failed to serialize rollback contexts")?;
        info!("Available rollbacks:\n{}", full_yaml);
        Ok(full_yaml)
    }
}

#[cfg(test)]
mod tests {
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
    ) -> HostStatus {
        HostStatus {
            ab_active_volume: active_volume,
            servicing_state,
            ..Default::default()
        }
    }
    fn prov(
        active_volume: Option<AbVolumeSelection>,
        expected_requires_reboot: bool,
        expected_available_rollbacks: Vec<usize>,
    ) -> HostStatusTest {
        HostStatusTest {
            host_status: host_status(active_volume, ServicingState::Provisioned),
            expected_requires_reboot,
            expected_available_rollbacks,
        }
    }
    fn inter(
        active_volume: Option<AbVolumeSelection>,
        servicing_state: ServicingState,
    ) -> HostStatusTest {
        HostStatusTest {
            host_status: host_status(active_volume, servicing_state),
            expected_requires_reboot: false,
            expected_available_rollbacks: vec![],
        }
    }

    #[test]
    fn test_something() {
        let volume_a = Some(AbVolumeSelection::VolumeA);
        let volume_b = Some(AbVolumeSelection::VolumeB);
        let host_status_list = vec![
            inter(None, ServicingState::CleanInstallFinalized),
            inter(None, ServicingState::CleanInstallFinalized),
            prov(volume_a, false, vec![]),
            inter(volume_a, ServicingState::RuntimeUpdateStaged),
            prov(volume_a, false, vec![2]),
            inter(volume_a, ServicingState::RuntimeUpdateStaged),
            prov(volume_a, false, vec![4, 2]),
            inter(volume_a, ServicingState::AbUpdateStaged),
            inter(volume_a, ServicingState::AbUpdateFinalized),
            prov(volume_b, true, vec![6, 4, 2]),
            inter(volume_b, ServicingState::AbUpdateStaged),
            inter(volume_b, ServicingState::AbUpdateFinalized),
            prov(volume_a, true, vec![9]),
            inter(volume_a, ServicingState::ManualRollbackStaged),
            inter(volume_a, ServicingState::ManualRollbackFinalized),
            prov(volume_b, false, vec![]),
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
                &context.get_available_rollbacks_output().unwrap(),
            )
            .unwrap();
            assert_eq!(
                serialized_output.len(),
                hs.expected_available_rollbacks.len()
            )
        }
    }
}
