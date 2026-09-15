//! Error types for `tailor-core`, including the typed errors the **ports** expose to adapters.
//!
//! Port error types (`ExecError`, `ResolveError`) live here, not in the adapter crates, so that the
//! port traits in `ports` can name them without `tailor-core` depending on its own adapters
//! (`meta/docs/2026-06-22-architecture.md` §6). Adapters map their internal failures into these.

use std::{io::Error as IoError, path::PathBuf};

use tailor_config::Arch;

/// Errors from container execution adapters (the `Executor` / `ContainerRuntime` ports).
#[derive(Debug, thiserror::Error)]
pub enum ExecError {
    #[error("container runtime error: {0}")]
    Runtime(String),

    #[error("Image Customizer failed for {cell} (exit {code})\n\n{dump}")]
    IcFailed {
        cell: String,
        code: i64,
        dump: String,
    },

    #[error("execution cancelled")]
    Cancelled,

    #[error("{context}")]
    Io {
        context: String,
        #[source]
        source: IoError,
    },

    #[error("unsafe directory `{}`: {reason}", .path.display())]
    UnsafeDir { path: PathBuf, reason: String },

    #[error(
        "extraParams flag `{param}` is managed by tailor and cannot be overridden; \
         remove it from extraParams"
    )]
    ReservedParam { param: String },

    #[error("{0}")]
    Other(String),
}

/// Errors from base-image / digest resolution (the `BaseResolver` port).
#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("failed to read local base `{}`: {source}", .path.display())]
    LocalRead {
        path: PathBuf,
        #[source]
        source: IoError,
    },

    #[error("registry resolution failed for `{reference}`: {detail}")]
    Registry { reference: String, detail: String },

    #[error("{0}")]
    Other(String),
}

/// Top-level orchestration errors.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error(transparent)]
    Config(#[from] tailor_config::ConfigError),

    #[error(transparent)]
    Resolve(#[from] ResolveError),

    #[error(transparent)]
    Exec(#[from] ExecError),

    #[error(transparent)]
    Sign(#[from] crate::signing::SignError),

    #[error("tailor.lock is missing a `{platform}` entry for `{reference}`; run `tailor lock`")]
    LockMissing { reference: String, platform: String },

    #[error("tailor.lock is out of date: {detail}")]
    LockDrift { detail: String },

    #[error("image `{image}` declares no base for architecture `{arch}`")]
    MissingArchBase { image: String, arch: String },

    #[error(
        "image `{image}` references base image `{name}`, undefined in tailor.yaml `baseImages` (known: {known})"
    )]
    UnknownBaseImage {
        image: String,
        name: String,
        known: String,
    },

    #[error(
        "image `{image}` cell `{slug}` references base image `{name}` (arch `{slot_arch}`) but the cell arch is `{cell_arch}`; pick a matching slot or fix the `arch`"
    )]
    BaseImageArchMismatch {
        image: String,
        slug: String,
        name: String,
        slot_arch: Arch,
        cell_arch: Arch,
    },

    #[error("base image `{name}` is missing its file `{}`; run `tailor bases download {name}` or place it", .path.display())]
    BaseImageMissing { name: String, path: PathBuf },

    #[error("image `{image}` declares dependency path `{}`, which does not exist (relative to the image directory)", .path.display())]
    MissingDependency { image: String, path: PathBuf },

    #[error(
        "image `{image}` cell `{slug}` sets base `oci.platform: {platform}` (arch `{platform_arch}`) but the cell arch is `{cell_arch}`; declare a matching `arch` or fix the platform"
    )]
    PlatformArchMismatch {
        image: String,
        slug: String,
        platform: String,
        platform_arch: String,
        cell_arch: Arch,
    },

    #[error("unknown toolchain id `{id}` (image `{image}`)")]
    UnknownToolchain { id: String, image: String },

    #[error("toolchain `{id}` (`{reference}`) was not resolved before planning")]
    MissingResolvedToolchain { id: String, reference: String },

    #[error(
        "image `{image}` references tools-dir source `{name}`, undefined in tailor.yaml `toolsDirSources` (known: {known})"
    )]
    UnknownToolsDirSource {
        image: String,
        name: String,
        known: String,
    },

    #[error("tools-dir source `{reference}` was not resolved before planning")]
    MissingResolvedToolsDir { reference: String },

    #[error(
        "image `{image}` sets `toolsDir`, but its inline IC config must include previewFeatures: [tools-dir]"
    )]
    ToolsDirPreviewMissing { image: String },

    #[error(
        "internal error: no build directory resolved for image `{image}`'s tools-dir (buildDirBase should have defaulted)"
    )]
    WritableToolsDirNeedsBuildDir { image: String },

    #[error("invalid `--select` entry `{entry}` (expected `axis=value`)")]
    SelectorSyntax { entry: String },

    #[error(
        "`--select` references axis `{axis}`, which image `{image}` does not declare (axes: {declared})"
    )]
    UnknownSelectorAxis {
        axis: String,
        image: String,
        declared: String,
    },

    #[error("no cells match the selection for image `{image}`")]
    NoCellsSelected { image: String },

    #[error("image dependency cycle: {chain}")]
    DependencyCycle { chain: String },

    #[error(
        "image `{image}` depends on `{dependency}`, which is not a workspace image (known: {known})"
    )]
    UnknownDependencyImage {
        image: String,
        dependency: String,
        known: String,
    },

    #[error(
        "image `{image}` cell `{slug}` depends on `{producer}`, but `{producer}` has axis `{axis}` \
         that `{image}` does not — the producer cell is ambiguous ({axis} ∈ {{{values}}}); pin it \
         with `cell: {{ {axis}: <value> }}`"
    )]
    AmbiguousProducerCell {
        image: String,
        slug: String,
        producer: String,
        axis: String,
        values: String,
    },

    #[error(
        "image `{image}` cell `{slug}` depends on `{producer}` cell `{coord}`, which does not exist \
         ({producer} builds: {available})"
    )]
    MissingProducerCell {
        image: String,
        slug: String,
        producer: String,
        coord: String,
        available: String,
    },

    #[error(
        "image `{image}` requests output `{output}` from `{producer}`, which produces: {available}"
    )]
    UnknownProducerOutput {
        image: String,
        producer: String,
        output: String,
        available: String,
    },

    #[error("image `{image}` pins `{axis}={value}` on dependency `{producer}`, but {detail}")]
    BadDependencyPin {
        image: String,
        producer: String,
        axis: String,
        value: String,
        detail: String,
    },

    #[error(
        "image `{image}` references `${{inputs.{name}}}` in its config, but declares no input named \
         `{name}` (declared: {declared})"
    )]
    UnknownInput {
        image: String,
        name: String,
        declared: String,
    },

    #[error("failed to access `{}`: {source}", .path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: IoError,
    },

    #[error("failed to (de)serialize `{}`: {source}", .path.display())]
    Serde {
        path: PathBuf,
        #[source]
        source: serde_yaml_ng::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ic_failure_surfaces_the_categorized_dump() {
        let err = ExecError::IcFailed {
            cell: "appliance_amd64_cosi".to_owned(),
            code: 1,
            dump: "  image customization failed:\n  out of disk space\n\n  last IC context:\n    \
                   INFO  Installing packages"
                .to_owned(),
        };
        let rendered = err.to_string();
        // The header names the cell and the exit code.
        assert!(rendered.contains("Image Customizer failed for appliance_amd64_cosi (exit 1)"));
        // The categorized cause (not just the exit code) is visible.
        assert!(rendered.contains("out of disk space"));
        assert!(rendered.contains("last IC context"));
    }
}
