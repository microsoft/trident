//! Domain model: the resolved image (`Target`), a single build unit (`Cell`), the build plan, and
//! the canonical per-cell `Fingerprint` (`meta/docs/2026-06-22-architecture.md` §4.1).

use std::{collections::BTreeMap, fmt, path::PathBuf, sync::Arc};

use serde_yaml_ng::Value;
use tailor_config::{
    Arch, BaseImageCatalogue, BaseSource, ExtraParam, ImageDefinition, OutputArtifactsPolicy,
    OutputSpec, ToolsDirSource, ToolsDirSourceInline,
};

/// A resolved image — the catalogue/authoring unit, after config load and defaults are applied.
#[derive(Debug, Clone)]
pub struct Target {
    /// The authored image definition (name, matrix, features, config, …).
    pub definition: ImageDefinition,
    /// The directory containing `image.yaml` (fragment and `$include` resolution root).
    pub dir: PathBuf,
    /// Output formats to build when a cell declares none (workspace `defaults.outputs`).
    pub default_outputs: Vec<OutputSpec>,
    /// Resolved `output.artifacts` staging policy (per-image override, else workspace default, else
    /// `managed`) — consulted only for cells that opt into the `output-artifacts` preview feature
    /// (`meta/docs/2026-06-29-output-artifacts-staging.md` §3).
    pub output_artifacts: OutputArtifactsPolicy,
    /// The workspace root, the base-dir for catalogue slot paths (`meta/docs/2026-06-29-base-image-catalogue.md` §3).
    pub root: PathBuf,
    /// The `tailor.yaml` base-image catalogue, against which `base: { ref: … }` is resolved.
    pub base_images: BaseImageCatalogue,
    /// The workspace `toolsDirSources` catalogue for `toolsDir.source: <name>` references.
    pub tools_dir_sources: Vec<ToolsDirSource>,
}

impl Target {
    /// The user-facing image name.
    pub fn name(&self) -> &str {
        &self.definition.name
    }
}

/// One cell in the build matrix: exactly one Image Customizer invocation producing one artifact.
#[derive(Debug, Clone)]
pub struct Cell {
    /// The image this cell belongs to.
    pub target: Arc<Target>,
    /// The full cell coordinate (every declared axis), in matrix-declared order.
    pub axes: BTreeMap<String, String>,
    /// The target architecture (drives `--platform linux/<arch>`).
    pub arch: Arch,
    /// The single output this cell builds.
    pub output: OutputSpec,
    /// The unique slug: `<image>_<axis values, matrix order>_<format>` (`meta/docs/2026-06-22-design.md` §10).
    pub slug: CellSlug,
    /// The merged, interpolated Image Customizer config for this cell.
    pub ic_config: Value,
    /// The resolved base image source. A catalogue reference is resolved to its slot's local file, so
    /// downstream sees a `path` base; [`Cell::base_image`] keeps the slot name for the matrix output.
    pub base: BaseSource,
    /// The `baseImages` slot name this cell resolved from, when its base is a catalogue reference
    /// (`meta/docs/2026-06-29-base-image-catalogue.md` §6.2); `None` for `path`/`oci`/`azureLinux` bases.
    pub base_image: Option<String>,
    /// Local RPM sources passed to IC as `--rpm-source`.
    pub rpm_sources: Vec<PathBuf>,
    /// Extra IC command-line flags appended verbatim after every tailor-managed flag.
    pub extra_params: Vec<ExtraParam>,
    /// Resolved `${inputs.<name>}` producer artifact paths substituted into `ic_config`, in declared
    /// order. Content-hashed into the fingerprint (like `extraDependencies`) so a producer rebuild
    /// invalidates this cell (`meta/docs/2026-09-09-inter-image-dependencies.md`). Live in the (already
    /// bound) output dir, so they need no extra container bind.
    pub input_deps: Vec<PathBuf>,
    /// Resolved IC tools-dir source, when this image requests tailor-managed `--tools-dir`.
    pub tools_dir: Option<CellToolsDir>,
    /// When `true`, this cell is dropped from bulk selection unless specifically requested — the run
    /// names it (`--cell`) or pins a member of [`skip_pins`](Cell::skip_pins) via `-s`
    /// (`meta/docs/2026-07-22-fragment-skip.md`).
    pub skip: bool,
    /// The `(axis, value)` coordinates a `-s` selector must pin to keep this cell despite `skip`;
    /// empty when not skipped (or skipped image-wide, which is handled during image selection).
    pub skip_pins: Vec<(String, String)>,
}

/// A cell's tools-dir source after resolving an image-level named reference or inline source.
#[derive(Debug, Clone)]
pub struct CellToolsDir {
    pub source: ToolsDirSourceInline,
    pub source_name: Option<String>,
}

/// A cell's unique slug — also the output basename, working-copy, and build-stamp key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CellSlug(pub String);

impl fmt::Display for CellSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for CellSlug {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

/// The canonical per-cell fingerprint: a SHA-256 over every build-affecting input (`meta/docs/2026-06-22-design.md`
/// §9.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    /// The lowercase hex digest, as recorded in build stamps.
    pub fn to_hex(self) -> String {
        hex::encode(self.0)
    }
}

impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// One cell with its fingerprint and incremental verdict.
#[derive(Debug, Clone)]
pub struct PlannedCell {
    pub cell: Cell,
    pub fingerprint: Fingerprint,
    /// `true` if a build stamp records the same fingerprint and the artifact exists.
    pub up_to_date: bool,
    /// The digest-pinned IC `--image` reference for a registry base (`oci:<repo>@sha256:…`), or
    /// `None` for a local-file base. Resolved during planning so the build stays reproducible.
    pub base_ref: Option<String>,
    /// Resolved/staged tools-dir plan, used by the executor and fingerprint.
    pub tools_dir: Option<crate::ports::ToolsDirPlan>,
}

/// An ordered list of cells to (re)build.
#[derive(Debug, Clone, Default)]
pub struct BuildPlan {
    pub cells: Vec<PlannedCell>,
}

impl BuildPlan {
    /// Cells that actually need building (not up to date).
    pub fn stale(&self) -> impl Iterator<Item = &PlannedCell> {
        self.cells.iter().filter(|planned| !planned.up_to_date)
    }
}
