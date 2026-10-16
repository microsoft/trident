//! Persists Nebraska's load-balancer affinity cookie
//! (`/var/lib/trident-acl-agent/nebraska-cookie` by default) across process
//! invocations and reboots.
//!
//! This is deliberately **not** part of `annotations::state`'s
//! `state.json`: that file exists to bridge one update *operation's*
//! pre-/post-reboot halves (every field there is keyed by `operation_id` or
//! is the single in-flight `pendingCommit`), and is read with
//! `deny_unknown_fields`. Nebraska's affinity cookie is a host/connection
//! -level attribute with no relationship to any particular operation - it
//! must outlive operations completing, failing, or being reset, and
//! vice versa - so it gets its own small file instead.

use std::{
    fs,
    io::{ErrorKind, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process,
};

use anyhow::{Context, Error};

/// The cookie is a bearer-style session-affinity credential, so store it
/// with owner-only permissions - the same posture `annotations::state`
/// already applies to `state.json`.
const COOKIE_FILE_MODE: u32 = 0o600;

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
            process::id()
        ));

        {
            // create_new (not create+truncate) so the file is always newly
            // created with COOKIE_FILE_MODE - see StateStore::save for why
            // truncating a stale temp file left behind by a prior crash
            // would silently keep that file's existing permissions. A
            // leftover temp file (same process::id() reused across
            // reboots) can only be this process's own abandoned write,
            // never another process's live file, so it is removed and
            // retried once.
            let mut open_opts = fs::OpenOptions::new();
            open_opts
                .write(true)
                .create_new(true)
                .mode(COOKIE_FILE_MODE);
            let mut file = match open_opts.open(&temp_path) {
                Ok(file) => file,
                Err(err) if err.kind() == ErrorKind::AlreadyExists => {
                    fs::remove_file(&temp_path).with_context(|| {
                        format!("failed to remove stale {}", temp_path.display())
                    })?;
                    open_opts
                        .open(&temp_path)
                        .with_context(|| format!("failed to create {}", temp_path.display()))?
                }
                Err(err) => {
                    return Err(err)
                        .with_context(|| format!("failed to create {}", temp_path.display()))
                }
            };
            file.write_all(cookie.as_bytes())
                .with_context(|| format!("failed to write {}", temp_path.display()))?;
            file.sync_all()
                .with_context(|| format!("failed to fsync {}", temp_path.display()))?;
        }

        fs::rename(&temp_path, &self.path).with_context(|| {
            format!(
                "failed to atomically replace {} with {}",
                self.path.display(),
                temp_path.display()
            )
        })?;

        // Best-effort: POSIX doesn't guarantee a rename is durable until the
        // containing directory's metadata is also synced, so fsync it too.
        // Not fatal if this fails (e.g. unsupported on some filesystems) -
        // the rename itself has already succeeded.
        if let Ok(dir) = fs::File::open(parent) {
            let _ = dir.sync_all();
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
