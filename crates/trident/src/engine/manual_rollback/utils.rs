use anyhow::{Context, Error};
use lazy_static::lazy_static;
use log::{info, trace};
use semver::Version;
use serde::{Deserialize, Serialize};

use trident_api::{
    config::HostConfiguration,
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
    status::{AbVolumeSelection, HostStatus, ServicingState, TridentVersion},
};

/// Minimum Trident version that supports manual rollback.
const MINIMUM_ROLLBACK_TRIDENT_VERSION_STR: &str = "0.21.0";
lazy_static! {
    /// SemVer instance for minimum rollback Trident version.
    static ref MINIMUM_ROLLBACK_TRIDENT_VERSION: Version =
        Version::parse(MINIMUM_ROLLBACK_TRIDENT_VERSION_STR)
            .expect("Failed to parse minimum rollback Trident version");
}

pub enum ManualRollbackRequestKind {
    RollbackOnlyIfNextIsRuntimeUpdate,
    RollbackAvailableAbUpdate,
    RollbackNext,
}

impl ManualRollbackRequestKind {
    pub fn from_flags(
        invoke_if_next_is_runtime: bool,
        invoke_available_ab: bool,
    ) -> Result<Self, TridentError> {
        match (invoke_if_next_is_runtime, invoke_available_ab) {
            (false, false) => Ok(ManualRollbackRequestKind::RollbackNext),
            (true, false) => Ok(ManualRollbackRequestKind::RollbackOnlyIfNextIsRuntimeUpdate),
            (false, true) => Ok(ManualRollbackRequestKind::RollbackAvailableAbUpdate),
            (true, true) => Err(TridentError::new(
                InvalidInputError::InvalidRollbackExpectation {
                    reason: "conflicting expectations: cannot expect to undo both a runtime update and an A/B update"
                        .to_string(),
                },
            )),
        }
    }
}

/// ManualRollbackKind represents the kind of manual rollback.
#[derive(clap::ValueEnum, Copy, Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ManualRollbackKind {
    /// Rollback of an A/B update that requires a reboot.
    Ab,
    /// Rollback of a runtime update that does not require a reboot.
    Runtime,
}

/// ManualRollbackChainItem represents an available rollback.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManualRollbackChainItem {
    /// The kind of manual rollback, either A/B or runtime.
    pub kind: ManualRollbackKind,
    /// The HostConfiguration that the rollback will restore.
    pub spec: HostConfiguration,
    /// The active volume that the rollback will restore.
    pub ab_active_volume: Option<AbVolumeSelection>,
    /// The install index of the rollback.
    pub install_index: usize,
    /// The index of the HostStatus entry that this rollback was
    /// derived from, used internally to maintian ordering.
    #[serde(skip)]
    host_status_index: i32,
}

