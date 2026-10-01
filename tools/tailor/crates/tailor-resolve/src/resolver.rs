use std::path::{Path, PathBuf};

use tailor_config::{Arch, BaseSource, ToolchainEntry, ToolsDirSourceInline};
use tailor_core::{BaseResolver, ResolveError, ResolvedBase};

use crate::{azure_linux, local, oci, toolchain};

#[derive(Debug, Default, Clone)]
pub struct OciResolver {
    cache_dir: Option<PathBuf>,
}

impl OciResolver {
    pub fn new() -> Self {
        Self { cache_dir: None }
    }

    pub fn with_cache_dir(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: Some(cache_dir.into()),
        }
    }
}

impl BaseResolver for OciResolver {
    async fn resolve(
        &self,
        source: &BaseSource,
        arch: Arch,
        image_dir: &Path,
    ) -> Result<ResolvedBase, ResolveError> {
        match source {
            // A relative `path` is authored relative to the image directory; resolve it against
            // `image_dir` (never the process CWD) so this hash/existence check sees the same file IC
            // will (`--image-file` is built the same way in tailor-exec).
            BaseSource::Path { path, .. } => {
                local::resolve(
                    tailor_config::absolutize(path, image_dir),
                    self.cache_dir.as_deref(),
                )
                .await
            }
            BaseSource::Oci { oci } => oci::resolve(oci, arch).await,
            BaseSource::AzureLinux { azure_linux } => azure_linux::resolve(azure_linux, arch).await,
            // Catalogue references are collapsed to a `path` base before resolution (orchestrator).
            BaseSource::Ref { reference } => Err(ResolveError::Other(format!(
                "unresolved base reference `{reference}`: a catalogue reference must be expanded before resolution"
            ))),
            // `base: { image }` is lowered to a `path` base before resolution (orchestrator).
            BaseSource::Image { image, .. } => Err(ResolveError::Other(format!(
                "unresolved image base `{image}`: a workspace-image base must be lowered before resolution"
            ))),
        }
    }

    async fn resolve_toolchain(&self, toolchain: &ToolchainEntry) -> Result<String, ResolveError> {
        toolchain::resolve(toolchain).await
    }

    async fn resolve_tools_dir(
        &self,
        source: &ToolsDirSourceInline,
    ) -> Result<String, ResolveError> {
        toolchain::resolve_tools_dir(source).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    use tempfile::tempdir;
    use xxhash_rust::xxh3;

    #[tokio::test]
    async fn dispatches_local_sources() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("base.raw");
        let content = b"resolver dispatch";
        fs::write(&path, content).unwrap();
        let source = BaseSource::Path { path, arch: None };

        let resolved = OciResolver::new()
            .resolve(&source, Arch::Amd64, dir.path())
            .await
            .unwrap();

        let expected = xxh3::xxh3_128(content).to_le_bytes();
        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: expected,
                size: content.len() as u64,
            }
        );
    }

    /// Regression: a relative `base.path` must be resolved against the image directory, not the
    /// process CWD. The base file lives one level *up* from the image dir (`<root>/artifacts/...`),
    /// reached via `../artifacts/...` — which only resolves correctly when joined onto `image_dir`.
    #[tokio::test]
    async fn resolves_relative_path_against_image_dir_not_cwd() {
        let root = tempdir().unwrap();
        let image_dir = root.path().join("image");
        let artifacts = root.path().join("artifacts");
        fs::create_dir_all(&image_dir).unwrap();
        fs::create_dir_all(&artifacts).unwrap();
        let content = b"baremetal base";
        fs::write(artifacts.join("base.raw"), content).unwrap();

        let source = BaseSource::Path {
            path: "../artifacts/base.raw".into(),
            arch: None,
        };

        let resolved = OciResolver::new()
            .resolve(&source, Arch::Amd64, &image_dir)
            .await
            .unwrap();

        let expected = xxh3::xxh3_128(content).to_le_bytes();
        assert_eq!(
            resolved,
            ResolvedBase::LocalFile {
                content_hash: expected,
                size: content.len() as u64,
            }
        );
    }
}
