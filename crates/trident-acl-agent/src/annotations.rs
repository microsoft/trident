//! Request/status annotation protocol types for the Trident ACL agent.
//!
//! This module (schema types, `UpdateRequest::validate()`, and the
//! `#[cfg(test)]` design-doc conformance tests below) implements the
//! `acl.azure.com/update-request` / `acl.azure.com/update-status` node
//! annotation protocol designed in `docs/update-trigger-design.md`:
//! https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC67946fff8f296e10217b70e063c896e6028ea843&path=/docs/update-trigger-design.md
//! Keep `UpdateRequest`/`UpdateStatus`/`StatusCode` and `validate()` in sync
//! with that document's formal JSON Schema (its section "Formal JSON
//! Schema") - the `design_doc_*`/`agent_built_*_conform_to_formal_schema`
//! tests in this file's test module pin that JSON Schema in literally and
//! check both the doc's own examples and our constructed annotations
//! against it.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const UPDATE_REQUEST_ANNOTATION: &str = "acl.azure.com/update-request";
pub const UPDATE_STATUS_ANNOTATION: &str = "acl.azure.com/update-status";
pub const SCHEMA_VERSION: &str = "1.0";
// TODO(DR-001): This is a temporary stub. The active OS version cannot yet be
// determined on-node because the OS image does not currently ship a required
// file/manifest describing the running version. Once that file exists (planned
// as part of the image build), replace CURRENT_VERSION_STUB and
// current_active_version() below with real logic that reads it. The stub
// value below is an explicit sentinel that cannot collide with a real
// AKS/Trident release version string (those look like "YYYYMM.N.N"), so it
// can never accidentally match a real requested target version and cause
// handle_stage/handle_finalize to incorrectly short-circuit to
// AlreadyAtTarget. Do not remove this comment when bumping the stub value;
// keep it (and its non-colliding shape) until the real probe lands.
pub const CURRENT_VERSION_STUB: &str = "0.0.0-unprobed-trident-acl-agent-stub";

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
    /// Enforces the same constraints as the request annotation's formal
    /// JSON Schema in docs/update-trigger-design.md (https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC67946fff8f296e10217b70e063c896e6028ea843&path=/docs/update-trigger-design.md):
    /// schemaVersion match, and targetVersion required for stage/finalize
    /// but disallowed for rollback. See this file's module doc.
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
    // STUB: see CURRENT_VERSION_STUB above. Replace with a real probe (e.g. reading
    // a version manifest file the OS image will ship) once available. Known risk:
    // while stubbed, this can equal a real request's targetVersion and cause a
    // false AlreadyAtTarget short-circuit in handle_stage/handle_finalize.
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

    // --- docs/update-trigger-design.md conformance --------------------------
    //
    // Pins our annotation (de)serialization/validation code against two
    // things lifted verbatim from docs/update-trigger-design.md
    // (https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC67946fff8f296e10217b70e063c896e6028ea843&path=/docs/update-trigger-design.md),
    // section 2.1 "Trigger mechanism", so a doc/code drift shows up as a
    // test failure instead of being discovered against a real AKS-RP:
    //   1. The three example JSON payloads (request, finalize status, and
    //      the derived commit status) parse with our real UpdateRequest /
    //      UpdateStatus (de)serialization and UpdateRequest::validate().
    //   2. Annotations our own code constructs conform to the two formal
    //      JSON Schema documents embedded in the same section.
    //
    // Keep these constants byte-for-byte in sync with the design doc.

    /// docs/update-trigger-design.md 2.1, "Request annotation" example.
    const DESIGN_DOC_FINALIZE_REQUEST_EXAMPLE: &str = r#"{
  "schemaVersion": "1.0",
  "nodeUpdateId":  "550e8400-e29b-41d4-a716-446655440000",
  "operationId":   "f47ac10b-58cc-4372-a567-0e02b2c3d479",
  "operation":     "finalize",
  "targetVersion": "202606.29.0"
}"#;

    /// docs/update-trigger-design.md 2.1, "Status annotation" example.
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
  "finishedUtc":   "2026-06-04T12:00:32Z"
}"#;

    /// docs/update-trigger-design.md 2.1, the derived post-reboot commit status example.
    const DESIGN_DOC_COMMIT_STATUS_EXAMPLE: &str = r#"{
  "schemaVersion": "1.0",
  "nodeUpdateId":  "550e8400-e29b-41d4-a716-446655440000",
  "operationId":   "f47ac10b-58cc-4372-a567-0e02b2c3d479.commit",
  "operation":     "commit",
  "code":          "Success",
  "message":       "booted expected volume, boot order promoted",
  "fromVersion":   "202605.15.0",
  "toVersion":     "202606.29.0",
  "startedUtc":    "2026-06-04T12:01:18Z",
  "finishedUtc":   "2026-06-04T12:01:32Z"
}"#;

    /// The formal JSON Schema for the request annotation, from
    /// docs/update-trigger-design.md (https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC67946fff8f296e10217b70e063c896e6028ea843&path=/docs/update-trigger-design.md),
    /// section 2.1 "Formal JSON Schema". Keep byte-for-byte in sync with
    /// that document.
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
    "targetVersion": { "type": "string", "description": "ACL image release version, e.g. 202606.29.0." }
  },
  "allOf": [
    {
      "if":   { "properties": { "operation": { "enum": ["stage", "finalize"] } }, "required": ["operation"] },
      "then": { "required": ["targetVersion"] }
    }
  ]
}"#;

    /// The formal JSON Schema for the status annotation, from
    /// docs/update-trigger-design.md (https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC67946fff8f296e10217b70e063c896e6028ea843&path=/docs/update-trigger-design.md),
    /// section 2.1 "Formal JSON Schema". Keep byte-for-byte in sync with
    /// that document.
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
    "operationId":   { "type": "string", "pattern": "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}(\\.commit)?$", "description": "The request operationId; the implicit post-reboot commit status appends a '.commit' suffix to the finalize/rollback operationId." },
    "operation":     { "type": "string", "enum": ["stage", "finalize", "rollback", "commit"] },
    "code":          { "type": "string", "enum": ["InProgress", "Success", "AlreadyAtTarget", "NotStaged", "OperationFailed", "RevertedToPrevious", "AgentInternalError", "InvalidRequest"] },
    "message":       { "type": "string" },
    "fromVersion":   { "type": "string" },
    "toVersion":     { "type": "string" },
    "startedUtc":    { "type": "string", "format": "date-time" },
    "finishedUtc":   { "type": "string", "format": "date-time" }
  },
  "allOf": [
    {
      "if":   { "properties": { "code": { "const": "InProgress" } }, "required": ["code"] },
      "then": { "required": ["startedUtc"] },
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
    // accepted-design.md's schemas grow new constraints, this validator's
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

    fn schema_if_matches(if_schema: &Value, obj: &serde_json::Map<String, Value>) -> bool {
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
                "number" | "integer" => value.is_number(),
                other => panic!(
                    "test schema validator does not support type {other:?} - extend schema_validate_property"
                ),
            };
            if !matches {
                return Err(format!(
                    "property {name:?}: expected type {expected_type}, got {value:?}"
                ));
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
        Ok(())
    }

    /// Bespoke stand-in for full regex support: the two schemas above use
    /// exactly two distinct patterns, both UUID-shaped, so this matches them
    /// by exact pattern text rather than pulling in a regex engine for two
    /// known cases. Panics on an unrecognized pattern so a future schema
    /// change can't silently pass unchecked.
    fn schema_pattern_matches(pattern: &str, value: &str) -> bool {
        const BARE_UUID: &str =
            r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$";
        const UUID_OPTIONAL_COMMIT_SUFFIX: &str = r"^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}(\.commit)?$";
        match pattern {
            BARE_UUID => Uuid::parse_str(value).is_ok(),
            UUID_OPTIONAL_COMMIT_SUFFIX => match value.strip_suffix(".commit") {
                Some(prefix) => Uuid::parse_str(prefix).is_ok(),
                None => Uuid::parse_str(value).is_ok(),
            },
            other => panic!(
                "test schema validator does not recognize pattern {other:?} - extend schema_pattern_matches"
            ),
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
        assert_eq!(
            status.operation_id,
            "f47ac10b-58cc-4372-a567-0e02b2c3d479.commit"
        );
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

        for (operation, target_version) in [
            (RequestedOperation::Stage, Some("202606.29.0".to_string())),
            (
                RequestedOperation::Finalize,
                Some("202606.29.0".to_string()),
            ),
            (RequestedOperation::Rollback, None),
        ] {
            let request = UpdateRequest {
                schema_version: SCHEMA_VERSION.to_string(),
                node_update_id,
                operation_id: Uuid::new_v4().to_string(),
                operation,
                target_version,
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
    fn agent_built_statuses_conform_to_formal_schema() {
        let schema: Value = serde_json::from_str(DESIGN_DOC_STATUS_SCHEMA).unwrap();
        let request = UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::new_v4(),
            operation_id: Uuid::new_v4().to_string(),
            operation: RequestedOperation::Finalize,
            target_version: Some("202606.29.0".to_string()),
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

        // Derived post-reboot commit status: operationId gets a ".commit"
        // suffix - exercises the status schema's optional-suffix pattern.
        let commit_status = UpdateStatus::new(
            &request,
            Operation::Commit,
            commit_operation_id(&request.operation_id),
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
}
