//! Kubernetes label and annotation schema for the AKS ↔ Harpoon protocol.
//!
//! See the design doc's schema (§3), state machine (§5), and failure reasons
//! (§6). Assumption note: duplicate RP retries that reuse the same request-id
//! are treated as idempotent re-affirmations of the current state rather than a
//! restart of the operation.

use serde::{Deserialize, Serialize};

pub const REQUEST_LABEL: &str = "kubernetes.azure.com/trident-abupdate-request";
pub const REQUEST_ID_LABEL: &str = "kubernetes.azure.com/trident-abupdate-request-id";
pub const TARGET_VERSION_LABEL: &str =
    "kubernetes.azure.com/trident-abupdate-target-os-image-version";

pub const STATE_LABEL: &str = "kubernetes.azure.com/trident-abupdate-state";
pub const OBSERVED_REQUEST_ID_LABEL: &str =
    "kubernetes.azure.com/trident-abupdate-observed-request-id";
pub const FAILURE_REASON_LABEL: &str = "kubernetes.azure.com/trident-abupdate-failure-reason";
pub const NODE_IMAGE_VERSION_LABEL: &str = "kubernetes.azure.com/node-image-version";

pub const FAILURE_DETAIL_ANNOTATION: &str = "kubernetes.azure.com/trident-abupdate-failure-detail";

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateRequest {
    Stage,
    Finalize,
    #[default]
    None,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Ready,
    Staging,
    Staged,
    Finalizing,
    Finalized,
    Committing,
    UpdateSucceeded,
    Failed,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum FailureReason {
    DownloadFailed,
    StageFailed,
    VersionMismatch,
    FinalizeFailed,
    CommitFailed,
    VolumeMismatch,
    Timeout,
    RollbackSucceeded,
    RollbackFailed,
    NoUpdateAvailable,
    /// commit() reported NeedsReboot (e.g. a Trident health-check failure).
    /// AKS-RP owns every reboot/rollback decision (accepted-design.md §2.5),
    /// so the agent reports this via labels instead of rebooting itself.
    HealthCheckFailed,
}

impl UpdateRequest {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        match value? {
            "stage" => Some(Self::Stage),
            "finalize" => Some(Self::Finalize),
            "none" => Some(Self::None),
            _ => None,
        }
    }
}

impl State {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        serde_json::from_str::<Self>(&format!("\"{}\"", value?)).ok()
    }
}

impl FailureReason {
    pub fn parse(value: Option<&str>) -> Option<Self> {
        serde_json::from_str::<Self>(&format!("\"{}\"", value?)).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_serde_roundtrip() {
        for state in [
            State::Ready,
            State::Staging,
            State::Staged,
            State::Finalizing,
            State::Finalized,
            State::Committing,
            State::UpdateSucceeded,
            State::Failed,
        ] {
            let encoded = serde_json::to_string(&state).unwrap();
            let decoded: State = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, state);
        }
    }

    #[test]
    fn failure_reason_serde_roundtrip() {
        for reason in [
            FailureReason::DownloadFailed,
            FailureReason::StageFailed,
            FailureReason::VersionMismatch,
            FailureReason::FinalizeFailed,
            FailureReason::CommitFailed,
            FailureReason::VolumeMismatch,
            FailureReason::Timeout,
            FailureReason::RollbackSucceeded,
            FailureReason::RollbackFailed,
            FailureReason::NoUpdateAvailable,
        ] {
            let encoded = serde_json::to_string(&reason).unwrap();
            let decoded: FailureReason = serde_json::from_str(&encoded).unwrap();
            assert_eq!(decoded, reason);
        }
    }
}
