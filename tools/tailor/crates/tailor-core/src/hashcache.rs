//! Content hashing with a `(size, mtime)` cache — the shared fast path for tailor's incremental
//! fingerprint. Hashes file bytes with **XXH3-128** (fast, non-cryptographic — appropriate for a
//! change-detection fingerprint, not a security digest), and skips the read entirely when an
//! unchanged `(size, mtime)` cache entry already records the hash.
//!
//! Used by both the base-image hasher (`tailor-resolve`) and the declared-dependency hasher
//! (`deps.rs`) so both share one implementation and one on-disk cache format
//! (`meta/docs/2026-06-22-design.md` §9). The cache stores one small text entry per absolute path;
//! its format is stable (`CACHE_VERSION`) so entries survive across runs.

use std::{
    env,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use xxhash_rust::xxh3::{self, Xxh3};

use crate::atomic;

/// Width of a content hash in bytes (XXH3-128 → 16).
pub const CONTENT_HASH_BYTES: usize = 16;

/// Read this many bytes at a time when hashing (8 MiB) — large enough to keep the hasher fed on big
/// images without an unreasonable buffer.
const HASH_BUFFER_SIZE: usize = 8 * 1024 * 1024;
/// Bumped only if the cache line format changes, invalidating older entries.
const CACHE_VERSION: &str = "1";
const CACHE_FIELD_SEPARATOR: &str = " | ";
const CACHE_ENTRY_EXTENSION: &str = "txt";

/// A file's content hash and byte size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileHash {
    pub hash: [u8; CONTENT_HASH_BYTES],
    pub size: u64,
}

/// XXH3-128 content hash of `path`, reusing a cached hash when the file's `(size, mtime)` is
/// unchanged. When `cache_dir` is set, the result is recorded there (one entry per absolute path) so
/// the next run — and, within one run, a second caller hashing the same path — skips the read.
pub fn hash_file_cached(path: &Path, cache_dir: Option<&Path>) -> io::Result<FileHash> {
    let metadata = fs::metadata(path)?;
    let size = metadata.len();
    let mtime_ns = modified_time_ns(&metadata);
    let abs_path = absolute_path_string(path);

    if let (Some(dir), Some(mtime_ns)) = (cache_dir, mtime_ns)
        && let Some(hash) = read_cache_entry(dir, size, mtime_ns, &abs_path)
    {
        return Ok(FileHash { hash, size });
    }

    let hashed = hash_file(path)?;
    if let (Some(dir), Some(mtime_ns)) = (cache_dir, mtime_ns) {
        let _ = write_cache_entry(dir, hashed.size, mtime_ns, &hashed.hash, &abs_path);
    }
    Ok(hashed)
}

/// Hash `path`'s bytes with XXH3-128, unconditionally reading the file (no cache).
fn hash_file(path: &Path) -> io::Result<FileHash> {
    let mut file = File::open(path)?;
    let mut hasher = Xxh3::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; HASH_BUFFER_SIZE];

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }

    Ok(FileHash {
        hash: hasher.digest128().to_le_bytes(),
        size,
    })
}

fn modified_time_ns(metadata: &fs::Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

fn absolute_path_string(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        env::current_dir().map_or_else(|_| path.to_path_buf(), |cwd| cwd.join(path))
    };
    absolute.to_string_lossy().into_owned()
}

fn read_cache_entry(
    cache_dir: &Path,
    size: u64,
    mtime_ns: u128,
    abs_path: &str,
) -> Option<[u8; CONTENT_HASH_BYTES]> {
    let text = fs::read_to_string(cache_entry_path(cache_dir, abs_path)).ok()?;
    let line = text.trim_end_matches(['\n', '\r']);
    let mut fields = line.splitn(5, CACHE_FIELD_SEPARATOR);
    let version = fields.next()?;
    let stored_size = fields.next()?.parse::<u64>().ok()?;
    let stored_mtime_ns = fields.next()?.parse::<u128>().ok()?;
    let hash_hex = fields.next()?;
    let stored_path = fields.next()?;
    if version != CACHE_VERSION
        || stored_size != size
        || stored_mtime_ns != mtime_ns
        || stored_path != abs_path
    {
        return None;
    }

    let mut hash = [0_u8; CONTENT_HASH_BYTES];
    hex::decode_to_slice(hash_hex, &mut hash).ok()?;
    Some(hash)
}

fn write_cache_entry(
    cache_dir: &Path,
    size: u64,
    mtime_ns: u128,
    content_hash: &[u8; CONTENT_HASH_BYTES],
    abs_path: &str,
) -> Result<(), io::Error> {
    if abs_path.contains('\n') || abs_path.contains('\r') {
        return Ok(());
    }
    fs::create_dir_all(cache_dir)?;
    let hash_hex = hex::encode(content_hash);
    let line = format!(
        "{CACHE_VERSION}{CACHE_FIELD_SEPARATOR}{size}{CACHE_FIELD_SEPARATOR}{mtime_ns}{CACHE_FIELD_SEPARATOR}{hash_hex}{CACHE_FIELD_SEPARATOR}{abs_path}\n"
    );
    atomic::write(cache_entry_path(cache_dir, abs_path), line.as_bytes())
}

fn cache_entry_path(cache_dir: &Path, abs_path: &str) -> PathBuf {
    let key = hex::encode(xxh3::xxh3_128(abs_path.as_bytes()).to_le_bytes());
    cache_dir.join(format!("{key}.{CACHE_ENTRY_EXTENSION}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        thread,
        time::{Duration, SystemTime},
    };

    use tempfile::{TempDir, tempdir};

    #[test]
    fn hashes_content_and_size() {
        let dir = tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, b"hello").unwrap();
        let h = hash_file_cached(&file, None).unwrap();
        assert_eq!(h.size, 5);
        // Deterministic across calls.
        assert_eq!(h, hash_file_cached(&file, None).unwrap());
    }

    #[test]
    fn distinct_content_hashes_differently() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, b"one").unwrap();
        fs::write(&b, b"two").unwrap();
        assert_ne!(
            hash_file_cached(&a, None).unwrap().hash,
            hash_file_cached(&b, None).unwrap().hash
        );
    }

    #[test]
    fn a_cache_hit_reuses_the_recorded_hash() {
        let cache: TempDir = tempdir().unwrap();
        let dir = tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, b"original").unwrap();
        let first = hash_file_cached(&file, Some(cache.path())).unwrap();
        // A cache entry now exists for this (size, mtime); a second call returns the same hash.
        let second = hash_file_cached(&file, Some(cache.path())).unwrap();
        assert_eq!(first, second);
        assert_eq!(fs::read_dir(cache.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_content_change_that_moves_mtime_busts_the_cache() {
        let cache = tempdir().unwrap();
        let dir = tempdir().unwrap();
        let file = dir.path().join("f");
        fs::write(&file, b"before").unwrap();
        let before = hash_file_cached(&file, Some(cache.path())).unwrap();
        // Rewrite with new content and a bumped mtime so the (size, mtime) key differs.
        thread::sleep(Duration::from_millis(10));
        fs::write(&file, b"different-length-content").unwrap();
        filetime_bump(&file);
        let after = hash_file_cached(&file, Some(cache.path())).unwrap();
        assert_ne!(before.hash, after.hash);
    }

    fn filetime_bump(path: &Path) {
        // Touch mtime to now+1s so the cache key changes deterministically in the test.
        let now = SystemTime::now() + Duration::from_secs(1);
        let f = File::options().write(true).open(path).unwrap();
        f.set_modified(now).unwrap();
    }
}
