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
}

/// OperationKind is classification of Operations based on their servicing state.
/// It is intended to be internal to the ManualRollbackContext construction logic.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
enum OperationKind {
    Unknown,
    Initial,
    AbUpdate,
    RuntimeUpdate,
    AbManualRollback,
    RuntimeManualRollback,
    AbUpdateAutoRollback,
}
impl OperationKind {
    fn keep_parsing(&self) -> bool {
        !matches!(
            self,
            OperationKind::Unknown | OperationKind::Initial | OperationKind::AbUpdateAutoRollback
        )
    }
}

/// Operation is an encapsulation of a set of HostStatus entries for use
/// internally with ManualRollbackContext's parsing logic.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct Operation {
    kind: OperationKind,
    from_host_status: Option<HostStatus>,
    to_host_status: Option<HostStatus>,
}
impl Operation {
    fn keep_parsing(&self) -> bool {
        // Check operation kind
        if !self.kind.keep_parsing() {
            trace!("Operation kind {:?} cannot rollback", self.kind);
            return false;
        }

        // Check HostStatus fields
        if let Some(from_hs) = &self.from_host_status {
            // Check from trident version
            match &from_hs.trident_version {
                // If version is not set or is not semver, consider it incompatible
                TridentVersion::Other(_) | TridentVersion::None => {
                    trace!(
                        "From HostStatus has incompatible Trident version: {:?}, cannot rollback",
                        from_hs.trident_version
                    );
                    return false;
                }
                TridentVersion::SemVer(version) => {
                    if *version < *MINIMUM_ROLLBACK_TRIDENT_VERSION {
                        trace!(
                            "From HostStatus has Trident version below minimum: {:?}, cannot rollback",
                            version
                        );
                        return false;
                    }
                }
            };
        }

        // For all operations, to_host_status must be present
        let to_hs = match &self.to_host_status {
            Some(hs) => hs,
            None => {
                trace!("To HostStatus is missing, cannot rollback");
                return false;
            }
        };

        // Check to trident version
        match &to_hs.trident_version {
            // If version is not set or is not semver, consider it incompatible
            TridentVersion::Other(_) | TridentVersion::None => {
                trace!(
                    "To HostStatus has incompatible Trident version: {:?}, cannot rollback",
                    to_hs.trident_version
                );
                return false;
            }
            TridentVersion::SemVer(version) => {
                if *version < *MINIMUM_ROLLBACK_TRIDENT_VERSION {
                    trace!(
                        "To HostStatus has Trident version below minimum: {:?}, cannot rollback",
                        version
                    );
                    return false;
                }
            }
        };

        // Check to last_error
        if to_hs.last_error.is_some() {
            trace!("To HostStatus has last_error set, cannot rollback");
            return false;
        }

        // Check to encryption configuration for A/B update
        if matches!(self.kind, OperationKind::AbUpdate) && to_hs.spec.storage.encryption.is_some() {
            trace!(
                "To HostStatus has encryption configuration set for A/B update, cannot rollback"
            );
            return false;
        }

        // All checks passed; operation can be rolled back
        true
    }
}

