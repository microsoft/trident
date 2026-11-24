//! In-memory fakes for testing port consumers without Docker or a network (`meta/docs/2026-06-22-architecture.md`
//! §8.3). The orchestrator and downstream crates use these to exercise the build pipeline.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use tailor_config::{Arch, BaseSource, ToolchainEntry, ToolsDirSourceInline};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
use tempfile::TempDir;

use crate::{
    domain::Cell,
    error::{ExecError, ResolveError},
    ports::{
        BaseResolver, ExecutionContext, ExecutionResult, Executor, ResolvedBase, RuntimeConfig,
    },
};

#[cfg(test)]
pub(crate) struct WorkOrderWorkspace(TempDir);

#[cfg(test)]
impl WorkOrderWorkspace {
    pub(crate) fn new() -> Self {
        Self(TempDir::new_in(".").unwrap())
    }

    pub(crate) fn target(&self, name: &str, fields: &str) -> Arc<crate::Target> {
        use std::fs;

        use tailor_config::{
            BaseImageCatalogue, ImageDefinition, OutputArtifactsPolicy, OutputFormat, OutputSpec,
        };

        let dir = self.0.path().join(name);
        fs::create_dir_all(&dir).unwrap();
        let yaml = format!("name: {name}\n{fields}\nconfig: {{}}\n");
        fs::write(dir.join("image.yaml"), &yaml).unwrap();
        let definition: ImageDefinition = serde_yaml_ng::from_str(&yaml).unwrap();
        Arc::new(crate::Target {
            definition,
            dir,
            root: self.0.path().to_path_buf(),
            default_outputs: vec![OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: None,
                compression: None,
            }],
            output_artifacts: OutputArtifactsPolicy::default(),
            base_images: BaseImageCatalogue::default(),
            tools_dir_sources: Vec::new(),
        })
    }
}

/// A deterministic `BaseResolver`: local paths hash to zeros, registry bases get a fixed digest.
#[derive(Debug, Default, Clone)]
pub struct FakeResolver;

impl BaseResolver for FakeResolver {
    async fn resolve(
        &self,
        source: &BaseSource,
        arch: Arch,
        _image_dir: &Path,
    ) -> Result<ResolvedBase, ResolveError> {
        let platform = format!("linux/{arch}");
        Ok(match source {
            // Catalogue references collapse to a `path` base before resolution; `image` bases are
            // lowered to a `path` base too. Treat any leftover as a local file so test doubles stay
            // deterministic.
            BaseSource::Path { .. } | BaseSource::Ref { .. } | BaseSource::Image { .. } => {
                ResolvedBase::LocalFile {
                    content_hash: [0; 16],
                    size: 0,
                }
            }
            BaseSource::Oci { oci } => ResolvedBase::Oci {
                reference: oci.uri.clone(),
                platform,
                digest: "sha256:fakeoci".to_owned(),
            },
            BaseSource::AzureLinux { azure_linux } => ResolvedBase::Oci {
                reference: format!(
                    "mcr.microsoft.com/azurelinux/{}/image/{}",
                    azure_linux.version, azure_linux.variant
                ),
                platform,
                digest: "sha256:fakeazl".to_owned(),
            },
        })
    }

    async fn resolve_toolchain(&self, _toolchain: &ToolchainEntry) -> Result<String, ResolveError> {
        Ok("sha256:faketoolchain".to_owned())
    }

    async fn resolve_tools_dir(
        &self,
        _source: &ToolsDirSourceInline,
    ) -> Result<String, ResolveError> {
        Ok("sha256:faketoolsdir".to_owned())
    }
}

/// An `Executor` that records the slug of every cell it is asked to run and produces no files.
#[derive(Debug, Default, Clone)]
pub struct FakeExecutor {
    calls: Arc<Mutex<Vec<String>>>,
}

impl FakeExecutor {
    /// A handle to the recorded invocations (cell slugs, in call order).
    pub fn recorder(&self) -> Arc<Mutex<Vec<String>>> {
        Arc::clone(&self.calls)
    }
}

impl Executor for FakeExecutor {
    async fn execute(
        &self,
        cell: &Cell,
        context: &ExecutionContext,
        _cancel: CancellationToken,
    ) -> Result<ExecutionResult, ExecError> {
        let slug = cell.slug.0.clone();
        let artifact_path = context.output_dir.join(&slug);
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(slug);
        }
        Ok(ExecutionResult {
            artifact_path,
            exit_code: 0,
            logs: String::new(),
        })
    }

    async fn clean(
        &self,
        _paths: &[PathBuf],
        _runtime: &RuntimeConfig,
        _cancel: CancellationToken,
    ) -> Result<(), ExecError> {
        Ok(())
    }
}
