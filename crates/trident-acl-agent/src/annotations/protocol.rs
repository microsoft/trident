//! Request/status annotation protocol types for the Trident ACL agent.
//!
//! This module (schema types, `UpdateRequest::validate()`, and the
//! `#[cfg(test)]` design-doc conformance tests below) implements the
//! `<prefix>/update-request`, `<prefix>/update-status`, and
//! `<prefix>/update-commit-status` node annotation protocol, where
//! `<prefix>` defaults to `acl.microsoft.com` (see
//! [`AnnotationKeys`]/[`crate::core::config::DEFAULT_ANNOTATION_PREFIX`]) and is
//! overridable via the `TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX`
//! environment variable. Keep `UpdateRequest`/`UpdateStatus`/`StatusCode` and
//! `validate()` in sync with that document's formal JSON Schema (its
//! section "Formal JSON Schema") - the
//! `design_doc_*`/`agent_built_*_conform_to_formal_schema` tests in this
//! file's test module pin that JSON Schema in literally and check both the
//! doc's own examples and our constructed annotations against it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use url::Url;
use uuid::Uuid;

use crate::{
    core::{config::DEFAULT_ANNOTATION_PREFIX, error::AgentError, trident::TridentClientError},
    AGENT_VERSION,
};

/// Sentinel `kind` used when Trident reported a remote failure without any
/// structured error at all (see `client.rs`'s `UNKNOWN_REMOTE_ERROR_SUBKIND`
/// for the matching `subkind` sentinel) - keeps `TridentErrorInfo::kind`
/// non-optional instead of adding a third `Option` layer for a case that
/// should already be rare/a Trident-side contract violation.
const UNKNOWN_ERROR_KIND: &str = "unknown";

/// Suffix (appended to the configured annotation prefix) for the request
/// annotation, e.g. `acl.microsoft.com/update-request`.
pub const UPDATE_REQUEST_SUFFIX: &str = "update-request";
/// Suffix for the operation-status annotation, e.g.
/// `acl.microsoft.com/update-status`.
pub const UPDATE_STATUS_SUFFIX: &str = "update-status";
/// Suffix for the post-reboot commit-status annotation, e.g.
/// `acl.microsoft.com/update-commit-status`.
pub const UPDATE_COMMIT_STATUS_SUFFIX: &str = "update-commit-status";

/// The full annotation keys for one deployment's configured annotation
/// prefix. Built once from [`crate::core::config::KubernetesConfig::annotation_prefix`]
/// and threaded through instead of hardcoding a fixed
/// `acl.microsoft.com` prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnotationKeys {
    pub request: String,
    pub status: String,
    pub commit_status: String,
}

impl AnnotationKeys {
    pub fn new(prefix: &str) -> Self {
        Self {
            request: format!("{prefix}/{UPDATE_REQUEST_SUFFIX}"),
            status: format!("{prefix}/{UPDATE_STATUS_SUFFIX}"),
            commit_status: format!("{prefix}/{UPDATE_COMMIT_STATUS_SUFFIX}"),
        }
    }
}

impl Default for AnnotationKeys {
    fn default() -> Self {
        Self::new(DEFAULT_ANNOTATION_PREFIX)
    }
}

pub const SCHEMA_VERSION: &str = "1.0";
const MAX_MESSAGE_BYTES: usize = 2048;
const TRUNCATION_MARKER: &str = "... (truncated)";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum RequestedOperation {
    Stage,
    Finalize,
    Rollback,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    Stage,
    Finalize,
    Rollback,
    Commit,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[serde(rename_all = "PascalCase")]
pub enum StatusCode {
    InProgress,
    Success,
    AlreadyAtTarget,
    NotStaged,
    OperationFailed,
    TargetBootFailed,
    AgentInternalError,
    InvalidRequest,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateRequest {
    pub schema_version: String,
    pub node_update_id: Uuid,
    pub operation_id: String,
    pub operation: RequestedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_version: Option<String>,
    /// The Omaha endpoint that serves the target image, with the path (e.g.
    /// `https://<host>/v1/update`). Required for `stage`/`finalize` (see
    /// [`UpdateRequest::validate`]) - AKS-RP holds this constant across one
    /// update's `stage` -> `finalize` -> `commit` lifecycle, in the same way
    /// it holds `nodeUpdateId` constant, since Nebraska's per-instance state
    /// is tied to one specific server. Omitted for `rollback`, which reports
    /// no Nebraska event. There is deliberately no static/config-file
    /// fallback if this is absent on a `stage`/`finalize` request: a
    /// fallback would let a node update from a source AKS-RP did not
    /// choose, so the agent rejects such a request with `InvalidRequest`
    /// instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<Url>,
    /// The Omaha application id of the ACL image on `server`. Same
    /// requirement/lifecycle/no-fallback rules as
    /// [`server`](UpdateRequest::server) - see its docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    /// The Omaha track that `server` resolves to the group serving this
    /// node. Same requirement/lifecycle/no-fallback rules as
    /// [`server`](UpdateRequest::server) - see its docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateStatus {
    pub schema_version: String,
    pub node_update_id: Uuid,
    pub operation_id: String,
    pub operation: Operation,
    pub code: StatusCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trident_error: Option<TridentErrorInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<String>,
    /// Version of trident-acl-agent that wrote this status (`AGENT_VERSION`).
    /// Packaged and versioned together with tridentd via the same RPM build
    /// (see `packaging/rpm/trident.spec`'s single `TRIDENT_VERSION`-stamped
    /// `%build` step for both `-p trident` and `-p trident-acl-agent`), so
    /// this value doubles as the tridentd version for that install.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trident_version: Option<String>,
    pub started_utc: DateTime<Utc>,
    pub last_updated_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_utc: Option<DateTime<Utc>>,
}

/// A structured Trident error surfaced on a failure status. Mirrors
/// `TridentError`'s `kind`/`subkind`/`location` - its `message`/
/// `error_message` are not duplicated here since `UpdateStatus::message`
/// already carries the human-readable failure text.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TridentErrorInfo {
    /// TridentError kind, e.g. `SERVICING_ERROR`, `HEALTH_CHECKS_ERROR`.
    pub kind: String,
    /// Finer-grained identifier within `kind`, e.g.
    /// `ab-update-reboot-check`.
    pub subkind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<ErrorLocation>,
}

/// Location in Trident's source where a `TridentErrorInfo` originated.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorLocation {
    pub path: String,
    pub line: u32,
}

