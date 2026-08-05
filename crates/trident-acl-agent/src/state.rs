//! Persistent agent state (`/var/lib/trident-acl-agent/state.json`):
//! completed-operation cache and the pending post-reboot commit record.
//!
//! Implements the `state.json` mechanism from `docs/update-trigger-design.md`:
//! https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC67946fff8f296e10217b70e063c896e6028ea843&path=/docs/update-trigger-design.md
//! (section 2.3), which bridges the pre-reboot finalize/rollback half and
//! the post-reboot commit half of an operation across the reboot.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::annotations::{UpdateRequest, UpdateStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersistentState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_commit: Option<PendingCommit>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub completed: BTreeMap<String, UpdateStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingCommit {
    pub request: UpdateRequest,
    pub operation_id: String,
    pub operation: crate::annotations::Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<String>,
    pub started_utc: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct StateStore {
    path: PathBuf,
}

impl StateStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load(&self) -> Result<PersistentState, anyhow::Error> {
        match fs::read_to_string(&self.path) {
            Ok(raw) => Ok(serde_json::from_str(&raw).context("failed to parse state.json")?),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(PersistentState::default())
            }
            Err(err) => {
                Err(anyhow::Error::new(err)
                    .context(format!("failed to read {}", self.path.display())))
            }
        }
    }

    pub fn save(&self, state: &PersistentState) -> Result<(), anyhow::Error> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&self.path, serde_json::to_string_pretty(state)?)
            .with_context(|| format!("failed to write {}", self.path.display()))
    }

    pub fn remember_completed(&self, status: UpdateStatus) -> Result<(), anyhow::Error> {
        let mut state = self.load()?;
        state.completed.insert(status.operation_id.clone(), status);
        self.save(&state)
    }

    pub fn set_pending_commit(&self, pending: PendingCommit) -> Result<(), anyhow::Error> {
        let mut state = self.load()?;
        state.pending_commit = Some(pending);
        self.save(&state)
    }

    pub fn clear_pending_commit(&self) -> Result<(), anyhow::Error> {
        let mut state = self.load()?;
        state.pending_commit = None;
        self.save(&state)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use uuid::Uuid;

    use super::*;
    use crate::annotations::{Operation, RequestedOperation, StatusCode, SCHEMA_VERSION};

    fn store() -> (tempfile::TempDir, StateStore) {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("state.json");
        let store = StateStore::new(path);
        (dir, store)
    }

    fn sample_request() -> UpdateRequest {
        UpdateRequest {
            schema_version: SCHEMA_VERSION.to_string(),
            node_update_id: Uuid::new_v4(),
            operation_id: "op-1".to_string(),
            operation: RequestedOperation::Finalize,
            target_version: Some("2.0.0".to_string()),
        }
    }

    fn sample_status(operation_id: &str) -> UpdateStatus {
        UpdateStatus::new(
            &sample_request(),
            Operation::Finalize,
            operation_id.to_string(),
            StatusCode::Success,
            "finalize completed",
            Some("1.0.0".to_string()),
            Some("2.0.0".to_string()),
            Utc::now(),
            Some(Utc::now()),
        )
    }

    fn sample_pending() -> PendingCommit {
        PendingCommit {
            request: sample_request(),
            operation_id: "op-1".to_string(),
            operation: Operation::Finalize,
            from_version: Some("1.0.0".to_string()),
            to_version: Some("2.0.0".to_string()),
            started_utc: Utc::now(),
        }
    }

    #[test]
    fn load_returns_default_when_file_missing() {
        let (_dir, store) = store();
        let state = store
            .load()
            .expect("load should not fail when file is absent");
        assert_eq!(state, PersistentState::default());
        assert!(state.pending_commit.is_none());
        assert!(state.completed.is_empty());
    }

    #[test]
    fn save_then_load_round_trips_full_state() {
        let (_dir, store) = store();
        let mut completed = std::collections::BTreeMap::new();
        completed.insert("op-1".to_string(), sample_status("op-1"));
        let state = PersistentState {
            pending_commit: Some(sample_pending()),
            completed,
        };
        store.save(&state).expect("save should succeed");
        let loaded = store.load().expect("load should succeed after save");

        assert_eq!(loaded, state);
    }

    #[test]
    fn save_creates_parent_directories() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let nested_path = dir.path().join("nested").join("deeper").join("state.json");
        let store = StateStore::new(nested_path.clone());

        store
            .save(&PersistentState::default())
            .expect("save should create missing parent directories");

        assert!(nested_path.exists());
    }

    #[test]
    fn remember_completed_inserts_by_operation_id_without_clobbering_others() {
        let (_dir, store) = store();
        store
            .remember_completed(sample_status("op-1"))
            .expect("remember_completed should succeed");
        store
            .remember_completed(sample_status("op-2"))
            .expect("remember_completed should succeed");

        let state = store.load().expect("load should succeed");
        assert_eq!(state.completed.len(), 2);
        assert!(state.completed.contains_key("op-1"));
        assert!(state.completed.contains_key("op-2"));
    }

    #[test]
    fn remember_completed_overwrites_same_operation_id() {
        let (_dir, store) = store();
        store
            .remember_completed(sample_status("op-1"))
            .expect("first remember_completed should succeed");

        let mut updated = sample_status("op-1");
        updated.message = "updated message".to_string();
        store
            .remember_completed(updated)
            .expect("second remember_completed should succeed");

        let state = store.load().expect("load should succeed");
        assert_eq!(state.completed.len(), 1);
        assert_eq!(state.completed["op-1"].message, "updated message");
    }

    #[test]
    fn set_and_clear_pending_commit_round_trip() {
        let (_dir, store) = store();
        assert!(store.load().unwrap().pending_commit.is_none());

        let pending = sample_pending();
        store
            .set_pending_commit(pending.clone())
            .expect("set_pending_commit should succeed");
        let state = store.load().expect("load should succeed");
        assert_eq!(state.pending_commit, Some(pending));

        store
            .clear_pending_commit()
            .expect("clear_pending_commit should succeed");
        let state = store.load().expect("load should succeed");
        assert!(state.pending_commit.is_none());
    }

    #[test]
    fn set_pending_commit_preserves_existing_completed_entries() {
        let (_dir, store) = store();
        store
            .remember_completed(sample_status("op-1"))
            .expect("remember_completed should succeed");
        store
            .set_pending_commit(sample_pending())
            .expect("set_pending_commit should succeed");

        let state = store.load().expect("load should succeed");
        assert!(state.pending_commit.is_some());
        assert_eq!(state.completed.len(), 1);
    }

    #[test]
    fn load_fails_with_context_on_corrupt_json() {
        let (_dir, store) = store();
        std::fs::write(store.path(), "not valid json").expect("failed to write corrupt state");

        let err = store.load().expect_err("load must fail on corrupt JSON");
        assert!(
            err.to_string().contains("failed to parse state.json"),
            "error should mention state.json parsing, got: {err}"
        );
    }

    #[test]
    fn deny_unknown_fields_rejects_unrecognized_state_json_keys() {
        let (_dir, store) = store();
        std::fs::write(
            store.path(),
            r#"{"pendingCommit": null, "completed": {}, "unexpectedField": true}"#,
        )
        .expect("failed to write state with unknown field");

        let err = store
            .load()
            .expect_err("load must reject unknown fields per deny_unknown_fields");
        assert!(err.to_string().contains("failed to parse state.json"));
    }
}
