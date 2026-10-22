//! Persists Nebraska's load-balancer affinity cookie
//! (`/var/lib/trident-acl-agent/nebraska-cookie` by default) across process
//! invocations and reboots.
//!
//! This is deliberately **not** part of `annotations::state`'s
//! `state.json`: that file exists to bridge one update *operation's*
//! pre-/post-reboot halves (every field there is keyed by `operation_id` or
//! is the single in-flight `pendingCommit`), and is read with
//! `deny_unknown_fields`. A separate cookie file preserves affinity across
//! process restarts and reboots without coupling its replacement policy to
//! the serialized update-state schema.

use std::{
    fs,
    io::{ErrorKind, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{anyhow, Context, Error};
use log::warn;
use reqwest::header::HeaderValue;
use uuid::Uuid;

/// The cookie is a bearer-style session-affinity credential, so store it
/// with owner-only permissions - the same posture `annotations::state`
/// already applies to `state.json`.
const COOKIE_FILE_MODE: u32 = 0o600;

/// Controls which responses may replace the persisted affinity cookie.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CookieLifetime {
    /// Start each initial update check without a cookie and retain only
    /// that response's cookie until the next initial check. A restarted
    /// initial check starts fresh; resuming completion does not reset it.
    /// If the initial response sets none, later responses cannot establish one.
    #[default]
    PerUpdate,
    /// Each response that supplies a cookie replaces the stored value for
    /// the next request. Responses without a cookie leave it unchanged.
    PerMessage,
}

impl FromStr for CookieLifetime {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "per-update" => Ok(Self::PerUpdate),
            "per-message" => Ok(Self::PerMessage),
            other => Err(anyhow!(
                "unknown cookie lifetime {other:?} (expected \"per-update\" or \"per-message\")"
            )),
        }
    }
}

/// Durable store for the single Nebraska affinity cookie this host has most
/// recently seen (e.g. an App-Service-style `ASLBSA`/`ASLBSACORS` pair,
/// already reduced to a `Cookie:`-header-ready string by
/// [`transport::ReqwestTransport`](super::transport::ReqwestTransport)).
#[derive(Debug, Clone)]
pub struct CookieStore {
    path: PathBuf,
}