/// ManualRollbackContext tracks available rollbacks and provides methods to query them.
pub(crate) struct ManualRollbackContext {
    /// The chain of available manual rollbacks.
    chain: Vec<ManualRollbackChainItem>,
}
impl ManualRollbackContext {
    /// Creates a new ManualRollbackContext from a list of HostStatus entries.
    /// Starts from the newest HostStatus and works backward to the oldest.
    ///
    /// A set of HostStatus entries are grouped into an Operation based on
    /// the servicing states. A set of Host Status entries is considered
    /// a group if it starts with a Provisioned state and includes all subsequent
    /// staged states until the next Provisioned state (or the end of the list).
    ///
    /// Operations are parsed until either:
    /// * the second A/B update operation is found (indicating that both volumes
    ///   have been updated)
    /// * an operation that cannot be rolled back is found
    /// * a HostStatus entry is None
    ///
    /// Once the Operations have been parsed, the ManualRollback operations (both
    /// A/B and runtime) and the operations that they undo (both A/B and runtime
    /// updates) are pruned from the list of Operations.
    ///
    /// The remaining operations are converted into ManualRollbackChainItems and
    /// stored in the ManualRollbackContext.
    pub fn new(host_statuses: &[Option<HostStatus>]) -> Result<Self, TridentError> {
        let mut operation_list: Vec<Operation> = vec![];

        // Only need to parse until the second A/B volume change
        let mut active_volume_changes = 0;

        // Create staging variable to track the current operation
        let mut current_operation = Operation {
            kind: OperationKind::Unknown,
            from_host_status: None,
            to_host_status: None,
        };

        // Check that the first host status is Provisioned; if not, return empty chain
        let mut first_host_status_is_provisioned = false;
        if let Some(Some(hs)) = host_statuses.first() {
            if matches!(hs.servicing_state, ServicingState::Provisioned) {
                current_operation.to_host_status = Some(hs.clone());
                first_host_status_is_provisioned = true;
            }
        }
        if !first_host_status_is_provisioned {
            trace!("First host status is not Provisioned, returning empty rollback chain");
            return Ok(Self { chain: vec![] });
        }

        let mut rollback_filters: Vec<OperationKind> = vec![];

        // Parse host status groups, where [provisioned & *finalize & *stage] == "group"
        // into operations
        for hs in host_statuses.iter() {
            match hs {
                Some(current_hs) => match current_hs.servicing_state {
                    ServicingState::Provisioned => {
                        if matches!(current_operation.kind, OperationKind::Unknown) {
                            // If operation kind has not been found, this is either a repeated
                            // Provisioned state (e.g., after offline-init) or the top of the
                            // host status list. In either case, do not push the operation.
                        } else {
                            // If operation kind has been found, the operation is complete and
                            // may be added to the operation list.
                            if Self::add_operation_to_list(
                                current_operation.kind.clone(),
                                &mut rollback_filters,
                            )? {
                                if !current_operation.keep_parsing() {
                                    trace!("Operation cannot be part of operation list, ending parsing here.");
                                    break;
                                }
                                current_operation.from_host_status = Some(current_hs.clone());
                                operation_list.push(current_operation.clone());
                            }

                            // Start new operation
                            current_operation = Operation {
                                kind: OperationKind::Unknown,
                                from_host_status: None,
                                to_host_status: None,
                            };
                        }
                    }
                    ServicingState::CleanInstallStaged => {
                        current_operation.kind = OperationKind::Initial;
                        current_operation.from_host_status = None;
                        current_operation.to_host_status = Some(current_hs.clone());
                    }
                    ServicingState::AbUpdateStaged => {
                        current_operation.kind = OperationKind::AbUpdate;
                        current_operation.to_host_status = Some(current_hs.clone());
                        // An A/B update operation represents an active volume
                        // change
                        active_volume_changes += 1;
                        if active_volume_changes >= 2 {
                            trace!("Detected second active volume change, ending parsing here.");
                            break;
                        }
                    }
                    ServicingState::RuntimeUpdateStaged => {
                        current_operation.kind = OperationKind::RuntimeUpdate;
                        current_operation.to_host_status = Some(current_hs.clone());
                    }
                    ServicingState::ManualRollbackAbStaged => {
                        current_operation.kind = OperationKind::AbManualRollback;
                        current_operation.to_host_status = Some(current_hs.clone());
                    }
                    ServicingState::ManualRollbackRuntimeStaged => {
                        current_operation.kind = OperationKind::RuntimeManualRollback;
                        current_operation.to_host_status = Some(current_hs.clone());
                    }
                    ServicingState::AbUpdateHealthCheckFailed => {
                        current_operation.kind = OperationKind::AbUpdateAutoRollback;
                        trace!("Detected AbUpdateAutoRollback operation, ending parsing here.");
                        break;
                    }
                    _ => {
                        // skip
                    }
                },
                None => {
                    trace!("Host status is None, ending parsing here.");
                    break;
                }
            }
        }

        // The remaining operations are the available rollbacks
        Ok(Self {
            chain: operation_list
                .iter()
                .map(|op| {
                    let from_hs = op
                        .clone()
                        .from_host_status
                        .expect("to_host_status must be present for rollbackable operation");
                    ManualRollbackChainItem {
                        spec: from_hs.spec.clone(),
                        ab_active_volume: from_hs.ab_active_volume,
                        install_index: from_hs.install_index,
                        kind: match &op.kind {
                            OperationKind::AbUpdate => ManualRollbackKind::Ab,
                            OperationKind::RuntimeUpdate => ManualRollbackKind::Runtime,
                            kind => panic!(
                                "Unexpected operation kind for rollbackable operation: {:?}",
                                kind
                            ),
                        },
                    }
                })
                .collect(),
        })
    }

