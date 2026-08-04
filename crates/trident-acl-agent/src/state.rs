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
