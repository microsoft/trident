use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const UPDATE_REQUEST_ANNOTATION: &str = "acl.azure.com/update-request";
pub const UPDATE_STATUS_ANNOTATION: &str = "acl.azure.com/update-status";
pub const SCHEMA_VERSION: &str = "1.0";
pub const CURRENT_VERSION_STUB: &str = "202601.1.0";

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum RequestedOperation {
    Stage,
    Finalize,
    Rollback,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum Operation {
    Stage,
    Finalize,
    Rollback,
    Commit,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
pub enum StatusCode {
    InProgress,
    Success,
    AlreadyAtTarget,
    NotStaged,
    OperationFailed,
    RevertedToPrevious,
    AgentInternalError,
    InvalidRequest,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateRequest {
    pub schema_version: String,
    pub node_update_id: Uuid,
    pub operation_id: String,
    pub operation: RequestedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpdateStatus {
    pub schema_version: String,
    pub node_update_id: Uuid,
    pub operation_id: String,
    pub operation: Operation,
    pub code: StatusCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<String>,
    pub started_utc: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_utc: Option<DateTime<Utc>>,
}

impl UpdateRequest {
    pub fn validate(self) -> Result<Self, String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!("unsupported schemaVersion {}", self.schema_version));
        }
        match self.operation {
            RequestedOperation::Stage | RequestedOperation::Finalize => {
                if self.target_version.as_deref().unwrap_or("").is_empty() {
                    return Err("targetVersion is required for stage/finalize".to_string());
                }
            }
            RequestedOperation::Rollback => {
                if self.target_version.is_some() {
                    return Err("targetVersion must be omitted for rollback".to_string());
                }
            }
        }
        Ok(self)
    }
}

impl UpdateStatus {
    // This constructor mirrors UpdateStatus's wire schema field-for-field
    // (see accepted-design.md's two-annotation JSON protocol); splitting it
    // into a builder would add ceremony across ~25 call sites in
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
        Self {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: request.node_update_id,
            operation_id,
            operation,
            code,
            message: message.into(),
            from_version,
            to_version,
            started_utc,
            finished_utc,
        }
    }
}

pub fn current_active_version() -> String {
    // TODO: Replace this stub with a real /etc-based active-version probe, or
    // compare an AKS-RP-supplied expected fromVersion against the on-disk value
    // once the accepted design grows that request field.
    CURRENT_VERSION_STUB.to_string()
}

pub fn commit_operation_id(operation_id: &str) -> String {
    format!("{operation_id}.commit")
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
    use chrono::TimeZone;
    use serde_json::Value;
    use uuid::Uuid;

    use super::*;

    fn sample_request(operation: RequestedOperation) -> UpdateRequest {
        UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::parse_str("11111111-1111-1111-1111-111111111111").unwrap(),
            operation_id: "op-1".to_string(),
            operation,
            target_version: Some("2.0.0".to_string()),
        }
    }

    fn fixed_time(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
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
        assert!(json.get("startedUtc").is_some());
        assert!(json.get("finishedUtc").is_some());
        // Confirms camelCase renaming applies to every field, not just a subset.
        assert!(json.get("nodeUpdateId").is_some());
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
    fn finalize_failure_reverted_annotation_has_reverted_to_previous_code() {
        let request = sample_request(RequestedOperation::Finalize);
        let status = UpdateStatus::new(
            &request,
            Operation::Finalize,
            request.operation_id.clone(),
            StatusCode::RevertedToPrevious,
            "finalize failed: trident reported ab-update-reboot-check failure",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            fixed_time(0),
            Some(fixed_time(5)),
        );

        let json = to_annotation_json(&status);
        assert_eq!(json["code"], "RevertedToPrevious");
        assert_eq!(json["operationId"], "op-1");
    }

    #[test]
    fn commit_success_annotation_uses_commit_suffixed_operation_id() {
        let request = sample_request(RequestedOperation::Finalize);
        let commit_id = commit_operation_id(&request.operation_id);
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
        assert_eq!(json["operationId"], "op-1.commit");
        assert_eq!(json["operation"], "commit");
        assert_eq!(json["code"], "Success");
    }

    #[test]
    fn commit_reboot_required_annotation_uses_agent_internal_error_code() {
        let request = sample_request(RequestedOperation::Finalize);
        let commit_id = commit_operation_id(&request.operation_id);
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
        let commit_id = commit_operation_id(&request.operation_id);

        for message in [
            "commit failed: trident reported ab-update-reboot-check failure",
            "commit failed: trident reported ab-update-health-check-commit-check failure",
        ] {
            let status = UpdateStatus::new(
                &request,
                Operation::Commit,
                commit_id.clone(),
                StatusCode::RevertedToPrevious,
                message,
                Some("1.0.0".to_string()),
                Some("2.0.0".to_string()),
                fixed_time(0),
                Some(fixed_time(5)),
            );

            let json = to_annotation_json(&status);
            assert_eq!(json["code"], "RevertedToPrevious");
            assert_eq!(json["operationId"], "op-1.commit");
        }
    }

    #[test]
    fn commit_failure_generic_annotation_has_operation_failed_code() {
        let request = sample_request(RequestedOperation::Finalize);
        let commit_id = commit_operation_id(&request.operation_id);
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
        assert_eq!(json["operationId"], "op-1.commit");
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
        assert!(json.get("finishedUtc").is_none());
        assert_eq!(json["fromVersion"], "1.0.0");
    }
}