impl UpdateRequest {
    /// Enforces the same constraints as the request annotation's formal
    /// JSON Schema: schemaVersion match,
    /// targetVersion required for stage/finalize but disallowed for
    /// rollback, and server/appId/track required for stage/finalize. See
    /// this file's module doc.
    pub fn validate(self) -> Result<Self, AgentError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(AgentError::InvalidRequest(format!(
                "unsupported schemaVersion {}",
                self.schema_version
            )));
        }
        if Uuid::parse_str(&self.operation_id).is_err() {
            return Err(AgentError::InvalidRequest(format!(
                "operationId must be a UUID, got {:?}",
                self.operation_id
            )));
        }
        match self.operation {
            RequestedOperation::Stage | RequestedOperation::Finalize => {
                if self.target_version.as_deref().unwrap_or("").is_empty() {
                    return Err(AgentError::InvalidRequest(
                        "targetVersion is required for stage/finalize".to_string(),
                    ));
                }
                if self.server.is_none() {
                    return Err(AgentError::InvalidRequest(
                        "server is required for stage/finalize".to_string(),
                    ));
                }
                if self
                    .server
                    .as_ref()
                    .is_some_and(|u| !matches!(u.scheme(), "http" | "https"))
                {
                    return Err(AgentError::InvalidRequest(
                        "server must be an http(s) URL".to_string(),
                    ));
                }
                if self.app_id.as_deref().unwrap_or("").is_empty() {
                    return Err(AgentError::InvalidRequest(
                        "appId is required for stage/finalize".to_string(),
                    ));
                }
                if self.track.as_deref().unwrap_or("").is_empty() {
                    return Err(AgentError::InvalidRequest(
                        "track is required for stage/finalize".to_string(),
                    ));
                }
            }
            RequestedOperation::Rollback => {
                if self.target_version.is_some() {
                    return Err(AgentError::InvalidRequest(
                        "targetVersion must be omitted for rollback".to_string(),
                    ));
                }
            }
        }
        Ok(self)
    }
}

impl UpdateStatus {
    // This constructor mirrors UpdateStatus's wire schema field-for-field
    // (the two-status-key JSON protocol); splitting
    // it into a builder would add ceremony across ~25 call sites in
    // orchestrator.rs without making any of them clearer.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        request: &UpdateRequest,
        operation: Operation,
        operation_id: String,
        code: StatusCode,
        message: impl Into<String>,
        from_version: Option<String>,
        to_version: Option<String>,
        started_utc: DateTime<Utc>,
        finished_utc: Option<DateTime<Utc>>,
    ) -> Self {
        let finished_or_started = finished_utc.unwrap_or(started_utc);
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: request.node_update_id,
            operation_id,
            operation,
            code,
            message: truncate_message(message.into()),
            trident_error: None,
            from_version,
            to_version,
            trident_version: Some(AGENT_VERSION.to_string()),
            started_utc,
            last_updated_utc: finished_or_started,
            finished_utc,
        }
    }

    /// Refreshes a status immediately before it is written to the node
    /// (the common path for every annotation write - see `publish_status`).
    /// Restamps `trident_version` with the currently-running agent's
    /// `AGENT_VERSION` rather than trusting whatever value the status
    /// already carries: a completed status can be loaded from
    /// `state.json` (written by a possibly older agent) and republished
    /// later by a newer agent (e.g. `recover_from_trident_state` replaying
    /// a cached commit/operation status), so only re-stamping here
    /// guarantees the annotation reflects the agent that actually wrote it.
    pub fn refreshed_for_write(&self) -> Self {
        let mut refreshed = self.clone();
        refreshed.last_updated_utc = Utc::now();
        refreshed.message = truncate_message(refreshed.message);
        refreshed.trident_version = Some(AGENT_VERSION.to_string());
        refreshed
    }

    /// Populates `trident_error` from a Trident-reported remote error, if
    /// present. Non-`Remote` `TridentClientError` variants (connect/timeout/
    /// stream failures) and agent-generated failures (e.g. `InvalidRequest`)
    /// carry no structured Trident error, so this is a no-op for them -
    /// `trident_error` stays unset, since only a subset of failure codes
    /// actually originate from Trident.
    pub fn with_trident_error(mut self, err: &TridentClientError) -> Self {
        if let Some(remote) = err.remote() {
            self.trident_error = Some(TridentErrorInfo {
                kind: remote
                    .kind
                    .map(|kind| kind.as_str_name().to_string())
                    .unwrap_or_else(|| UNKNOWN_ERROR_KIND.to_string()),
                subkind: remote.subkind.clone(),
                location: remote.location.as_ref().map(|location| ErrorLocation {
                    path: location.path.clone(),
                    line: location.line,
                }),
            });
        }
        self
    }

    /// Compares two statuses ignoring `last_updated_utc` and
    /// `trident_version`.
    ///
    /// `publish_status` stamps a fresh `last_updated_utc` on every write via
    /// `refreshed_for_write`, so a straight `PartialEq` between an
    /// already-on-the-node status and a cached/completed one to decide
    /// whether a re-publish is needed would never be equal after the first
    /// publish - triggering another watch event, another "different"
    /// comparison, and another publish, forever. `trident_version` is
    /// excluded for the same reason: `refreshed_for_write` also
    /// unconditionally restamps it with the currently-running agent's
    /// `AGENT_VERSION`, so after an upgrade a cached status carrying the old
    /// version would otherwise never compare equal to the just-republished
    /// one, reintroducing the same infinite re-publish loop. Callers that
    /// only care whether the *content* already matches (and so a re-publish
    /// would be a no-op) must use this instead of `==`/`!=`.
    pub fn same_content(&self, other: &Self) -> bool {
        self.schema_version == other.schema_version
            && self.node_update_id == other.node_update_id
            && self.operation_id == other.operation_id
            && self.operation == other.operation
            && self.code == other.code
            && self.message == other.message
            && self.trident_error == other.trident_error
            && self.from_version == other.from_version
            && self.to_version == other.to_version
            && self.started_utc == other.started_utc
            && self.finished_utc == other.finished_utc
    }
}

fn truncate_message(message: String) -> String {
    if message.len() <= MAX_MESSAGE_BYTES {
        return message;
    }

    let budget = MAX_MESSAGE_BYTES.saturating_sub(TRUNCATION_MARKER.len());
    let mut end = 0;
    for (idx, ch) in message.char_indices() {
        let next = idx + ch.len_utf8();
        if next > budget {
            break;
        }
        end = next;
    }

    let mut truncated = message[..end].to_string();
    truncated.push_str(TRUNCATION_MARKER);
    truncated
}

