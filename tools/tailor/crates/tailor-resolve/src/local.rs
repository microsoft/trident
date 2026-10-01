use std::path::Path;

use tokio::task;
use tracing::debug;

use tailor_core::{ResolveError, ResolvedBase, hashcache};

pub(crate) async fn resolve(
    path: impl AsRef<Path>,
    cache_dir: Option<&Path>,
) -> Result<ResolvedBase, ResolveError> {
    let path = path.as_ref().to_path_buf();
    let cache_dir = cache_dir.map(Path::to_path_buf);
    task::spawn_blocking(move || resolve_blocking(&path, cache_dir.as_deref()))
        .await
        .map_err(|source| ResolveError::Other(format!("local base hash task failed: {source}")))?
}

fn resolve_blocking(path: &Path, cache_dir: Option<&Path>) -> Result<ResolvedBase, ResolveError> {
    // The shared hasher (`tailor-core::hashcache`) XXH3-hashes the file, reusing a cached hash when
    // the base's (size, mtime) is unchanged so a repeat build skips the (potentially multi-GB) read.
    debug!(path = %path.display(), "resolving local base hash (size+mtime cache)");
    let hashed =
        hashcache::hash_file_cached(path, cache_dir).map_err(|source| ResolveError::LocalRead {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(ResolvedBase::LocalFile {
        content_hash: hashed.hash,
        size: hashed.size,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tailor_core::hashcache::CONTENT_HASH_BYTES;
    use tempfile::tempdir;
    use xxhash_rust::xxh3;

    fn expected_hash(content: &[u8]) -> [u8; CONTENT_HASH_BYTES] {
        xxh3::xxh3_128(content).to_le_bytes()
    }

    #[tokio::test]
    async fn hashes_local_file_and_reports_size() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("base.raw");
        let content = b"tailor local base\n";
        fs::write(&path, content).unwrap();

        let resolved = resolve(&path, None).await.unwrap();

        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: expected_hash(content),
                size: content.len() as u64,
            }
        );
    }

    #[tokio::test]
    async fn reports_local_read_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("missing.raw");

        let err = resolve(&path, None).await.unwrap_err();

        assert!(matches!(err, ResolveError::LocalRead { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn a_cache_dir_is_threaded_through_and_populated() {
        let dir = tempdir().unwrap();
        let cache_dir = dir.path().join("cache");
        let path = dir.path().join("base.raw");
        let content = b"real content";
        fs::write(&path, content).unwrap();

        // First resolve populates the cache; the second reads it. Both return the true hash.
        let first = resolve(&path, Some(&cache_dir)).await.unwrap();
        let second = resolve(&path, Some(&cache_dir)).await.unwrap();
        assert_eq!(first, second);
        assert_eq!(
            first,
            ResolvedBase::LocalFile {
                content_hash: expected_hash(content),
                size: content.len() as u64,
            }
        );
        assert!(cache_dir.is_dir(), "the cache dir should be created");
        assert_eq!(fs::read_dir(&cache_dir).unwrap().count(), 1);
    }

    #[tokio::test]
    async fn an_unusable_cache_dir_falls_back_to_hashing() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("base.raw");
        let cache_dir = dir.path().join("cache-as-file");
        let content = b"uncached content";
        fs::write(&path, content).unwrap();
        fs::write(&cache_dir, b"not a directory").unwrap();

        let resolved = resolve(&path, Some(&cache_dir)).await.unwrap();

        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: expected_hash(content),
                size: content.len() as u64,
            }
        );
    }
}