    // Helper function to determine if an operation should be added to the operation list.
    // Specifically, this function filters out:
    //  * a RuntimeManualRollback operation and its required subsequent RuntimeUpdate
    //  * an AbManualRollback operation and any subsequent RuntimeUpdates that occur before
    //    its required AbUpdate operation (which will also be filtered out)
    fn add_operation_to_list(
        current_operation_kind: OperationKind,
        rollback_filters: &mut Vec<OperationKind>,
    ) -> Result<bool, TridentError> {
        // For ManualRollback operations, do not add to the operation list, and
        // configure ongoing_rollback_operation_type so that subsesquent Update
        // operations are not added either.
        Ok(match current_operation_kind {
            OperationKind::AbManualRollback => {
                rollback_filters.insert(0, OperationKind::AbManualRollback);
                false
            }
            OperationKind::RuntimeManualRollback => {
                rollback_filters.insert(0, OperationKind::RuntimeManualRollback);
                false
            }
            OperationKind::AbUpdate => {
                match rollback_filters.first() {
                    Some(OperationKind::AbManualRollback) => {
                        // Currently filtering for A/B manual rollback; reset filter and skip this operation
                        rollback_filters.remove(0);
                        false
                    }
                    Some(OperationKind::RuntimeManualRollback) => {
                        return Err(TridentError::new(InvalidInputError::InvalidRollbackExpectation {
                            reason: "Unexpected host_status sequence: A/B update operation found during runtime manual rollback".to_string(),
                        }));
                    }
                    _ => {
                        // Do not filter this operation
                        true
                    }
                }
            }
            OperationKind::RuntimeUpdate => {
                match rollback_filters.first() {
                    Some(OperationKind::AbManualRollback) => {
                        // Currently filtering for A/B manual rollback; skip this operation
                        false
                    }
                    Some(OperationKind::RuntimeManualRollback) => {
                        // Currently filtering for Runtime manual rollback; reset filter and skip this operation
                        rollback_filters.remove(0);
                        false
                    }
                    _ => {
                        // Do not filter this operation
                        true
                    }
                }
            }
            _ => {
                if !rollback_filters.is_empty() {
                    return Err(TridentError::new(InvalidInputError::InvalidRollbackExpectation {
                        reason: "Unexpected host_status sequence: non-update operation found during manual rollback".to_string(),
                    }));
                } else {
                    // Do not filter this operation
                    true
                }
            }
        })
    }

    /// Get the full rollback chain
    pub fn get_rollback_chain(&self) -> Result<Vec<ManualRollbackChainItem>, Error> {
        Ok(self.chain.clone())
    }

    /// Get the full rollback chain as YAML string
    pub fn get_rollback_chain_yaml(&self) -> Result<String, Error> {
        let contexts = self.get_rollback_chain()?;
        let full_yaml =
            serde_yaml::to_string(&contexts).context("Failed to serialize rollback contexts")?;
        info!("Available rollbacks:\n{}", full_yaml);
        Ok(full_yaml)
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
            .map(|hst| Some(hst.host_status.clone()))
            .rev()
            .collect::<Vec<_>>();
        ManualRollbackContext::new(&host_statuses).unwrap()
    }
    fn rollback_context_testing(host_status_test_list: &[HostStatusTest], test_description: &str) {
        let final_state = host_status_test_list.last().unwrap();
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
            prov(VOL_A, false, vec![], OLD),
            inter(VOL_A, RU_STAGE, OLD),
            prov(VOL_A, false, vec![], MIN),
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
