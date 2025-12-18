use anyhow::{Context, Error};
use log::{info, trace};
use serde::{Deserialize, Serialize};

use trident_api::{
    error::{InvalidInputError, TridentError},
    status::{AbVolumeSelection, HostStatus, ServicingState, ServicingType},
};

const MINIMUM_ROLLBACK_TRIDENT_VERSION: &str = "0.21.0";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RollbackDetail {
    pub requires_reboot: bool,
    pub host_status: HostStatus,
    #[serde(skip)]
    host_status_index: i32,
}
pub struct ManualRollbackContext {
    volume_a_available_rollbacks: Vec<RollbackDetail>,
    volume_b_available_rollbacks: Vec<RollbackDetail>,
    active_volume: Option<AbVolumeSelection>,
    rollback_action: Option<ServicingType>,
    rollback_volume: Option<AbVolumeSelection>,
}
impl ManualRollbackContext {
    pub fn new(host_statuses: &[HostStatus]) -> Result<Self, TridentError> {
        let minimum_rollback_trident_semver =
            semver::Version::parse(MINIMUM_ROLLBACK_TRIDENT_VERSION).map_err(|e| {
                TridentError::new(InvalidInputError::InvalidRollbackExpectation {
                    reason: format!(
                        "Failed to parse minimum rollback Trident version '{MINIMUM_ROLLBACK_TRIDENT_VERSION}': {e}",
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
                        let last_error_exists = hs.last_error.is_some();
                        let encryption_configured = hs.spec.storage.encryption.is_some();
                        let active_volume_changed = hs.ab_active_volume != instance.active_volume;
                        let encryption_with_volume_change =
                            encryption_configured && active_volume_changed;
                        trace!(
                            "New Provisioned state detected at index {} for active volume {:?}, last_error_exists={}, trident_is_too_old={}, encryption_with_volume_change={}",
                            i,
                            instance.active_volume,
                            last_error_exists,
                            trident_is_too_old,
                            encryption_with_volume_change
                        );
                        // Prepend the last Provisioned index to the previously active volume's available
                        // rollbacks.
                        //
                        // There are a set of reasons to not add an available rollback:
                        //   1. The Trident version is too old to support manual rollback
                        //   2. If a last_error is set on the HostStatus
                        //   3. FOR NOW: if encryption is configured, as we do not yet support
                        //      manual rollback of ab update with encryption
                        match (
                            last_error_exists,
                            trident_is_too_old,
                            encryption_with_volume_change,
                            instance.active_volume,
                        ) {
                            (false, false, false, Some(AbVolumeSelection::VolumeA)) => {
                                instance
                                    .volume_a_available_rollbacks
                                    .insert(0, host_status_context);
                            }
                            (false, false, false, Some(AbVolumeSelection::VolumeB)) => {
                                instance
                                    .volume_b_available_rollbacks
                                    .insert(0, host_status_context);
                            }
                            // Do not add an available rollback for the following conditions
                            (true, _, _, _)
                            | (false, true, _, _)
                            | (false, false, true, _)
                            | (false, false, false, None) => {}
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
                needs_reboot = matches!(hs.servicing_state, ServicingState::AbUpdateFinalized);
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

    pub fn get_rollback_chain(&self) -> Result<Vec<RollbackDetail>, Error> {
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

    pub fn get_rollback_chain_yaml(&self) -> Result<String, Error> {
        let contexts = self.get_rollback_chain()?;
        let full_yaml =
            serde_yaml::to_string(&contexts).context("Failed to serialize rollback contexts")?;
        info!("Available rollbacks:\n{}", full_yaml);
        Ok(full_yaml)
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
}

/// Get requested rollback.
pub fn get_requested_rollback_info(
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

#[cfg(test)]
mod tests {
    use crate::TRIDENT_VERSION;
    use maplit::hashmap;
    use sysdefs::tpm2::Pcr;

    use super::*;

    fn get_requires_reboot(ctx: &ManualRollbackContext) -> bool {
        matches!(ctx.rollback_action, Some(ServicingType::AbUpdate))
    }

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
        encryption: bool,
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
        let host_config = trident_api::config::HostConfiguration {
            storage: trident_api::config::Storage {
                encryption: if encryption {
                    Some(trident_api::config::Encryption {
                        pcrs: vec![Pcr::Pcr4, Pcr::Pcr7, Pcr::Pcr11],
                        ..Default::default()
                    })
                } else {
                    None
                },
                ..Default::default()
            },
            ..Default::default()
        };
        HostStatus {
            spec: host_config,
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
                false,
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
                false,
            ),
            expected_requires_reboot,
            expected_available_rollbacks,
        }
    }
    fn prov_enc(
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
                true,
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
            host_status: host_status(active_volume, servicing_state, old_version, None, false),
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
            host_status: host_status(active_volume, servicing_state, old_version, error, false),
            expected_requires_reboot: false,
            expected_available_rollbacks: vec![],
        }
    }
    fn inter_enc(
        active_volume: Option<AbVolumeSelection>,
        servicing_state: ServicingState,
        old_version: &str,
    ) -> HostStatusTest {
        HostStatusTest {
            host_status: host_status(active_volume, servicing_state, old_version, None, true),
            expected_requires_reboot: false,
            expected_available_rollbacks: vec![],
        }
    }

    fn create_rollback_context_for_testing(
        host_status_test_list: &[HostStatusTest],
    ) -> ManualRollbackContext {
        let host_statuses = host_status_test_list
            .iter()
            .map(|hst| hst.host_status.clone())
            .collect::<Vec<_>>();
        ManualRollbackContext::new(&host_statuses).unwrap()
    }
    fn rollback_context_testing(host_status_test_list: &[HostStatusTest], test_description: &str) {
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
        assert_eq!(get_requires_reboot(&context), expected_requires_reboot);
        let serialized_output = serde_yaml::from_str::<Vec<serde_yaml::Value>>(
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
        for hs in host_status_list.iter() {
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
    fn test_ab_update_encryption() {
        let host_status_list = vec![
            inter_enc(None, CI_FINAL, MIN),
            inter_enc(None, CI_FINAL, MIN),
            prov_enc(VOL_A, false, vec![], MIN),
            inter_enc(VOL_A, AB_STAGE, MIN),
            inter_enc(VOL_A, AB_FINAL, MIN),
            prov_enc(VOL_B, false, vec![], MIN),
        ];
        rollback_context_testing(&host_status_list, "Validate a/b update with encryption");
    }

    #[test]
    fn test_runtime_update_encryption() {
        let host_status_list = vec![
            inter_enc(None, CI_FINAL, MIN),
            inter_enc(None, CI_FINAL, MIN),
            prov_enc(VOL_A, false, vec![], MIN),
            inter_enc(VOL_A, RU_STAGE, MIN),
            prov_enc(VOL_A, false, vec![2], MIN),
        ];
        rollback_context_testing(&host_status_list, "Validate runtime update with encryption");
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