impl CookieStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Loads the last-persisted cookie, or `None` if this host has never
    /// seen one yet (including a brand-new install with no file at all).
    pub fn load(&self) -> Result<Option<String>, Error> {
        match fs::read_to_string(&self.path) {
            Ok(raw) => {
                let trimmed = raw.trim();
                Ok(if trimmed.is_empty() {
                    None
                } else {
                    HeaderValue::from_str(trimmed).context("invalid Nebraska cookie header")?;
                    Some(trimmed.to_string())
                })
            }
            Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
            Err(err) => Err(err).with_context(|| format!("failed to read {}", self.path.display())),
        }
    }

    /// Atomically persists `cookie`, replacing any previous value.
    ///
    /// Mirrors `annotations::state::StateStore::save`'s crash safety: write
    /// to a sibling temp file at owner-only (0600) permissions, fsync it,
    /// then atomically rename over the real path.
    pub fn save(&self, cookie: &str) -> Result<(), Error> {
        HeaderValue::from_str(cookie).context("invalid Nebraska cookie header")?;
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
                .unwrap_or("nebraska-cookie"),
            Uuid::new_v4()
        ));

        // A per-write name prevents concurrent clients in this process
        // from removing or renaming each other's live temporary file.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(COOKIE_FILE_MODE)
            .open(&temp_path)
            .with_context(|| format!("failed to create {}", temp_path.display()))?;
        let result = (|| {
            file.write_all(cookie.as_bytes())
                .with_context(|| format!("failed to write {}", temp_path.display()))?;
            file.sync_all()
                .with_context(|| format!("failed to fsync {}", temp_path.display()))?;
            drop(file);

            fs::rename(&temp_path, &self.path).with_context(|| {
                format!(
                    "failed to atomically replace {} with {}",
                    self.path.display(),
                    temp_path.display()
                )
            })
        })();
        if result.is_err() {
            if let Err(err) = fs::remove_file(&temp_path) {
                if err.kind() != ErrorKind::NotFound {
                    warn!("failed to remove Nebraska cookie temporary file: {err}");
                }
            }
        }
        result?;

        // Best-effort: POSIX doesn't guarantee a rename is durable until the
        // containing directory's metadata is also synced, so fsync it too.
        // Not fatal if this fails (e.g. unsupported on some filesystems) -
        // the rename itself has already succeeded.
        if let Err(err) = fs::File::open(parent).and_then(|dir| dir.sync_all()) {
            warn!("Nebraska cookie was saved but directory sync failed: {err}");
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    fn store() -> (tempfile::TempDir, CookieStore) {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("nebraska-cookie");
        let store = CookieStore::new(path);
        (dir, store)
    }

    #[test]
    fn load_returns_none_when_file_is_missing() {
        let (_dir, store) = store();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn empty_cookie_loads_as_absent() {
        let (_dir, store) = store();
        store.save("").unwrap();
        assert_eq!(store.load().unwrap(), None);
    }

    #[test]
    fn invalid_cookie_headers_are_rejected_without_replacing_store() {
        let (_dir, store) = store();
        store.save("ASLBSA=valid").unwrap();
        store.save("ASLBSA=bad\r\nInjected: value").unwrap_err();
        assert_eq!(store.load().unwrap().as_deref(), Some("ASLBSA=valid"));
        fs::write(store.path(), "ASLBSA=bad\r\nInjected: value").unwrap();
        store.load().unwrap_err();
    }

    #[test]
    fn concurrent_saves_publish_whole_values_without_temporary_file_collisions() {
        let (dir, store) = store();
        let values = Arc::new(
            (0..8)
                .map(|i| format!("ASLBSA={}", i.to_string().repeat(8192)))
                .collect::<Vec<_>>(),
        );
        let barrier = Arc::new(Barrier::new(values.len()));
        let threads = (0..values.len())
            .map(|index| {
                let store = store.clone();
                let barrier = barrier.clone();
                let values = values.clone();
                thread::spawn(move || {
                    barrier.wait();
                    for _ in 0..8 {
                        store.save(&values[index]).unwrap();
                        assert!(values.contains(&store.load().unwrap().unwrap()));
                    }
                })
            })
            .collect::<Vec<_>>();
        for thread in threads {
            thread.join().unwrap();
        }
        assert!(values.contains(&store.load().unwrap().unwrap()));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn failed_rename_cleans_up_only_its_temporary_file() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("cookie");
        fs::create_dir(&destination).unwrap();
        let stale = dir.path().join("cookie.tmp-unrelated");
        fs::write(&stale, "keep this file").unwrap();
        CookieStore::new(&destination)
            .save("ASLBSA=new")
            .unwrap_err();
        assert!(destination.is_dir());
        assert_eq!(fs::read_to_string(stale).unwrap(), "keep this file");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn save_then_load_round_trips() {
        let (_dir, store) = store();
        store.save("ASLBSA=abc123").unwrap();
        assert_eq!(store.load().unwrap(), Some("ASLBSA=abc123".to_string()));
    }

    #[test]
    fn save_overwrites_a_previous_value() {
        let (_dir, store) = store();
        store.save("ASLBSA=old").unwrap();
        store.save("ASLBSA=new").unwrap();
        assert_eq!(store.load().unwrap(), Some("ASLBSA=new".to_string()));
    }

    #[test]
    fn saved_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, store) = store();
        store.save("ASLBSA=abc123").unwrap();
        let mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, COOKIE_FILE_MODE);
    }

    #[test]
    fn save_creates_missing_parent_directories() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("nested").join("nebraska-cookie");
        let store = CookieStore::new(path);
        store.save("ASLBSA=abc123").unwrap();
        assert_eq!(store.load().unwrap(), Some("ASLBSA=abc123".to_string()));
    }
}