impl From<RequestedOperation> for Operation {
    fn from(value: RequestedOperation) -> Self {
        match value {
            RequestedOperation::Stage => Operation::Stage,
            RequestedOperation::Finalize => Operation::Finalize,
            RequestedOperation::Rollback => Operation::Rollback,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::TimeZone;
    use serde_json::{Map, Value};

    #[test]
    fn annotation_keys_default_uses_acl_microsoft_com_prefix() {
        let keys = AnnotationKeys::default();
        assert_eq!(keys.request, "acl.microsoft.com/update-request");
        assert_eq!(keys.status, "acl.microsoft.com/update-status");
        assert_eq!(keys.commit_status, "acl.microsoft.com/update-commit-status");
    }

    #[test]
    fn annotation_keys_new_applies_custom_prefix() {
        let keys = AnnotationKeys::new("contoso.example.com");
        assert_eq!(keys.request, "contoso.example.com/update-request");
        assert_eq!(keys.status, "contoso.example.com/update-status");
        assert_eq!(
            keys.commit_status,
            "contoso.example.com/update-commit-status"
        );
    }

    fn sample_request(operation: RequestedOperation) -> UpdateRequest {
        UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            operation_id: "op-1".to_string(),
            operation,
            target_version: Some("2.0.0".to_string()),
            server: None,
            app_id: None,
            track: None,
        }
    }

    fn fixed_time(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    #[test]
    fn same_content_ignores_last_updated_utc() {
        // Regression test: publish_status() -> refreshed_for_write() stamps a
        // fresh last_updated_utc on every write. If the "already completed"
        // dedupe check in orchestrator.rs compared statuses with `==`/`!=`
        // instead of `same_content`, a cached status would never equal the
        // freshly-published one (their last_updated_utc always differs),
        // causing an infinite republish loop on every watch event.
        let request = sample_request(RequestedOperation::Finalize);
        let original = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::Success,
            "finalize completed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );
        let republished = original.refreshed_for_write();

        assert_ne!(
            original.last_updated_utc, republished.last_updated_utc,
            "refreshed_for_write should always stamp a new timestamp"
        );
        assert_ne!(
            original, republished,
            "PartialEq must still distinguish them (guards against same_content silently replacing derived Eq)"
        );
        assert!(
            original.same_content(&republished),
            "same_content must ignore last_updated_utc"
        );
    }

    #[test]
    fn refreshed_for_write_restamps_trident_version() {
        // Regression test: a completed status can be loaded from
        // state.json (written by a possibly older agent) and republished
        // later by a newer agent (recover_from_trident_state replaying a
        // cached commit/operation status via publish_status). Verify
        // refreshed_for_write - the common path for every annotation write
        // - always overwrites trident_version with the currently-running
        // AGENT_VERSION, rather than preserving whatever an older/stale
        // status carries.
        let request = sample_request(RequestedOperation::Finalize);
        let mut stale = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::Success,
            "finalize completed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );
        stale.trident_version = Some("0.0.1-old-agent".to_string());

        let republished = stale.refreshed_for_write();

        assert_eq!(
            republished.trident_version.as_deref(),
            Some(AGENT_VERSION),
            "refreshed_for_write must restamp trident_version with the current agent's AGENT_VERSION"
        );
    }

    #[test]
    fn same_content_detects_real_differences() {
        let request = sample_request(RequestedOperation::Finalize);
        let success = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::Success,
            "finalize completed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );
        let failed = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::OperationFailed,
            "finalize failed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );

        assert!(!success.same_content(&failed));
    }

    #[test]
    fn same_content_ignores_trident_version() {
        // Regression test: refreshed_for_write() always restamps
        // trident_version with the currently-running agent's AGENT_VERSION
        // (see refreshed_for_write_restamps_trident_version). If
        // same_content compared trident_version, a cached status from
        // before an agent upgrade would never again compare equal to the
        // freshly-republished one, reintroducing the infinite re-publish
        // loop same_content exists to prevent.
        let request = sample_request(RequestedOperation::Finalize);
        let mut cached = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::Success,
            "finalize completed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );
        cached.trident_version = Some("0.0.1-old-agent".to_string());
        let republished = cached.refreshed_for_write();

        assert_ne!(cached.trident_version, republished.trident_version);
        assert!(cached.same_content(&republished));
    }

    #[test]
    fn truncates_messages_longer_than_2048_bytes() {
        let request = sample_request(RequestedOperation::Stage);
        let status = UpdateStatus::new(
            &request,
            Operation::Stage,
            request.operation_id.clone(),
            StatusCode::OperationFailed,
            "x".repeat(3000),
            None,
            None,
            fixed_time(0),
            Some(fixed_time(1)),
        );

        assert!(status.message.len() <= MAX_MESSAGE_BYTES);
        assert!(status.message.ends_with(TRUNCATION_MARKER));
    }

    /// Round-trips `status` through JSON and returns the parsed `Value`, also
    /// asserting the annotation is valid JSON and that deserializing it back
    /// produces an identical `UpdateStatus` (guards against any field being
    /// silently dropped or renamed by a future schema change).
    fn to_annotation_json(status: &UpdateStatus) -> Value {
        let text = serde_json::to_string(status).expect("UpdateStatus must serialize to JSON");
        let value: Value = serde_json::from_str(&text).expect("annotation must be valid JSON");
        let round_tripped: UpdateStatus =
            serde_json::from_str(&text).expect("annotation must deserialize back to UpdateStatus");
        assert_eq!(&round_tripped, status);
        value
    }

    #[test]
    fn stage_success_annotation_has_expected_shape() {
        let request = sample_request(RequestedOperation::Stage);
        let status = UpdateStatus::new(
            &request,
            Operation::Stage,
            request.operation_id.clone(),
            StatusCode::Success,
            "stage completed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );

        let json = to_annotation_json(&status);
        assert_eq!(json["schemaVersion"], "1.0");
        assert_eq!(json["operationId"], "op-1");
        assert_eq!(json["operation"], "stage");
        assert_eq!(json["code"], "Success");
        assert_eq!(json["message"], "stage completed");
        assert_eq!(json["fromVersion"], "1.0.0");
        assert_eq!(json["toVersion"], "2.0.0");
        assert_eq!(json["tridentVersion"], AGENT_VERSION);
        assert!(json.get("startedUtc").is_some());
        assert!(json.get("lastUpdatedUtc").is_some());
        assert!(json.get("finishedUtc").is_some());
        // Confirms camelCase renaming applies to every field, not just a subset.
        assert!(json.get("nodeUpdateId").is_some());
    }

    #[test]
    fn new_always_populates_trident_version_with_agent_version() {
        let request = sample_request(RequestedOperation::Stage);
        let status = UpdateStatus::new(
            &request,
            Operation::Stage,
            request.operation_id.clone(),
            StatusCode::Success,
            "stage completed",
            None,
            None,
            fixed_time(0),
            Some(fixed_time(5)),
        );

        assert_eq!(status.trident_version.as_deref(), Some(AGENT_VERSION));
    }

    #[test]
    fn stage_failure_annotation_has_operation_failed_code() {
        let request = sample_request(RequestedOperation::Stage);
        let status = UpdateStatus::new(
            &request,
            Operation::Stage,
            request.operation_id.clone(),
            StatusCode::OperationFailed,
            "stage failed: disk full",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );

        let json = to_annotation_json(&status);
        assert_eq!(json["code"], "OperationFailed");
        assert_eq!(json["message"], "stage failed: disk full");
    }

    #[test]
    fn finalize_success_annotation_records_operation_and_no_finish_before_reboot() {
        let request = sample_request(RequestedOperation::Finalize);
        // In-progress finalize status published before reboot has no
        // finishedUtc yet - confirm the annotation omits the field entirely
        // (skip_serializing_if) rather than emitting `null`.
        let in_progress = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "finalizing update",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            None,
        );

        let json = to_annotation_json(&in_progress);
        assert_eq!(json["operation"], "finalize");
        assert_eq!(json["code"], "InProgress");
        assert!(
            json.get("finishedUtc").is_none(),
            "finishedUtc should be omitted, not null, while in progress"
        );
    }

    #[test]
    fn finalize_failure_reverted_annotation_has_target_boot_failed_code() {
        let request = sample_request(RequestedOperation::Finalize);
        let status = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::TargetBootFailed,
            "finalize failed: trident reported ab-update-reboot-check failure",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );

        let json = to_annotation_json(&status);
        assert_eq!(json["code"], "TargetBootFailed");
        assert_eq!(json["operationId"], "op-1");
    }

    #[test]
    fn commit_success_annotation_reuses_operation_id() {
        let request = sample_request(RequestedOperation::Finalize);
        let commit_id = request.operation_id.clone();
        let status = UpdateStatus::new(
            &request,
            Operation::Commit,
            commit_id.clone(),
            StatusCode::Success,
            "commit completed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );

        let json = to_annotation_json(&status);
        assert_eq!(json["operationId"], "op-1");
        assert_eq!(json["operation"], "commit");
        assert_eq!(json["code"], "Success");
    }

    #[test]
    fn commit_reboot_required_annotation_uses_agent_internal_error_code() {
        let request = sample_request(RequestedOperation::Finalize);
        let commit_id = request.operation_id.clone();
        let status = UpdateStatus::new(
            &request,
            Operation::Commit,
            commit_id,
            StatusCode::AgentInternalError,
            "commit requested another reboot",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );

        let json = to_annotation_json(&status);
        assert_eq!(json["code"], "AgentInternalError");
        assert!(json["message"].as_str().unwrap().contains("another reboot"));
    }

    #[test]
    fn commit_failure_reverted_annotations_cover_both_reverted_subkinds() {
        let request = sample_request(RequestedOperation::Finalize);
        let commit_id = request.operation_id.clone();

        for message in [
            "commit failed: trident reported ab-update-reboot-check failure",
            "commit failed: trident reported ab-update-health-check-commit-check failure",
        ] {
            let status = UpdateStatus::new(
                &request,
                Operation::Commit,
                commit_id.clone(),
                StatusCode::TargetBootFailed,
                message,
                Some("1.0.0".to_string()),
                Some("2.0.0".to_string()),
                fixed_time(0),
                Some(fixed_time(5)),
            );

            let json = to_annotation_json(&status);
            assert_eq!(json["code"], "TargetBootFailed");
            assert_eq!(json["operationId"], "op-1");
        }
    }

    #[test]
    fn commit_failure_generic_annotation_has_operation_failed_code() {
        let request = sample_request(RequestedOperation::Finalize);
        let commit_id = request.operation_id.clone();
        let status = UpdateStatus::new(
            &request,
            Operation::Commit,
            commit_id,
            StatusCode::OperationFailed,
            "commit failed: commit rpc failed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );

        let json = to_annotation_json(&status);
        assert_eq!(json["code"], "OperationFailed");
        assert_eq!(json["operationId"], "op-1");
    }

    #[test]
    fn optional_version_fields_are_omitted_not_null_when_absent() {
        let request = sample_request(RequestedOperation::Rollback);
        let status = UpdateStatus::new(
            &request,
            Operation::Rollback,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "staging rollback",
            Some("1.0.0".to_string()),
            None,
            fixed_time(0),
            None,
        );

        let json = to_annotation_json(&status);
        assert!(json.get("toVersion").is_none());
        assert!(json.get("lastUpdatedUtc").is_some());
        assert!(json.get("finishedUtc").is_none());
        assert_eq!(json["fromVersion"], "1.0.0");
    }

    // Pins our annotation (de)serialization/validation code against two
    // things lifted verbatim from the design doc, so a
    // doc/code drift shows up as a
    // test failure instead of being discovered against a real AKS-RP:
    //   1. The three example JSON payloads (request, finalize status, and
    //      the derived commit status) parse with our real UpdateRequest /
    //      UpdateStatus (de)serialization and UpdateRequest::validate().
    //   2. Annotations our own code constructs conform to the two formal
    //      JSON Schema documents embedded in the same section.
    //
    // Keep these constants byte-for-byte in sync with the design doc.

    /// The design doc's "Request annotation" example
    /// (adapted to `finalize` to pair with the status/commit examples
    /// below, which also share this `finalize`; server/appId/track values
    /// are the doc's own example values for those fields, required on
    /// stage/finalize).
    const DESIGN_DOC_FINALIZE_REQUEST_EXAMPLE: &str = r#"{
  "schemaVersion": "1.0",
  "nodeUpdateId":  "550e8400-e29b-41d4-a716-446655440000",
  "operationId":   "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "operation":     "finalize",
  "targetVersion": "202606.29.0",
  "server":        "https://nebraska.example.com/v1/update",
  "appId":         "11111111-2222-3333-4444-555555555555",
  "track":         "pin-202606.29.0"
}"#;

    /// The design doc's "Status annotation" example.
    const DESIGN_DOC_FINALIZE_STATUS_EXAMPLE: &str = r#"{
  "schemaVersion": "1.0",
  "nodeUpdateId":  "550e8400-e29b-41d4-a716-446655440000",
  "operationId":   "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "operation":     "finalize",
  "code":          "Success",
  "message":       "boot armed, rebooting, awaiting commit",
  "fromVersion":   "202605.15.0",
  "toVersion":     "202606.29.0",
  "startedUtc":    "2026-06-04T12:00:00Z",
  "lastUpdatedUtc": "2026-06-04T12:00:32Z",
  "finishedUtc":   "2026-06-04T12:00:32Z"
}"#;

    /// The design doc's derived post-reboot commit status example.
    const DESIGN_DOC_COMMIT_STATUS_EXAMPLE: &str = r#"{
  "schemaVersion": "1.0",
  "nodeUpdateId":  "550e8400-e29b-41d4-a716-446655440000",
  "operationId":   "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "operation":     "commit",
  "code":          "Success",
  "message":       "booted expected volume, boot order promoted",
  "fromVersion":   "202605.15.0",
  "toVersion":     "202606.29.0",
  "startedUtc":    "2026-06-04T12:01:18Z",
  "lastUpdatedUtc": "2026-06-04T12:01:32Z",
  "finishedUtc":   "2026-06-04T12:01:32Z"
}"#;

