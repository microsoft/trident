//! Crash-safe file writes: stage into a sibling temp file, `fsync`, then `rename` over the target.
//! A rename within a directory is atomic on POSIX, so a reader (or a later run) sees either the old
//! bytes or the complete new bytes — never a torn write left by an interrupted build. Build stamps,
//! the hash cache, and the published artifact all use this so an interrupted build cannot leave a
//! half-written file that a later run would trust.

use std::{
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
    process,
    sync::atomic::{AtomicU64, Ordering},
};

/// A monotonic counter making concurrent temp paths in one process distinct (belt-and-suspenders:
/// the single-worktree guard already precludes two concurrent builds sharing an output dir).
static COUNTER: AtomicU64 = AtomicU64::new(0);

/// A unique sibling temp path next to `path` (same directory → same filesystem, so the finalizing
/// rename is atomic). The name is hidden and carries the pid + a counter to avoid collisions.
pub fn temp_sibling(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let temp_name = format!(".{file_name}.tmp.{}.{seq}", process::id());
    match path.parent() {
        Some(parent) => parent.join(temp_name),
        None => PathBuf::from(temp_name),
    }
}

/// Atomically replace `path`'s contents with `bytes`: write a sibling temp file, `fsync` it, then
/// rename it over `path`. Creates the parent directory if needed.
pub fn write(path: impl AsRef<Path>, bytes: &[u8]) -> io::Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = temp_sibling(path);
    finish(&temp, path, |file| file.write_all(bytes))
}

/// Stage the target through a sibling temp file: run `write_into` to populate `temp`, `fsync` it,
/// then rename it over `dest`. On any error the temp file is removed. Lets callers stream large
/// content (e.g. a compressed artifact) into place atomically without buffering it in memory.
pub fn finish(
    temp: &Path,
    dest: &Path,
    write_into: impl FnOnce(&mut File) -> io::Result<()>,
) -> io::Result<()> {
    let result = (|| {
        let mut file = File::create(temp)?;
        write_into(&mut file)?;
        file.sync_all()?;
        fs::rename(temp, dest)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn write_replaces_contents_and_leaves_no_temp() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("stamp.json");
        write(&path, b"first").unwrap();
        write(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");

        // No stray temp files remain in the directory.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "stamp.json")
            .collect();
        assert!(leftovers.is_empty(), "unexpected temp files: {leftovers:?}");
    }

    #[test]
    fn write_creates_missing_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/deep/cache.txt");
        write(&path, b"x").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"x");
    }

    #[test]
    fn finish_removes_the_temp_on_failure() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("out.bin");
        let temp = temp_sibling(&dest);
        let err = finish(&temp, &dest, |_| Err(io::Error::other("boom"))).unwrap_err();
        assert_eq!(err.to_string(), "boom");
        assert!(!temp.exists(), "temp file must be cleaned up on failure");
        assert!(!dest.exists(), "dest must not be created on failure");
    }
}
