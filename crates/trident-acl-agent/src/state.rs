//! Persistent agent state, used to carry information across the A/B reboot.
//!
//! In `--events full` mode the agent must, after the reboot, report an
//! update-complete event (`3/2`) to Nebraska with the correct
//! `previousversion`. Because the process dies at the reboot, the version we
//! were on before the update is recorded here on the shared, persistent root
//! (`/var`) and read back by the restarted agent.

use std::{fs, io::ErrorKind, path::PathBuf};

use anyhow::{Context, Error};
use log::debug;
use semver::Version;
use serde::{Deserialize, Serialize};

/// Directory holding the agent's persistent state. On the ACL demo image this
/// is on the shared ext4 root, so it survives the A/B update and reboot.
const STATE_DIR: &str = "/var/lib/trident-acl-agent";

/// File within [`STATE_DIR`] holding the serialized [`PendingUpdate`].
const STATE_FILE: &str = "state.json";

/// A record of an update that has been finalized locally but whose completion
/// has not yet been reported to Nebraska (because it completes across a reboot).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingUpdate {
    /// The version the agent was running before triggering the update.
    pub previous_version: Version,

    /// The version the agent expects to be running after the reboot.
    pub target_version: Version,
}

fn state_path() -> PathBuf {
    PathBuf::from(STATE_DIR).join(STATE_FILE)
}

/// Records a pending update to disk, to be picked up after the reboot. Best
/// effort: the state directory is created if missing.
pub fn record_pending_update(pending: &PendingUpdate) -> Result<(), Error> {
    let dir = PathBuf::from(STATE_DIR);
    fs::create_dir_all(&dir)
        .with_context(|| format!("Failed to create state directory '{}'", dir.display()))?;
    let path = state_path();
    let json = serde_json::to_string_pretty(pending).context("Failed to serialize agent state")?;
    fs::write(&path, json)
        .with_context(|| format!("Failed to write agent state to '{}'", path.display()))?;
    debug!("Recorded pending update to '{}': {pending:?}", path.display());
    Ok(())
}

/// Loads the pending update from disk, if any. Returns `Ok(None)` when no state
/// file exists (the common case).
pub fn load_pending_update() -> Result<Option<PendingUpdate>, Error> {
    let path = state_path();
    let json = match fs::read_to_string(&path) {
        Ok(json) => json,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e)
                .with_context(|| format!("Failed to read agent state from '{}'", path.display()))
        }
    };
    let pending =
        serde_json::from_str(&json).context("Failed to deserialize agent state")?;
    Ok(Some(pending))
}

/// Clears any recorded pending update. Treats a missing file as success.
pub fn clear_pending_update() -> Result<(), Error> {
    let path = state_path();
    match fs::remove_file(&path) {
        Ok(()) => {
            debug!("Cleared pending update state at '{}'", path.display());
            Ok(())
        }
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e)
            .with_context(|| format!("Failed to remove agent state '{}'", path.display())),
    }
}
