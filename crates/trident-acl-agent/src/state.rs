//! Persistent agent state (`/var/lib/trident-acl-agent/state.json`):
//! completed-operation cache and the pending post-reboot commit record.
//!
//! Implements the `state.json` mechanism from the current accepted design
//! (`accepted-design-v2.md`, section 2.3), which bridges the pre-reboot
//! finalize/rollback half and the post-reboot commit half of an operation
//! across the reboot.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::annotations::{Operation, UpdateRequest, UpdateStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersistentState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_commit: Option<PendingCommit>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub completed: BTreeMap<String, CompletedEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CompletedEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<UpdateStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<UpdateStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingCommit {
    pub request: UpdateRequest,
    pub operation_id: String,
    pub operation: Operation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_version: Option<String>,
    pub started_utc: chrono::DateTime<chrono::Utc>,
    pub boot_marker: String,
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
        let parent = match self.path.parent() {
            Some(parent) => {
                fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
                parent
            }
            None => Path::new("."),
        };

        let temp_path = parent.join(format!(
            "{}.tmp-{}",
            self.path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("state.json"),
            std::process::id()
        ));
        fs::write(&temp_path, serde_json::to_string_pretty(state)?)
            .with_context(|| format!("failed to write {}", temp_path.display()))?;
        fs::rename(&temp_path, &self.path).with_context(|| {
            format!(
                "failed to atomically replace {} with {}",
                self.path.display(),
                temp_path.display()
            )
        })
    }

    pub fn remember_completed(&self, status: UpdateStatus) -> Result<(), anyhow::Error> {
        let mut state = self.load()?;
        let entry = state
            .completed
            .entry(status.operation_id.clone())
            .or_default();
        match status.operation {
            Operation::Commit => entry.commit = Some(status),
            _ => entry.operation = Some(status),
        }
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
    use url::Url;
    use uuid::Uuid;

    use super::*;
    use crate::annotations::{RequestedOperation, StatusCode, SCHEMA_VERSION};

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
            server: None,
            app_id: None,
            track: None,
        }
    }

    fn sample_status(operation: Operation) -> UpdateStatus {
        UpdateStatus::new(
            &sample_request(),
            operation,
            "op-1".to_string(),
            StatusCode::Success,
            format!("{operation:?} completed"),
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
            boot_marker: "boot-1".to_string(),
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
        completed.insert(
            "op-1".to_string(),
            CompletedEntry {
                operation: Some(sample_status(Operation::Finalize)),
                commit: Some(sample_status(Operation::Commit)),
            },
        );
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
    fn remember_completed_tracks_operation_and_commit_separately_under_same_operation_id() {
        let (_dir, store) = store();
        store
            .remember_completed(sample_status(Operation::Finalize))
            .expect("remember_completed should succeed");
        store
            .remember_completed(sample_status(Operation::Commit))
            .expect("remember_completed should succeed");

        let state = store.load().expect("load should succeed");
        let entry = state.completed.get("op-1").expect("entry should exist");
        assert!(entry.operation.is_some());
        assert!(entry.commit.is_some());
        assert_eq!(state.completed.len(), 1);
    }

    #[test]
    fn remember_completed_overwrites_same_half_only() {
        let (_dir, store) = store();
        store
            .remember_completed(sample_status(Operation::Finalize))
            .expect("first remember_completed should succeed");

        let mut updated = sample_status(Operation::Finalize);
        updated.message = "updated message".to_string();
        store
            .remember_completed(updated)
            .expect("second remember_completed should succeed");

        let state = store.load().expect("load should succeed");
        let entry = state.completed.get("op-1").expect("entry should exist");
        assert_eq!(entry.operation.as_ref().unwrap().message, "updated message");
        assert!(entry.commit.is_none());
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
    fn pending_commit_persists_server_app_id_and_track_overrides() {
        // PendingCommit.request carries the whole UpdateRequest, so a
        // server/appId/track override present at finalize time must survive
        // the reboot unchanged, ready for the post-reboot commit's Nebraska
        // event report (see Orchestrator::resolve_nebraska_endpoint,
        // Orchestrator::resolve_nebraska_app_id, and
        // Orchestrator::resolve_nebraska_track).
        let (_dir, store) = store();
        let mut pending = sample_pending();
        pending.request.server = Some(Url::parse("https://nebraska.example/v1/update").unwrap());
        pending.request.app_id = Some("59bbad61-257d-47f4-9730-6848d88e1a6e".to_string());
        pending.request.track = Some("pin-202608.6.0".to_string());

        store
            .set_pending_commit(pending.clone())
            .expect("set_pending_commit should succeed");
        let state = store.load().expect("load should succeed");
        assert_eq!(state.pending_commit, Some(pending));
    }

    #[test]
    fn set_pending_commit_preserves_existing_completed_entries() {
        let (_dir, store) = store();
        store
            .remember_completed(sample_status(Operation::Finalize))
            .expect("remember_completed should succeed");
        store
            .set_pending_commit(sample_pending())
            .expect("set_pending_commit should succeed");

        let state = store.load().expect("load should succeed");
        assert!(state.pending_commit.is_some());
        assert_eq!(state.completed.len(), 1);
        assert!(state.completed.contains_key("op-1"));
    }

    #[test]
    fn save_is_atomic_replace() {
        let (_dir, store) = store();
        store
            .save(&PersistentState::default())
            .expect("initial save should succeed");

        let metadata_before = std::fs::metadata(store.path()).expect("state file should exist");
        let state = PersistentState {
            pending_commit: Some(sample_pending()),
            completed: BTreeMap::new(),
        };
        store.save(&state).expect("second save should succeed");

        let metadata_after = std::fs::metadata(store.path()).expect("state file should exist");
        assert!(metadata_after.len() > 0);
        assert!(metadata_before.modified().is_ok());
    }

    #[test]
    fn deserialize_rejects_unknown_top_level_fields() {
        let err = serde_json::from_str::<PersistentState>(
            r#"{"pendingCommit": null, "completed": {}, "unexpectedField": true}"#,
        )
        .unwrap_err();

        assert!(err.to_string().contains("unexpectedField"));
    }
}