    /// The formal JSON Schema for the request annotation. Keep
    /// byte-for-byte in sync with the design doc.
    const DESIGN_DOC_REQUEST_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://acl.azure.com/schemas/update-request/1.0.json",
  "title": "ACL A/B update request annotation",
  "type": "object",
  "additionalProperties": false,
  "required": ["schemaVersion", "nodeUpdateId", "operationId", "operation"],
  "properties": {
    "schemaVersion": { "type": "string", "const": "1.0" },
    "nodeUpdateId":  { "type": "string", "format": "uuid", "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$" },
    "operationId":   { "type": "string", "format": "uuid", "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$" },
    "operation":     { "type": "string", "enum": ["stage", "finalize", "rollback"] },
    "targetVersion": { "type": "string", "description": "ACL image release version, e.g. 202606.29.0." },
    "server":        { "type": "string", "format": "uri", "pattern": "^https://[^/]", "description": "Omaha endpoint that serves the target image, with the path, e.g. https://<host>/v1/update." },
    "appId":         { "type": "string", "description": "Omaha application id of the ACL image on that endpoint." },
    "track":         { "type": "string", "description": "Omaha track that the update server resolves to the group serving the node." }
  },
  "allOf": [
    {
      "if":   { "properties": { "operation": { "enum": ["stage", "finalize"] } }, "required": ["operation"] },
      "then": { "required": ["targetVersion"] }
    },
    {
      "if":   { "properties": { "operation": { "enum": ["stage", "finalize"] } }, "required": ["operation"] },
      "then": { "required": ["server", "appId", "track"] }
    }
  ]
}"#;

    /// The formal JSON Schema for the status annotations. Keep
    /// byte-for-byte in sync with the design doc.
    const DESIGN_DOC_STATUS_SCHEMA: &str = r#"{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://acl.azure.com/schemas/update-status/1.0.json",
  "title": "ACL A/B update status annotation",
  "type": "object",
  "additionalProperties": false,
  "required": ["schemaVersion", "nodeUpdateId", "operationId", "operation", "code"],
  "properties": {
    "schemaVersion": { "type": "string", "const": "1.0" },
    "nodeUpdateId":  { "type": "string", "format": "uuid", "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$" },
    "operationId":   { "type": "string", "format": "uuid", "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$", "description": "The operationId of the request this status reports on. The post-reboot commit status repeats the operationId of the finalize or rollback that caused the reboot." },
    "operation":     { "type": "string", "enum": ["stage", "finalize", "rollback", "commit"] },
    "code":          { "type": "string", "enum": ["InProgress", "Success", "AlreadyAtTarget", "NotStaged", "OperationFailed", "TargetBootFailed", "AgentInternalError", "InvalidRequest"] },
    "message":       { "type": "string", "maxLength": 2048 },
    "tridentError": {
      "type": "object",
      "additionalProperties": false,
      "required": ["kind", "subkind"],
      "description": "Structured Trident error, present only when the failure originated from a Trident remote error - a subset of failure codes carry this; most do not.",
      "properties": {
        "kind":    { "type": "string", "description": "TridentError kind, e.g. SERVICING_ERROR, HEALTH_CHECKS_ERROR." },
        "subkind": { "type": "string", "description": "Finer-grained identifier within kind, e.g. ab-update-reboot-check." },
        "location": {
          "type": "object",
          "additionalProperties": false,
          "required": ["path", "line"],
          "description": "Location in Trident's source where the error originated.",
          "properties": {
            "path": { "type": "string" },
            "line": { "type": "integer", "minimum": 0, "maximum": 4294967295 }
          }
        }
      }
    },
    "fromVersion":   { "type": "string" },
    "toVersion":     { "type": "string" },
    "tridentVersion": { "type": "string", "description": "Version of trident-acl-agent that wrote this status. Packaged and released together with tridentd, so this is also the tridentd version." },
    "startedUtc":    { "type": "string", "format": "date-time" },
    "lastUpdatedUtc": { "type": "string", "format": "date-time", "description": "When the agent last wrote this status. The agent refreshes it on every write, including a periodic InProgress heartbeat, so AKS-RP and the watchdog can tell a working agent from a stuck one." },
    "finishedUtc":   { "type": "string", "format": "date-time" }
  },
  "allOf": [
    {
      "if":   { "properties": { "code": { "const": "InProgress" } }, "required": ["code"] },
      "then": { "required": ["startedUtc", "lastUpdatedUtc"] },
      "else": { "required": ["startedUtc", "finishedUtc"] }
    }
  ]
}"#;

    // --- minimal JSON Schema subset validator ------------------------------
    //
    // Deliberately not a general-purpose JSON Schema engine: supports only
    // the exact vocabulary the two schemas above actually use (type,
    // additionalProperties, required, properties.{type,const,enum,format,
    // pattern}, and a single-level allOf/if/then/else). Panics loudly on any
    // schema keyword/pattern/type/format it doesn't recognize, so if
    // the schemas grow new constraints, this validator's
    // blind spots don't silently mask them - the test fails instead,
    // prompting an update here.

    fn schema_validate(schema: &Value, instance: &Value) -> Result<(), String> {
        let schema_obj = schema.as_object().ok_or("schema is not a JSON object")?;
        let obj = instance
            .as_object()
            .ok_or("instance is not a JSON object")?;

        let properties = schema_obj.get("properties").and_then(Value::as_object);

        if schema_obj
            .get("additionalProperties")
            .and_then(Value::as_bool)
            == Some(false)
        {
            if let Some(props) = properties {
                for key in obj.keys() {
                    if !props.contains_key(key) {
                        return Err(format!(
                            "property {key:?} not declared in schema (additionalProperties: false)"
                        ));
                    }
                }
            }
        }

        if let Some(required) = schema_obj.get("required").and_then(Value::as_array) {
            for req in required {
                let name = req.as_str().ok_or("required entry is not a string")?;
                if !obj.contains_key(name) {
                    return Err(format!("missing required property {name:?}"));
                }
            }
        }

        if let Some(props) = properties {
            for (name, prop_schema) in props {
                if let Some(value) = obj.get(name) {
                    schema_validate_property(name, prop_schema, value)?;
                }
            }
        }

        if let Some(all_of) = schema_obj.get("allOf").and_then(Value::as_array) {
            for clause in all_of {
                let clause_obj = clause.as_object().ok_or("allOf entry is not an object")?;
                let condition_met = match clause_obj.get("if") {
                    Some(if_schema) => schema_if_matches(if_schema, obj),
                    None => true,
                };
                let branch = if condition_met {
                    clause_obj.get("then")
                } else {
                    clause_obj.get("else")
                };
                if let Some(branch) = branch {
                    schema_validate(branch, instance)?;
                }
            }
        }

        Ok(())
    }

    fn schema_if_matches(if_schema: &Value, obj: &Map<String, Value>) -> bool {
        let Some(if_obj) = if_schema.as_object() else {
            return false;
        };
        if let Some(required) = if_obj.get("required").and_then(Value::as_array) {
            for req in required {
                let Some(name) = req.as_str() else {
                    return false;
                };
                if !obj.contains_key(name) {
                    return false;
                }
            }
        }
        if let Some(props) = if_obj.get("properties").and_then(Value::as_object) {
            for (name, prop_schema) in props {
                let Some(value) = obj.get(name) else {
                    return false;
                };
                if let Some(enum_values) = prop_schema.get("enum").and_then(Value::as_array) {
                    if !enum_values.iter().any(|v| v == value) {
                        return false;
                    }
                }
                if let Some(const_value) = prop_schema.get("const") {
                    if value != const_value {
                        return false;
                    }
                }
            }
        }
        true
    }

    fn schema_validate_property(
        name: &str,
        prop_schema: &Value,
        value: &Value,
    ) -> Result<(), String> {
        if let Some(expected_type) = prop_schema.get("type").and_then(Value::as_str) {
            let matches = match expected_type {
                "string" => value.is_string(),
                "object" => value.is_object(),
                "array" => value.is_array(),
                "boolean" => value.is_boolean(),
                "number" => value.is_number(),
                "integer" => value.is_i64() || value.is_u64(),
                other => panic!(
                    "test schema validator does not support type {other:?} - extend schema_validate_property"
                ),
            };
            if !matches {
                return Err(format!(
                    "property {name:?}: expected type {expected_type}, got {value:?}"
                ));
            }
            // Recurse into nested object schemas (e.g. `tridentError`/`location`)
            // so their own properties/required/additionalProperties are
            // checked too, not just "is this an object".
            if expected_type == "object" {
                schema_validate(prop_schema, value)
                    .map_err(|err| format!("property {name:?}: {err}"))?;
            }
        }
        if let Some(const_value) = prop_schema.get("const") {
            if value != const_value {
                return Err(format!(
                    "property {name:?}: expected const {const_value:?}, got {value:?}"
                ));
            }
        }
        if let Some(enum_values) = prop_schema.get("enum").and_then(Value::as_array) {
            if !enum_values.iter().any(|v| v == value) {
                return Err(format!(
                    "property {name:?}: value {value:?} not in enum {enum_values:?}"
                ));
            }
        }
        if let Some(format) = prop_schema.get("format").and_then(Value::as_str) {
            let s = value.as_str().ok_or_else(|| {
                format!("property {name:?}: expected string for format {format:?}")
            })?;
            match format {
                "uuid" => {
                    Uuid::parse_str(s).map_err(|err| {
                        format!("property {name:?}: {s:?} is not a valid uuid: {err}")
                    })?;
                }
                "date-time" => {
                    DateTime::parse_from_rfc3339(s).map_err(|err| {
                        format!("property {name:?}: {s:?} is not a valid date-time: {err}")
                    })?;
                }
                "uri" => {
                    Url::parse(s).map_err(|err| {
                        format!("property {name:?}: {s:?} is not a valid uri: {err}")
                    })?;
                }
                other => panic!(
                    "test schema validator does not support format {other:?} - extend schema_validate_property"
                ),
            }
        }
        if let Some(pattern) = prop_schema.get("pattern").and_then(Value::as_str) {
            let s = value
                .as_str()
                .ok_or_else(|| format!("property {name:?}: expected string to check pattern"))?;
            if !schema_pattern_matches(pattern, s) {
                return Err(format!(
                    "property {name:?}: {s:?} does not match pattern {pattern:?}"
                ));
            }
        }
        if let Some(minimum) = prop_schema.get("minimum").and_then(Value::as_f64) {
            let n = value
                .as_f64()
                .ok_or_else(|| format!("property {name:?}: expected number to check minimum"))?;
            if n < minimum {
                return Err(format!("property {name:?}: {n} is below minimum {minimum}"));
            }
        }
        if let Some(maximum) = prop_schema.get("maximum").and_then(Value::as_f64) {
            let n = value
                .as_f64()
                .ok_or_else(|| format!("property {name:?}: expected number to check maximum"))?;
            if n > maximum {
                return Err(format!("property {name:?}: {n} exceeds maximum {maximum}"));
            }
        }
        if let Some(max_length) = prop_schema.get("maxLength").and_then(Value::as_u64) {
            let s = value
                .as_str()
                .ok_or_else(|| format!("property {name:?}: expected string to check maxLength"))?;
            // Matches truncate_message's byte-based budget (MAX_MESSAGE_BYTES)
            // rather than a char count, since that's the unit actually
            // enforced in production.
            if s.len() as u64 > max_length {
                return Err(format!(
                    "property {name:?}: {s:?} exceeds maxLength {max_length}"
                ));
            }
        }
        Ok(())
    }

    /// Bespoke stand-in for full regex support: the two schemas above use
    /// only a few distinct patterns - two UUID-shaped ones and the
    /// `server` https-with-host one - so this matches them by exact pattern
    /// text rather than pulling in a regex engine for such a small,
    /// enumerable set. Panics on an unrecognized pattern so a future schema
    /// change can't silently pass unchecked.
    fn schema_pattern_matches(pattern: &str, value: &str) -> bool {
        const BARE_UUID: &str =
            r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";
        const HTTPS_WITH_HOST: &str = r"^https://[^/]";
        match pattern {
            BARE_UUID => Uuid::parse_str(value).is_ok(),
            HTTPS_WITH_HOST => value
                .strip_prefix("https://")
                .and_then(|rest| rest.chars().next())
                .is_some_and(|c| c != '/'),
            other => panic!(
                "test schema validator does not recognize pattern {other:?} - extend schema_pattern_matches"
            ),
        }
    }

    // --- UpdateRequest::validate() ------------------------------------------

    fn valid_nebraska_request(operation: RequestedOperation) -> UpdateRequest {
        UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4().to_string(),
            operation,
            target_version: Some("202606.29.0".to_string()),
            server: Some(Url::parse("https://nebraska.example/v1/update").unwrap()),
            app_id: Some("app-id".to_string()),
            track: Some("track".to_string()),
        }
    }

    #[test]
    fn validate_accepts_stage_and_finalize_with_nebraska_fields() {
        for operation in [RequestedOperation::Stage, RequestedOperation::Finalize] {
            valid_nebraska_request(operation)
                .validate()
                .unwrap_or_else(|err| panic!("{operation:?} with server/appId/track: {err}"));
        }
    }

    #[test]
    fn validate_rejects_stage_and_finalize_missing_server() {
        for operation in [RequestedOperation::Stage, RequestedOperation::Finalize] {
            let mut request = valid_nebraska_request(operation);
            request.server = None;
            let err = request
                .validate()
                .expect_err("missing server must be rejected for stage/finalize");
            assert!(err.to_string().contains("server"), "{err}");
        }
    }

    #[test]
    fn validate_accepts_plain_http_server() {
        // Deliberately not https-only: test/dev harnesses (e.g. the storm
        // E2E suite) point `server` at a local, unencrypted Nebraska stub.
        for operation in [RequestedOperation::Stage, RequestedOperation::Finalize] {
            let mut request = valid_nebraska_request(operation);
            request.server = Some(Url::parse("http://127.0.0.1:8080/v1/update").unwrap());
            request
                .validate()
                .unwrap_or_else(|err| panic!("{operation:?} with http:// server: {err}"));
        }
    }

    #[test]
    fn validate_rejects_stage_and_finalize_non_http_scheme_server() {
        for operation in [RequestedOperation::Stage, RequestedOperation::Finalize] {
            let mut request = valid_nebraska_request(operation);
            request.server = Some(Url::parse("ftp://nebraska.example/v1/update").unwrap());
            let err = request
                .validate()
                .expect_err("a non-http(s) server scheme must be rejected");
            assert!(err.to_string().contains("server"), "{err}");
        }
    }

    #[test]
    fn validate_rejects_stage_and_finalize_missing_app_id() {
        for operation in [RequestedOperation::Stage, RequestedOperation::Finalize] {
            let mut request = valid_nebraska_request(operation);
            request.app_id = None;
            let err = request
                .validate()
                .expect_err("missing appId must be rejected for stage/finalize");
            assert!(err.to_string().contains("appId"), "{err}");
        }
    }

    #[test]
    fn validate_rejects_stage_and_finalize_missing_track() {
        for operation in [RequestedOperation::Stage, RequestedOperation::Finalize] {
            let mut request = valid_nebraska_request(operation);
            request.track = None;
            let err = request
                .validate()
                .expect_err("missing track must be rejected for stage/finalize");
            assert!(err.to_string().contains("track"), "{err}");
        }
    }

    #[test]
    fn validate_allows_rollback_without_nebraska_fields() {
        // Rollback reports no Nebraska event, so it carries no update
        // source: server/appId/track are not
        // required, and validate() must not reject their absence.
        let request = UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4().to_string(),
            operation: RequestedOperation::Rollback,
            target_version: None,
            server: None,
            app_id: None,
            track: None,
        };
        request
            .validate()
            .expect("rollback without server/appId/track must validate");
    }

    #[test]
    fn validate_rejects_non_uuid_operation_id() {
        // The formal schema requires `operationId` to be UUID-shaped
        // (`format: uuid`); a non-UUID value must be rejected regardless of
        // operation kind.
        for operation in [
            RequestedOperation::Stage,
            RequestedOperation::Finalize,
            RequestedOperation::Rollback,
        ] {
            let mut request = valid_nebraska_request(operation);
            request.operation_id = "op-1".to_string();
            let err = request
                .validate()
                .expect_err("a non-UUID operationId must be rejected");
            assert!(err.to_string().contains("operationId"), "{err}");
        }
    }

    // --- example payload parsing tests -------------------------------------

    #[test]
    fn design_doc_finalize_request_example_parses_and_validates() {
        let request: UpdateRequest = serde_json::from_str(DESIGN_DOC_FINALIZE_REQUEST_EXAMPLE)
            .expect("design doc's finalize request example must parse as UpdateRequest");
        let request = request
            .validate()
            .expect("design doc's finalize request example must pass UpdateRequest::validate()");
        assert_eq!(request.schema_version, "1.0");
        assert_eq!(
            request.node_update_id,
            Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
        );
        assert_eq!(request.operation_id, "f47ac10b-58cc-4372-a567-0e02b2c3d479");
        assert_eq!(request.operation, RequestedOperation::Finalize);
        assert_eq!(request.target_version.as_deref(), Some("202606.29.0"));
    }

    #[test]
    fn design_doc_finalize_status_example_parses() {
        let status: UpdateStatus = serde_json::from_str(DESIGN_DOC_FINALIZE_STATUS_EXAMPLE)
            .expect("design doc's finalize status example must parse as UpdateStatus");
        assert_eq!(status.operation, Operation::Finalize);
        assert_eq!(status.code, StatusCode::Success);
        assert_eq!(status.from_version.as_deref(), Some("202605.15.0"));
        assert_eq!(status.to_version.as_deref(), Some("202606.29.0"));
        assert!(status.finished_utc.is_some());
    }

    #[test]
    fn design_doc_commit_status_example_parses() {
        let status: UpdateStatus = serde_json::from_str(DESIGN_DOC_COMMIT_STATUS_EXAMPLE)
            .expect("design doc's commit status example must parse as UpdateStatus");
        assert_eq!(status.operation, Operation::Commit);
        assert_eq!(status.code, StatusCode::Success);
        assert_eq!(status.operation_id, "f47ac10b-58cc-4372-a567-0e02b2c3d479");
    }

    // --- example payloads validated against the embedded formal schema ----

    #[test]
    fn design_doc_finalize_request_example_matches_formal_schema() {
        let schema: Value = serde_json::from_str(DESIGN_DOC_REQUEST_SCHEMA).unwrap();
        let instance: Value = serde_json::from_str(DESIGN_DOC_FINALIZE_REQUEST_EXAMPLE).unwrap();
        schema_validate(&schema, &instance)
            .expect("design doc's own finalize request example must satisfy its own schema");
    }

    #[test]
    fn design_doc_finalize_status_example_matches_formal_schema() {
        let schema: Value = serde_json::from_str(DESIGN_DOC_STATUS_SCHEMA).unwrap();
        let instance: Value = serde_json::from_str(DESIGN_DOC_FINALIZE_STATUS_EXAMPLE).unwrap();
        schema_validate(&schema, &instance)
            .expect("design doc's own finalize status example must satisfy its own schema");
    }

    #[test]
    fn design_doc_commit_status_example_matches_formal_schema() {
        let schema: Value = serde_json::from_str(DESIGN_DOC_STATUS_SCHEMA).unwrap();
        let instance: Value = serde_json::from_str(DESIGN_DOC_COMMIT_STATUS_EXAMPLE).unwrap();
        schema_validate(&schema, &instance)
            .expect("design doc's own commit status example must satisfy its own schema");
    }

    // --- annotations *we construct* validated against the embedded schema -

    #[test]
    fn agent_built_requests_conform_to_formal_schema() {
        let schema: Value = serde_json::from_str(DESIGN_DOC_REQUEST_SCHEMA).unwrap();
        let node_update_id = Uuid::new_v4();

        for (operation, target_version, nebraska) in [
            (
                RequestedOperation::Stage,
                Some("202606.29.0".to_string()),
                Some((
                    "https://nebraska.example.com/v1/update",
                    "11111111-2222-3333-4444-555555555555",
                    "pin-202606.29.0",
                )),
            ),
            (
                RequestedOperation::Finalize,
                Some("202606.29.0".to_string()),
                Some((
                    "https://nebraska.example.com/v1/update",
                    "11111111-2222-3333-4444-555555555555",
                    "pin-202606.29.0",
                )),
            ),
            (RequestedOperation::Rollback, None, None),
        ] {
            let request = UpdateRequest {
                schema_version: SCHEMA_VERSION.to_string(),
                node_update_id,
                operation_id: Uuid::new_v4().to_string(),
                operation,
                target_version,
                server: nebraska.map(|(server, ..)| Url::parse(server).unwrap()),
                app_id: nebraska.map(|(_, app_id, _)| app_id.to_string()),
                track: nebraska.map(|(_, _, track)| track.to_string()),
            };
            let request = request
                .validate()
                .unwrap_or_else(|err| panic!("{operation:?} request must validate: {err}"));
            let instance: Value = serde_json::to_value(&request).unwrap();
            schema_validate(&schema, &instance).unwrap_or_else(|err| {
                panic!(
                    "agent-constructed {operation:?} request must conform to the formal schema: {err}"
                )
            });
        }
    }

    #[test]
    fn with_trident_error_populates_kind_subkind_and_location_and_conforms_to_schema() {
        use trident_proto::v1::{FileLocation, TridentErrorKind};

        let schema: Value = serde_json::from_str(DESIGN_DOC_STATUS_SCHEMA).unwrap();
        let request = UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4().to_string(),
            operation: RequestedOperation::Finalize,
            target_version: Some("202606.29.0".to_string()),
            server: None,
            app_id: None,
            track: None,
        };

        use crate::core::trident::RemoteError;
        let remote_err = TridentClientError::Remote {
            operation: "commit",
            details: RemoteError {
                kind: Some(TridentErrorKind::ServicingError),
                subkind: "ab-update-reboot-check".to_string(),
                message: "reboot check failed".to_string(),
                error_message: "reboot check failed".to_string(),
                location: Some(FileLocation {
                    path: "crates/trident/src/servicing.rs".to_string(),
                    line: 42,
                }),
            },
        };
        let status = UpdateStatus::new(
            &request,
            Operation::Commit,
            request.operation_id.clone(),
            StatusCode::TargetBootFailed,
            format!("commit detected rollback to previous version: {remote_err}"),
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        )
        .with_trident_error(&remote_err);

        let trident_error = status
            .trident_error
            .as_ref()
            .expect("trident_error must be populated");
        assert_eq!(trident_error.kind, "SERVICING_ERROR");
        assert_eq!(trident_error.subkind, "ab-update-reboot-check");
        let location = trident_error
            .location
            .as_ref()
            .expect("location must be populated");
        assert_eq!(location.path, "crates/trident/src/servicing.rs");
        assert_eq!(location.line, 42);
        schema_validate(&schema, &serde_json::to_value(&status).unwrap())
            .expect("status carrying trident_error must still conform to the formal schema");
    }

    #[test]
    fn design_doc_status_schema_rejects_out_of_range_line() {
        // Regression test: ErrorLocation::line and protobuf
        // FileLocation::line are u32, so the formal JSON Schema must reject
        // negative and > u32::MAX values for `line` - otherwise a
        // schema-valid instance could fail to deserialize into
        // ErrorLocation.
        let schema: Value = serde_json::from_str(DESIGN_DOC_STATUS_SCHEMA).unwrap();
        let base = serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "nodeUpdateId": Uuid::new_v4().to_string(),
            "operationId": Uuid::new_v4().to_string(),
            "operation": "commit",
            "code": "TargetBootFailed",
            "message": "commit failed",
            "startedUtc": fixed_time(0).to_rfc3339(),
            "lastUpdatedUtc": fixed_time(0).to_rfc3339(),
            "finishedUtc": fixed_time(5).to_rfc3339(),
            "tridentError": {
                "kind": "SERVICING_ERROR",
                "subkind": "ab-update-reboot-check",
                "location": { "path": "crates/trident/src/servicing.rs", "line": 42 }
            }
        });

        let mut negative = base.clone();
        negative["tridentError"]["location"]["line"] = serde_json::json!(-1);
        schema_validate(&schema, &negative).expect_err("schema must reject a negative line number");

        let mut too_large = base.clone();
        too_large["tridentError"]["location"]["line"] = serde_json::json!(4294967296u64);
        schema_validate(&schema, &too_large)
            .expect_err("schema must reject a line number beyond u32::MAX");

        let mut fractional = base.clone();
        fractional["tridentError"]["location"]["line"] = serde_json::json!(1.5);
        schema_validate(&schema, &fractional)
            .expect_err("schema must reject a fractional line number");

        schema_validate(&schema, &base)
            .expect("baseline instance with an in-range line must conform to the schema");
    }

    #[test]
    fn with_trident_error_is_noop_for_non_remote_errors() {
        let request = UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4().to_string(),
            operation: RequestedOperation::Finalize,
            target_version: Some("202606.29.0".to_string()),
            server: None,
            app_id: None,
            track: None,
        };

        let timeout_err = TridentClientError::Timeout {
            operation: "commit",
            timeout: std::time::Duration::from_secs(5),
        };
        let status = UpdateStatus::new(
            &request,
            Operation::Commit,
            request.operation_id.clone(),
            StatusCode::OperationFailed,
            format!("commit failed: {timeout_err}"),
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        )
        .with_trident_error(&timeout_err);

        assert_eq!(status.trident_error, None);
    }

    #[test]
    fn agent_built_statuses_conform_to_formal_schema() {
        let schema: Value = serde_json::from_str(DESIGN_DOC_STATUS_SCHEMA).unwrap();
        let request = UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4().to_string(),
            operation: RequestedOperation::Finalize,
            target_version: Some("202606.29.0".to_string()),
            server: None,
            app_id: None,
            track: None,
        };

        // InProgress: startedUtc only, no finishedUtc yet.
        let in_progress = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::InProgress,
            "finalizing update",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            None,
        );
        schema_validate(&schema, &serde_json::to_value(&in_progress).unwrap())
            .expect("agent-constructed InProgress status must conform to the formal schema");

        // Terminal Success: both startedUtc and finishedUtc present.
        let success = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::Success,
            "boot armed, rebooting, awaiting commit",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );
        schema_validate(&schema, &serde_json::to_value(&success).unwrap())
            .expect("agent-constructed terminal Success status must conform to the formal schema");

        // Rollback status: no toVersion - still conforms (toVersion is optional).
        let rollback_request = UpdateRequest {
            operation: RequestedOperation::Rollback,
            target_version: None,
            ..request.clone()
        };
        let rollback_status = UpdateStatus::new(
            &rollback_request,
            Operation::Rollback,
            rollback_request.operation_id.clone(),
            StatusCode::Success,
            "rollback finalize completed; rebooting for commit",
            Some("2.0.0".to_string()),
            None,
            fixed_time(0),
            Some(fixed_time(5)),
        );
        schema_validate(&schema, &serde_json::to_value(&rollback_status).unwrap())
            .expect("agent-constructed rollback status must conform to the formal schema");

        // Derived post-reboot commit status: operationId stays unchanged
        // while the separate commit annotation key distinguishes the half.
        let commit_status = UpdateStatus::new(
            &request,
            Operation::Commit,
            request.operation_id.clone(),
            StatusCode::Success,
            "booted expected volume, boot order promoted",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(10),
            Some(fixed_time(15)),
        );
        schema_validate(&schema, &serde_json::to_value(&commit_status).unwrap())
            .expect("agent-constructed commit status must conform to the formal schema");
    }

    // --- server / appId / track (Nebraska fields, required for stage/finalize) -

    #[test]
    fn server_field_round_trips_and_conforms_to_formal_schema() {
        let schema: Value = serde_json::from_str(DESIGN_DOC_REQUEST_SCHEMA).unwrap();
        let mut request = valid_nebraska_request(RequestedOperation::Stage);
        request.operation_id = Uuid::new_v4().to_string();
        request.server = Some(Url::parse("https://nebraska.example/v1/update").unwrap());

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["server"], "https://nebraska.example/v1/update");
        schema_validate(&schema, &json)
            .expect("request with server/appId/track must conform to the formal schema");

        let round_tripped: UpdateRequest = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped.server, request.server);
    }

    #[test]
    fn server_field_absent_when_not_set() {
        let request = sample_request(RequestedOperation::Stage);
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("server").is_none());
    }

    #[test]
    fn app_id_field_round_trips_and_conforms_to_formal_schema() {
        let schema: Value = serde_json::from_str(DESIGN_DOC_REQUEST_SCHEMA).unwrap();
        let mut request = valid_nebraska_request(RequestedOperation::Stage);
        request.operation_id = Uuid::new_v4().to_string();
        request.app_id = Some("59bbad61-257d-47f4-9730-6848d88e1a6e".to_string());

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["appId"], "59bbad61-257d-47f4-9730-6848d88e1a6e");
        schema_validate(&schema, &json)
            .expect("request with server/appId/track must conform to the formal schema");

        let round_tripped: UpdateRequest = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped.app_id, request.app_id);
    }

    #[test]
    fn app_id_field_absent_when_not_set() {
        let request = sample_request(RequestedOperation::Stage);
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("appId").is_none());
    }

    #[test]
    fn track_field_round_trips_and_conforms_to_formal_schema() {
        let schema: Value = serde_json::from_str(DESIGN_DOC_REQUEST_SCHEMA).unwrap();
        let mut request = valid_nebraska_request(RequestedOperation::Stage);
        request.operation_id = Uuid::new_v4().to_string();
        request.track = Some("pin-202608.6.0".to_string());

        let json = serde_json::to_value(&request).unwrap();
        assert_eq!(json["track"], "pin-202608.6.0");
        schema_validate(&schema, &json)
            .expect("request with server/appId/track must conform to the formal schema");

        let round_tripped: UpdateRequest = serde_json::from_value(json).unwrap();
        assert_eq!(round_tripped.track, request.track);
    }

    #[test]
    fn track_field_absent_when_not_set() {
        let request = sample_request(RequestedOperation::Stage);
        let json = serde_json::to_value(&request).unwrap();
        assert!(json.get("track").is_none());
    }
}
