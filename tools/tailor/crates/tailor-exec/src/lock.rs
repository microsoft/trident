//! A per-worktree build lock. A build writes shared state under the output directory (stamps, the
//! hash cache, artifacts); two concurrent builds sharing one output directory would race on those
//! writes. An advisory `flock` on `<output>/.tailor/build.lock` admits a single build per output
//! directory and is released automatically when the process exits — even on a crash — so it never
//! leaves a stale lock behind.

use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg},
};

const LOCK_FILE: &str = "build.lock";

/// An acquired exclusive build lock. Dropping it (or the process exiting) releases the lock.
pub struct WorktreeLock {
    // Held for its `flock` side effect; released on drop.
    _flock: Flock<File>,
}

impl WorktreeLock {
    /// Try to acquire the exclusive build lock in `state_dir` (the output directory's `.tailor`
    /// state directory). Returns `Ok(None)` when another build already holds it, so the caller can
    /// report a friendly "already running" error rather than blocking indefinitely.
    pub fn acquire(state_dir: impl AsRef<Path>) -> io::Result<Option<Self>> {
        let state_dir = state_dir.as_ref();
        std::fs::create_dir_all(state_dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state_dir.join(LOCK_FILE))?;
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(flock) => Ok(Some(Self { _flock: flock })),
            Err((_file, Errno::EWOULDBLOCK | Errno::EACCES)) => Ok(None),
            Err((_file, errno)) => Err(io::Error::from(errno)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn second_acquire_is_blocked_while_the_first_is_held() {
        let dir = TempDir::new().unwrap();
        let first = WorktreeLock::acquire(dir.path()).unwrap();
        assert!(first.is_some(), "first build should acquire the lock");

        let second = WorktreeLock::acquire(dir.path()).unwrap();
        assert!(
            second.is_none(),
            "a second build in the same worktree must be refused"
        );
    }

    #[test]
    fn lock_is_reacquirable_after_release() {
        let dir = TempDir::new().unwrap();
        {
            let held = WorktreeLock::acquire(dir.path()).unwrap();
            assert!(held.is_some());
        }
        // Dropped above → releasable again.
        let again = WorktreeLock::acquire(dir.path()).unwrap();
        assert!(again.is_some(), "lock must be free after the holder drops");
    }
}