/// ManualRollbackContext tracks available rollbacks for each volume and
/// provides methods to query them.
pub(crate) struct ManualRollbackContext {
    /// Track the available rollbacks for volume A.
    volume_a_available_rollbacks: Vec<ManualRollbackChainItem>,
    /// Track the available rollbacks for volume B.
    volume_b_available_rollbacks: Vec<ManualRollbackChainItem>,
    /// Track the currently active volume.
    active_volume: Option<AbVolumeSelection>,
}
impl ManualRollbackContext {
    /// Creates a new ManualRollbackContext from a list of HostStatus entries.
    /// Start from the oldest HostStatus and work forward to the newest,
    /// tracking available rollbacks for each volume.
    ///
    /// For any operation (install, update, rollback), there are a series of
    /// HostStatus entries that represent the state transitions. These related
    /// states can be used to infer what operations were performed, and what
    /// volumes were affected, and thus what rollbacks are available.
    ///
    /// The list of available rollbacks for each volume can be:
    /// - active volume: 0 or more runtime rollbacks
    /// - inactive volume: 1 A/B update followed by 0 or more runtime rollbacks
    ///
    /// As each HostStatus is processed, the available rollback lists are updated
    /// for each successful Provisioned state.
    /// - A Provisioned state that was reached via an update operation will be added
    ///   to the list of available rollbacks for the servicing volume.
    /// - A Provisioned state that was reached via a runtime manual-rollback operation
    ///   will remove the next available rollback(s) from the active volume available
    ///   rollbacks list.
    /// - A Provisioned state that was reached via an A/B manual-rollback operation
    ///   will remove all of the available rollback(s) from the active volume available
    ///   rollbacks list and the first available rollback from the inactive volume.
    ///
    /// There are a some complications:
    /// - Only Trident versions >= MINIMUM_ROLLBACK_TRIDENT_VERSION_STR support
    ///   manual rollback. Older versions will not have rollbacks added.
    /// - If a last_error is set on the HostStatus, no rollback will be added.
    /// - The database may have consequutive 'duplicate' Provisioned states; these
    ///   are typically found after offline-init and should be consolidated into a
    ///   single state for rollback purposes.
    /// - ManualRollback must be considered, typically by popping the appropriate
    ///   available rollback(s) from the active volume available rollbacks list.
    /// - Auto-rollback must be considered, typically by not adding the an available
    ///   rollback for the Provisioned state reached via auto-rollback.
    pub fn new(host_statuses: &[HostStatus]) -> Result<Self, TridentError> {
        // Initialize context from HostStatus entries.
        let mut instance = ManualRollbackContext {
            volume_a_available_rollbacks: Vec::new(),
            volume_b_available_rollbacks: Vec::new(),
            active_volume: None,
        };

        // Track state as HostStatus entries are processed
        let mut is_auto_rollback = false;
        let mut is_manual_rollback = false;
        let mut is_ab_update = false;
        let mut last_provisioned_index = -1;

        for (i, hs) in host_statuses.iter().enumerate() {
            trace!(
                "Processing HostStatus at index {}: servicing_state='{:?}', ab_active_volume='{:?}'",
                i,
                hs.servicing_state,
                hs.ab_active_volume,
            );
            // An A/B update overwrites the inactive volume, so the
            // inactive volume will no longer have any available rollbacks.
            // Clear the ManualRollbackContext's available rollback list
            // for the inactive volume to reflect this.
            if hs.servicing_state == ServicingState::AbUpdateStaged {
                trace!("'AbUpdateStaged' detected at index {}: clearing available rollbacks for inactive volume '{:?}': a:[{:?}] b:[{:?}]",
                    i,
                    hs.ab_active_volume,
                    instance.volume_a_available_rollbacks.len(),
                    instance.volume_b_available_rollbacks.len()
                );
                if let Some(volume) = hs.ab_active_volume {
                    instance.clear_available_rollbacks(volume, true);
                }
            }

            // Handle each HostStatus entry based on whether it's a Provisioned
            // state or not.
            //
            // Available rollbacks are added when Provisioned states are processed.
            // Other states are used to track what type of operation (install, runtime
            // update, A/B update, auto-rollback, manual-rollback) was performed
            // to reach the Provisioned state.
            if hs.servicing_state == ServicingState::Provisioned {
                trace!(
                    "Processing 'Provisioned' state at index {} for active volume '{:?}'",
                    i,
                    hs.ab_active_volume
                );

                // Update context if a new (non-repeated) Provisioned state is
                // processed that is not the first Provisioned state (which would
                // be install or offline-init, and not something that can be rolled
                // back).
                if last_provisioned_index != -1 && last_provisioned_index != (i - 1) as i32 {
                    // Create the rollback chain item for this Provisioned state
                    let host_status_context = ManualRollbackChainItem {
                        spec: host_statuses[last_provisioned_index as usize].spec.clone(),
                        ab_active_volume: host_statuses[last_provisioned_index as usize]
                            .ab_active_volume,
                        install_index: host_statuses[last_provisioned_index as usize].install_index,
                        kind: if is_ab_update {
                            ManualRollbackKind::Ab
                        } else {
                            ManualRollbackKind::Runtime
                        },
                        host_status_index: last_provisioned_index,
                    };
                    if is_auto_rollback {
                        // If this Provisioned state was reached via an auto-rollback,
                        // it cannot be rolled back from, so do not add it to the available
                        // rollbacks.
                        trace!(
                            "Auto-rollback detected at index {} for active volume '{:?}'",
                            i,
                            instance.active_volume
                        );
                    } else if is_manual_rollback {
                        // If this Provisioned state was reached via a manual rollback,
                        // we need to remove the appropriate available rollback(s).
                        let active_volume_changed = hs.ab_active_volume != instance.active_volume;
                        trace!(
                            "Manual rollback detected at index {} for active volume '{:?}', active_volume_changed={}",
                            i,
                            instance.active_volume,
                            active_volume_changed
                        );
                        if active_volume_changed {
                            // If the active volume changed during a manual rollback, then:
                            //   1. All of the available rollbacks for the previously active
                            //      volume can be removed. If these exist, they are runtime
                            //      updates that were applied after the A/B udpate that is
                            //      being rolled back.
                            if let Some(volume) = instance.active_volume {
                                trace!(
                                    "Active volume changed during manual rollback at index {}: clearing available rollbacks for previously active volume '{:?}': a:[{:?}] b:[{:?}]",
                                    i,
                                    volume,
                                    instance.volume_a_available_rollbacks.len(),
                                    instance.volume_b_available_rollbacks.len()
                                );
                                instance.clear_available_rollbacks(volume, false);
                            }
                            //   2. The first available rollback for the newly active volume,
                            //      which is the A/B update being rolled back, can be removed.
                            if let Some(volume) = hs.ab_active_volume {
                                trace!(
                                    "Removing first available rollback for newly active volume '{:?}' during manual rollback at index {}",
                                    volume,
                                    i
                                );
                                instance.remove_available_rollback(volume);
                            }
                        } else {
                            // If the active volume did not change, then a runtime rollback was performed
                            // and we can remove the first available rollback for the active volume
                            if let Some(volume) = instance.active_volume {
                                trace!(
                                    "Removing first available rollback for active volume '{:?}' during manual rollback at index {}",
                                    volume,
                                    i
                                );
                                instance.remove_available_rollback(volume);
                            }
                        }
                    } else {
                        // This provisioned state was reached via an update operation.
                        // Add this update to the update's servicing volume's available
                        // rollbacks unless prevented by one of these conditions:
                        //   1. The Trident version is too old to support manual rollback
                        //   2. If a last_error is set on the HostStatus
                        let trident_is_compatible =
                            Self::is_trident_version_compatible(hs.trident_version.clone())?;
                        let last_error_exists = hs.last_error.is_some();
                        trace!(
                            "New 'Provisioned' state detected at index {} for active volume '{:?}', last_error_exists={}, trident_compatible={}",
                            i,
                            instance.active_volume,
                            last_error_exists,
                            trident_is_compatible
                        );
                        if trident_is_compatible && !last_error_exists {
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
                }
                // Update the context's active volume
                instance.active_volume = hs.ab_active_volume;
                // Track last provisioned state index
                last_provisioned_index = i as i32;
                // Reset the loop's update-type tracking
                is_ab_update = false;
                // Reset the loop's manual rollback tracking
                is_manual_rollback = false;
                // Reset the loop's auto-rollback tracking
                is_auto_rollback = false;
            } else {
                // Update tracking for non-Provisioned state
                match hs.servicing_state {
                    ServicingState::ManualRollbackAbStaged
                    | ServicingState::ManualRollbackRuntimeStaged
                    | ServicingState::ManualRollbackAbFinalized => {
                        is_manual_rollback = true;
                    }
                    ServicingState::AbUpdateStaged | ServicingState::AbUpdateFinalized => {
                        is_ab_update = true;
                    }
                    ServicingState::AbUpdateHealthCheckFailed => {
                        is_auto_rollback = true;
                    }
                    _ => {}
                }
            }
        }
        Ok(instance)
    }

    /// Get the full rollback chain
    pub fn get_rollback_chain(&self) -> Result<Vec<ManualRollbackChainItem>, Error> {
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

    /// Get the full rollback chain as YAML string
    pub fn get_rollback_chain_yaml(&self) -> Result<String, Error> {
        let contexts = self.get_rollback_chain()?;
        let full_yaml =
            serde_yaml::to_string(&contexts).context("Failed to serialize rollback contexts")?;
        info!("Available rollbacks:\n{}", full_yaml);
        Ok(full_yaml)
    }

    /// Clear available rollbacks for a given active/inactive volume.
    fn clear_available_rollbacks(&mut self, volume: AbVolumeSelection, inactive: bool) {
        match (inactive, volume) {
            (false, AbVolumeSelection::VolumeA) => self.volume_a_available_rollbacks.clear(),
            (false, AbVolumeSelection::VolumeB) => self.volume_b_available_rollbacks.clear(),
            (true, AbVolumeSelection::VolumeA) => self.volume_b_available_rollbacks.clear(),
            (true, AbVolumeSelection::VolumeB) => self.volume_a_available_rollbacks.clear(),
        }
    }

    /// Remove the first available rollback for a given volume.
    fn remove_available_rollback(&mut self, volume: AbVolumeSelection) {
        match volume {
            AbVolumeSelection::VolumeA => {
                if !self.volume_a_available_rollbacks.is_empty() {
                    self.volume_a_available_rollbacks.remove(0);
                }
            }
            AbVolumeSelection::VolumeB => {
                if !self.volume_b_available_rollbacks.is_empty() {
                    self.volume_b_available_rollbacks.remove(0);
                }
            }
        }
    }

    /// Check if the given Trident version is compatible with manual rollback.
    fn is_trident_version_compatible(
        trident_version: TridentVersion,
    ) -> Result<bool, TridentError> {
        let trident_is_compatible = match trident_version {
            // If version is not set or is not semver, consider it incompatible
            TridentVersion::Other(_) | TridentVersion::None => false,
            TridentVersion::SemVer(version) => version >= *MINIMUM_ROLLBACK_TRIDENT_VERSION,
        };
        Ok(trident_is_compatible)
    }

    /// Get detail for requested rollback.
    /// * If there are no rollbacks available, return None.
    /// * If no request specifications are made, return the next available rollback.
    /// * If ab is requested and
    ///     + available in the chain, return it.
    ///     - no ab updates are available in the chain, return error.
    /// * If runtime is requested and
    ///     + the next available is runtime, return it.
    ///     - the next available is ab, return error.
    /// * If both ab and runtime are requested, return error.
    pub fn get_requested_rollback(
        &self,
        requested_rollback_kind: ManualRollbackRequestKind,
    ) -> Result<Option<ManualRollbackChainItem>, TridentError> {
        let available_rollbacks =
            self.get_rollback_chain()
                .structured(ServicingError::ManualRollback {
                    message: "Failed to get available rollbacks",
                })?;

        if available_rollbacks.is_empty() {
            return Ok(None);
        }

        match requested_rollback_kind {
            ManualRollbackRequestKind::RollbackNext => {
                // No expectations specified, proceed with first
                Ok(Some(available_rollbacks[0].clone()))
            }
            ManualRollbackRequestKind::RollbackOnlyIfNextIsRuntimeUpdate => {
                // Expecting runtime rollback as first
                if matches!(available_rollbacks[0].kind, ManualRollbackKind::Ab) {
                    return Err(TridentError::new(
                        InvalidInputError::InvalidRollbackExpectation {
                            reason:
                                "expected to undo a runtime update but rollback will undo an A/B update"
                                    .to_string(),
                        },
                    ));
                }
                Ok(Some(available_rollbacks[0].clone()))
            }
            ManualRollbackRequestKind::RollbackAvailableAbUpdate => {
                // Find first A/B rollback along with its index
                let Some((index, _)) = available_rollbacks
                    .iter()
                    .enumerate()
                    .find(|(_, r)| matches!(r.kind, ManualRollbackKind::Ab))
                else {
                    return Err(TridentError::new(
                        InvalidInputError::InvalidRollbackExpectation {
                            reason:
                                "expected to undo an A/B update but no A/B rollback is available"
                                    .to_string(),
                        },
                    ));
                };
                Ok(Some(available_rollbacks[index].clone()))
            }
        }
    }

    /// Check requested rollback, returning
    ///   * none: if there are no rollbacks available
    ///   * runtime: if runtime is the next available rollback and ab was not requested
    ///   * ab: if ab is the next available and runtime was not requested or if ab was requested and available in the chain
    pub fn check_requested_rollback(
        &self,
        rollback_request_kind: ManualRollbackRequestKind,
    ) -> Result<String, TridentError> {
        let rollback = self.get_requested_rollback(rollback_request_kind)?;
        match rollback {
            None => Ok("none".to_string()),
            Some(item) => Ok(match item.kind {
                ManualRollbackKind::Ab => "ab".to_string(),
                ManualRollbackKind::Runtime => "runtime".to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::TRIDENT_VERSION;
    use maplit::hashmap;
    use sysdefs::tpm2::Pcr;

    use super::*;

    // fn get_requires_reboot(ctx: &ManualRollbackContext) -> bool {
    //     matches!(ctx.rollback_action, Some(ServicingType::AbUpdate))
    // }

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
        let trident_version = match old_version {
            "" => TridentVersion::None,
            v => TridentVersion::SemVer(Version::parse(v).unwrap()),
        };
        HostStatus {
            spec: host_config,
            ab_active_volume: active_volume,
            servicing_state,
            trident_version,
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
        let rollback_chain = context.get_rollback_chain().unwrap();
        assert_eq!(rollback_chain.len(), expected_available_rollbacks.len());
        if !expected_available_rollbacks.is_empty() {
            let next_rollback = rollback_chain.first().unwrap();
            assert_eq!(
                matches!(next_rollback.kind, ManualRollbackKind::Ab),
                expected_requires_reboot
            );
        }
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
    const MIN: &str = MINIMUM_ROLLBACK_TRIDENT_VERSION_STR;
    const NEW: &str = TRIDENT_VERSION;
    const CI_FINAL: ServicingState = ServicingState::CleanInstallFinalized;
    const RU_STAGE: ServicingState = ServicingState::RuntimeUpdateStaged;
    const AB_STAGE: ServicingState = ServicingState::AbUpdateStaged;
    const AB_FINAL: ServicingState = ServicingState::AbUpdateFinalized;
    const AB_HC_FAIL: ServicingState = ServicingState::AbUpdateHealthCheckFailed;
    const MR_AB_STAGE: ServicingState = ServicingState::ManualRollbackAbStaged;
    const MR_RU_STAGE: ServicingState = ServicingState::ManualRollbackRuntimeStaged;
    const MR_AB_FINAL: ServicingState = ServicingState::ManualRollbackAbFinalized;

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
            inter(VOL_A, MR_AB_STAGE, MIN),
            inter(VOL_A, MR_AB_FINAL, MIN),
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
            inter(VOL_A, MR_RU_STAGE, MIN),
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
            inter(VOL_A, MR_AB_STAGE, MIN),
            inter(VOL_A, MR_AB_FINAL, MIN),
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
        rollback_context_testing(&host_status_list, "Offline init and A/B update");
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
        rollback_context_testing(&host_status_list, "Clean install and A/B update");
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
            // Manual Rollback of the available A/B update skips
            // 2 runtime updates
            inter(VOL_B, MR_AB_STAGE, MIN),
            inter(VOL_B, MR_AB_FINAL, MIN),
            prov(VOL_A, false, vec![], MIN),
        ];
        rollback_context_testing(
            &host_status_list,
            "Validate A/B update rollback that skips runtime rollbacks",
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
            "Validate A/B update stage as final state",
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
        rollback_context_testing(&host_status_list, "Validate A/B update health check failed");
    }

    #[test]
    fn test_ab_update_encryption() {
        let host_status_list = vec![
            inter_enc(None, CI_FINAL, MIN),
            inter_enc(None, CI_FINAL, MIN),
            prov_enc(VOL_A, false, vec![], MIN),
            inter_enc(VOL_A, AB_STAGE, MIN),
            inter_enc(VOL_A, AB_FINAL, MIN),
            prov_enc(VOL_B, true, vec![2], MIN), // Now expects rollback available, referring to index 2
        ];
        rollback_context_testing(&host_status_list, "Validate A/B update with encryption");
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
        // if both runtime rollback is requested but no rollbacks are available, error is returned
        assert!(context
            .get_requested_rollback(ManualRollbackRequestKind::RollbackOnlyIfNextIsRuntimeUpdate)
            .unwrap()
            .is_none());
        assert_eq!(
            context
                .check_requested_rollback(
                    ManualRollbackRequestKind::RollbackOnlyIfNextIsRuntimeUpdate
                )
                .unwrap(),
            "none"
        );
        // if ab rollback is requested but no rollbacks are available, error is returned
        assert!(context
            .get_requested_rollback(ManualRollbackRequestKind::RollbackAvailableAbUpdate)
            .unwrap()
            .is_none());
        assert_eq!(
            context
                .check_requested_rollback(ManualRollbackRequestKind::RollbackAvailableAbUpdate)
                .unwrap(),
            "none"
        );

        // Add some operations to datastore
        host_status_list.push(inter(VOL_A, AB_STAGE, MIN));
        host_status_list.push(inter(VOL_A, AB_FINAL, MIN));
        host_status_list.push(prov(VOL_B, true, vec![2], MIN));
        host_status_list.push(inter(VOL_B, RU_STAGE, MIN));
        host_status_list.push(prov(VOL_B, false, vec![5, 2], MIN));
        let context = create_rollback_context_for_testing(&host_status_list);
        // if no specific rollback is requested and the next rollback is runtime, return runtime rollback
        assert!(context
            .get_requested_rollback(ManualRollbackRequestKind::RollbackNext)
            .unwrap()
            .is_some());
        assert_eq!(
            context
                .check_requested_rollback(ManualRollbackRequestKind::RollbackNext)
                .unwrap(),
            "runtime"
        );
        // if ab rollback is requested and it is not the next rollback, return the ab rollback
        assert!(context
            .get_requested_rollback(ManualRollbackRequestKind::RollbackAvailableAbUpdate)
            .unwrap()
            .is_some());
        assert_eq!(
            context
                .check_requested_rollback(ManualRollbackRequestKind::RollbackAvailableAbUpdate)
                .unwrap(),
            "ab"
        );

        // Add an A/B update to database
        host_status_list.push(inter(VOL_B, AB_STAGE, MIN));
        host_status_list.push(inter(VOL_B, AB_FINAL, MIN));
        host_status_list.push(prov(VOL_B, true, vec![2], MIN));
        let context = create_rollback_context_for_testing(&host_status_list);
        // if runtime rollback is requested and it is not the next rollback, return an error
        assert!(context
            .get_requested_rollback(ManualRollbackRequestKind::RollbackOnlyIfNextIsRuntimeUpdate)
            .is_err());
        assert!(context
            .check_requested_rollback(ManualRollbackRequestKind::RollbackOnlyIfNextIsRuntimeUpdate)
            .is_err(),);
    }
}
