use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

use indexmap::IndexMap;
use semver::Version;
use serde::Deserialize;
use serde_yaml_ng::Value;

use crate::{
    error::ConfigError,
    types::{
        Arch, Compression, LogLevel, Operation, OutputArtifactsPolicy, OutputFormat, ParamValue,
        PreviewFeature,
    },
};

// ===== tailor.yaml — workspace / tool config (reference/tailor-yaml.md) =====

/// The highest `schemaVersion` this build of tailor understands. A config may declare any version
/// from 1 up to this (older majors stay supported — backward compatible); a higher version is
/// rejected because it may use semantics this tailor does not implement. Bump when a
/// backward-incompatible config change ships.
pub const SUPPORTED_SCHEMA_VERSION: u32 = 1;

/// `tailor.yaml` — the workspace root: toolchains, runtime, defaults, and the image catalogue.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolConfig {
    pub schema_version: u32,
    /// Opt-in switches for tailor features that are not yet part of the stable 1.0 contract (e.g.
    /// `signing`). A gated feature used without being listed here is a hard error.
    #[serde(default)]
    pub preview_features: Vec<PreviewFeature>,
    pub toolchains: Toolchains,
    #[serde(default)]
    pub tools_dir_sources: Vec<ToolsDirSource>,
    #[serde(default)]
    pub runtime: Option<Runtime>,
    #[serde(default)]
    pub defaults: Option<Defaults>,
    /// Workspace-wide signing profiles (`meta/docs/2026-06-29-signing.md` §4).
    #[serde(default)]
    pub signing: Option<SigningConfig>,
    #[serde(default)]
    pub images: Option<ImageCatalogue>,
    /// Named base-image slots referenced by `base: { ref: <name> }` (`meta/docs/2026-06-29-base-image-catalogue.md` §3).
    #[serde(default)]
    pub base_images: Option<BaseImageCatalogue>,
    /// Declarative render-ahead of committed IC config YAMLs for a non-tailor pipeline
    /// (`meta/docs/2026-07-20-export-configs-only.md`). Its presence makes `tailor export` /
    /// `tailor export --check` argument-free.
    #[serde(default)]
    pub export: Option<ExportConfig>,
}

/// `export:` — declarative render-ahead of committed IC config YAMLs
/// (`meta/docs/2026-07-20-export-configs-only.md`). `tailor export` renders every selected cell's
/// merged IC config to `<outputDir>/<slug>.yaml`; `tailor export --check` fails on drift.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExportConfig {
    /// Committed output directory (relative to the workspace root): one `<slug>.yaml` per cell.
    pub output_dir: PathBuf,
    /// What to emit; defaults to configs-only (the only scope today).
    #[serde(default)]
    pub scope: ExportScope,
    /// Restrict to a subset of images; empty ⇒ all images in the workspace.
    #[serde(default)]
    pub images: Vec<String>,
}

/// What `tailor export` emits. Only `configsOnly` exists today; an enum so future scopes (standalone
/// build scripts, a manifest) can be added without a breaking config change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportScope {
    /// Only the merged IC config YAML per cell.
    #[default]
    ConfigsOnly,
}

impl ToolConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Reject a config newer than this tailor understands (`schemaVersion: 0` is never valid).
        // Older/current versions pass — images and fragments inherit this one workspace version.
        if self.schema_version == 0 || self.schema_version > SUPPORTED_SCHEMA_VERSION {
            return Err(ConfigError::UnsupportedSchemaVersion {
                found: self.schema_version,
                supported: SUPPORTED_SCHEMA_VERSION,
            });
        }
        self.toolchains.validate()?;
        ensure_unique_names(
            "toolsDirSources",
            self.tools_dir_sources
                .iter()
                .map(|source| source.name.as_str()),
        )?;
        if let Some(base_images) = &self.base_images {
            base_images.validate()?;
        }
        Ok(())
    }

    /// Whether `feature` is opted in via `previewFeatures:` in `tailor.yaml`.
    #[must_use]
    pub fn preview_enabled(&self, feature: PreviewFeature) -> bool {
        self.preview_features.contains(&feature)
    }
}

/// Repo-wide Image Customizer toolchain(s). This is where the IC version lives.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Toolchains {
    pub default: String,
    pub entries: Vec<ToolchainEntry>,
}

impl Toolchains {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&ToolchainEntry> {
        self.entries.iter().find(|entry| entry.name == name)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        ensure_unique_names(
            "toolchains.entries",
            self.entries.iter().map(|entry| entry.name.as_str()),
        )
    }
}

/// A pinned Image Customizer container.
///
/// `version` is optional, informational metadata (tailor does not gate IC versions — `meta/docs/2026-06-22-design.md`
/// §8). The registry tag actually pulled is `tag`, else `version`, else `latest` — see
/// [`ToolchainEntry::effective_tag`].
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainEntry {
    pub name: String,
    pub container: String,
    #[serde(default)]
    pub version: Option<Version>,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub pull: PullPolicy,
}

impl ToolchainEntry {
    /// The registry tag to pull: an explicit `tag`, else the `version` string, else the built-in
    /// [`DEFAULT_IC_TAG`](crate::defaults::DEFAULT_IC_TAG) (`meta/docs/2026-06-22-design.md` §5.1).
    pub fn effective_tag(&self) -> String {
        if let Some(tag) = &self.tag {
            return tag.clone();
        }
        self.version.as_ref().map_or_else(
            || crate::defaults::DEFAULT_IC_TAG.to_owned(),
            Version::to_string,
        )
    }
}

/// A named container userspace exported and passed to IC as `--tools-dir`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsDirSource {
    pub name: String,
    pub container: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub pull: PullPolicy,
}

impl ToolsDirSource {
    /// The registry tag to pull: an explicit `tag`, else `latest`.
    pub fn effective_tag(&self) -> String {
        self.tag
            .clone()
            .unwrap_or_else(|| crate::defaults::DEFAULT_IC_TAG.to_owned())
    }

    pub fn inline(&self) -> ToolsDirSourceInline {
        ToolsDirSourceInline {
            container: self.container.clone(),
            tag: self.tag.clone(),
            pull: self.pull,
        }
    }
}

/// Inline tools-dir source fields (`toolsDir.source: { container, tag? }`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolsDirSourceInline {
    pub container: String,
    #[serde(default)]
    pub tag: Option<String>,
    #[serde(default)]
    pub pull: PullPolicy,
}

impl ToolsDirSourceInline {
    /// The registry tag to pull: an explicit `tag`, else `latest`.
    pub fn effective_tag(&self) -> String {
        self.tag
            .clone()
            .unwrap_or_else(|| crate::defaults::DEFAULT_IC_TAG.to_owned())
    }
}

/// How an image selects its toolchain: an id (workspace) or an inline definition (standalone).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ToolchainRef {
    Id(String),
    Inline(ToolchainEntry),
}

/// The container engine tailor talks to. Podman speaks the same Docker Engine API, so it needs no
/// separate implementation — only a different socket (`meta/docs/2026-06-29-container-runtimes.md` §2-§3).
///
/// This is a *selector*: it picks the default socket and the `auto` probe order. The engine that
/// actually governs runtime behavior is whatever the endpoint reports on connect (§4-§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// The Docker daemon (default; today's behavior, unchanged).
    #[default]
    Docker,
    /// Podman via its Docker-compatible API (`podman system service`).
    Podman,
    /// Probe known Docker/Podman sockets and use the first that answers.
    Auto,
}

impl Engine {
    /// The lowercase token (`docker` | `podman` | `auto`), as written in `tailor.yaml` / `--engine`.
    pub fn as_str(self) -> &'static str {
        match self {
            Engine::Docker => "docker",
            Engine::Podman => "podman",
            Engine::Auto => "auto",
        }
    }
}

impl std::fmt::Display for Engine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Container-runtime (bollard) knobs.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Runtime {
    /// Which container engine to use: `docker` (default), `podman`, or `auto`
    /// (`meta/docs/2026-06-29-container-runtimes.md` §3). A selector only — the live daemon governs behavior.
    #[serde(default)]
    pub engine: Option<Engine>,
    /// An explicit engine endpoint (`unix://…`, `tcp://…`, or a bare socket path), overriding the
    /// engine's default socket and the `DOCKER_HOST` / `CONTAINER_HOST` environment variables.
    #[serde(default)]
    pub host: Option<String>,
    #[serde(default)]
    pub privileged: Option<bool>,
    #[serde(default)]
    pub mounts: Option<Mounts>,
    #[serde(default)]
    pub build_dir_base: Option<PathBuf>,
    #[serde(default)]
    pub log_level: Option<LogLevel>,
    #[serde(default)]
    pub image_cache_dir: Option<PathBuf>,
    /// Opt-in directory for per-cell IC debug logs; persistence is off unless set
    /// (`meta/docs/2026-06-29-logging.md` §5.5). Overridden by `--log-dir` and `TAILOR_LOG_DIR`.
    #[serde(default)]
    pub log_dir: Option<PathBuf>,
    #[serde(default)]
    pub janitor_image: Option<JanitorImage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Mounts {
    #[serde(default)]
    pub host_root: Option<PathBuf>,
    #[serde(default)]
    pub dev: Option<bool>,
    #[serde(default)]
    pub extra_paths: Vec<ExtraMount>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtraMount {
    pub path: PathBuf,
    #[serde(default)]
    pub access: Access,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Access {
    #[default]
    Ro,
    Rw,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PullPolicy {
    Always,
    #[default]
    Missing,
    Never,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JanitorImage {
    pub container: String,
    #[serde(default)]
    pub tag: Option<String>,
}

// ===== Signing (meta/docs/2026-06-29-signing.md) =====

/// Workspace-wide signing profiles (`tailor.yaml`). An image opts in with [`ImageDefinition::signing`]
/// (`meta/docs/2026-06-29-signing.md` §4).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SigningConfig {
    /// Profile used when an image says `signing: true`.
    #[serde(default)]
    pub default: Option<String>,
    /// Named signing profiles, keyed by id.
    #[serde(default)]
    pub profiles: BTreeMap<String, SigningProfile>,
}

/// One signing profile: a key-source `backend` plus its backend-specific settings. Private key
/// material is always **referenced** (a path), never inlined (`meta/docs/2026-06-29-signing.md` §4, §9).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SigningProfile {
    /// The key-source backend.
    pub backend: SigningBackend,
    /// `keypair`: PEM private key file.
    #[serde(default)]
    pub key: Option<PathBuf>,
    /// `keypair`: PEM signing certificate file.
    #[serde(default)]
    pub cert: Option<PathBuf>,
    /// `local-test-ca`: where to write the enrollable CA certificate.
    #[serde(default)]
    pub publish_ca_cert: Option<PathBuf>,
    /// `azure-key-vault`: vault URL.
    #[serde(default)]
    pub vault: Option<String>,
    /// `azure-key-vault`: certificate name.
    #[serde(default)]
    pub certificate: Option<String>,
}

/// Where a signing key comes from (`meta/docs/2026-06-29-signing.md` §6). The PE signer is orthogonal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SigningBackend {
    /// Mint a self-signed CA + leaf per build with pure-Rust `rcgen` (MVP / CI; not a production root).
    LocalTestCa,
    /// Bring your own PEM key + certificate.
    Keypair,
    /// Remote signing via Azure Key Vault (future).
    AzureKeyVault,
}

impl SigningBackend {
    /// The lowercase token as written in `tailor.yaml`.
    pub fn as_str(self) -> &'static str {
        match self {
            SigningBackend::LocalTestCa => "local-test-ca",
            SigningBackend::Keypair => "keypair",
            SigningBackend::AzureKeyVault => "azure-key-vault",
        }
    }
}

impl SigningProfile {
    /// Structural validation: every backend's required fields are present (`meta/docs/2026-06-29-signing.md` §4).
    /// This is *config* validation only; capability/filesystem probing is the build-time preflight
    /// (`tailor-core` signing preflight, §5.1).
    pub fn validate(&self, profile_id: &str) -> Result<(), ConfigError> {
        let missing = |field: &str| ConfigError::InvalidSigningProfile {
            profile: profile_id.to_owned(),
            detail: format!("backend `{}` requires `{field}`", self.backend.as_str()),
        };
        match self.backend {
            SigningBackend::Keypair => {
                if self.key.is_none() {
                    return Err(missing("key"));
                }
                if self.cert.is_none() {
                    return Err(missing("cert"));
                }
            }
            SigningBackend::AzureKeyVault => {
                if self.vault.is_none() {
                    return Err(missing("vault"));
                }
                if self.certificate.is_none() {
                    return Err(missing("certificate"));
                }
            }
            SigningBackend::LocalTestCa => {}
        }
        Ok(())
    }
}

/// An image's `signing:` opt-in: `true` ⇒ the workspace default profile, a string ⇒ that named
/// profile, `false`/omitted ⇒ unsigned (`meta/docs/2026-06-29-signing.md` §4).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum SigningRef {
    /// `signing: true` (default profile) or `signing: false` (unsigned).
    Enabled(bool),
    /// `signing: <profile-id>`.
    Profile(String),
}

/// Resolve an image's `signing:` ref against the workspace profiles (`meta/docs/2026-06-29-signing.md` §4).
///
/// Returns the resolved `(profile_id, profile)` for a signed image, or `None` when unsigned
/// (`false` / omitted). Validates the resolved profile structurally.
pub fn resolve_signing<'a>(
    image: Option<&SigningRef>,
    workspace: Option<&'a SigningConfig>,
) -> Result<Option<(String, &'a SigningProfile)>, ConfigError> {
    let reference = match image {
        None | Some(SigningRef::Enabled(false)) => return Ok(None),
        Some(reference) => reference,
    };
    let config = workspace.ok_or_else(|| ConfigError::SigningMisconfigured {
        detail: "an image requests signing, but `tailor.yaml` defines no `signing:` profiles"
            .to_owned(),
    })?;
    let profile_id = match reference {
        SigningRef::Enabled(true) => {
            config
                .default
                .clone()
                .ok_or_else(|| ConfigError::SigningMisconfigured {
                    detail: "an image says `signing: true`, but `signing.default` is not set"
                        .to_owned(),
                })?
        }
        SigningRef::Profile(name) => name.clone(),
        SigningRef::Enabled(false) => unreachable!("handled above"),
    };
    let profile =
        config
            .profiles
            .get(&profile_id)
            .ok_or_else(|| ConfigError::UnknownSigningProfile {
                profile: profile_id.clone(),
            })?;
    profile.validate(&profile_id)?;
    Ok(Some((profile_id, profile)))
}

/// Defaults inherited by every image that does not set the field itself.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    #[serde(default)]
    pub outputs: Option<Vec<OutputSpec>>,
    /// Default `output.artifacts` staging policy for images that opt into the `output-artifacts`
    /// preview feature (`meta/docs/2026-06-29-output-artifacts-staging.md` §3.3); omitted ⇒ `managed`.
    #[serde(default, rename = "outputArtifacts")]
    pub output_artifacts: Option<OutputArtifactsPolicy>,
}

/// Which images belong to the workspace. Omitted ⇒ auto-discover `*/image.yaml` at depth 1.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageCatalogue {
    #[serde(default)]
    pub members: Option<Vec<String>>,
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub inline: Vec<ImageDefinition>,
}

// ===== image.yaml — image definition base document (reference/image-yaml.md) =====

/// `image.yaml` — one image definition.
///
/// This models the **base document**; fragment directives are layered in a later phase. The
/// `config:` value is opaque (an inline IC config mapping or a path string) — never modeled here.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImageDefinition {
    pub name: String,
    #[serde(default)]
    pub toolchain: Option<ToolchainRef>,
    /// Exclude this image from **bulk** selection — any command run with no image named
    /// (`build`, `matrix`, `slugs`, `validate`, `render`, `export`, `clean`, …). Lets an
    /// experimental config live in the tree without the pipeline (which runs those verbs on *all*
    /// images) picking it up. Build it by naming it explicitly (`tailor build <name>`).
    #[serde(default)]
    pub skip: bool,
    #[serde(default)]
    pub tools_dir: Option<ToolsDir>,
    #[serde(default)]
    pub matrix: Option<AxisValues>,
    /// Which cells of the `matrix:` product to build (`include`/`exclude` sub-cubes). Requires
    /// `matrix:`; omitted ⇒ the full product.
    #[serde(default)]
    pub selectors: Option<Selectors>,
    #[serde(default)]
    pub outputs: Option<Vec<OutputSpec>>,
    #[serde(default)]
    pub base: Option<BaseSource>,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub params: IndexMap<String, ParamValue>,
    #[serde(default)]
    pub rpm_sources: Vec<PathBuf>,
    #[serde(default)]
    pub operation: Option<Operation>,
    /// Per-image override of the `output.artifacts` staging policy
    /// (`meta/docs/2026-06-29-output-artifacts-staging.md` §3.3); omitted ⇒ the workspace default, else `managed`.
    #[serde(default)]
    pub output_artifacts: Option<OutputArtifactsPolicy>,
    /// Signing opt-in: `true` ⇒ the workspace default profile, `<id>` ⇒ that profile, omitted ⇒
    /// unsigned (`meta/docs/2026-06-29-signing.md` §4).
    #[serde(default)]
    pub signing: Option<SigningRef>,
    #[serde(default)]
    pub extra_dependencies: Vec<PathBuf>,
    /// The named inputs this image consumes (`meta/docs/2026-09-09-inter-image-dependencies.md` §2.1).
    /// Each binds a `name` to a typed source (v1: an `image` — another workspace image's output),
    /// referenced in `config:` as `${inputs.<name>}`. An `image` input also creates a dependency edge.
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
    /// Order-only build dependencies on other workspace images (names). An `image` base or input
    /// already implies an edge; `dependsOn` is for images that must build first but are referenced
    /// nowhere (`meta/docs/2026-09-09-inter-image-dependencies.md` §2.3). Contributes no fingerprint input.
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Extra Image Customizer command-line flags, appended verbatim after every flag tailor manages.
    /// For experimental/non-standard IC builds that expose options tailor does not model; a flag
    /// tailor already emits is rejected. Mergeable across fragments (concatenated, base → specific).
    #[serde(default)]
    pub extra_params: Vec<ExtraParam>,
    #[serde(default)]
    pub config: Option<Value>,
}

/// One extra Image Customizer command-line flag. `value` is optional and joined to `param` with a
/// single `=` (`{param}={value}`), so `{ param: --foo, value: bar }` becomes the argv token
/// `--foo=bar` and `{ param: --foo }` becomes the bare flag `--foo`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExtraParam {
    pub param: String,
    #[serde(default)]
    pub value: Option<String>,
}

/// One entry in an image's `inputs:` catalogue (`meta/docs/2026-09-09-inter-image-dependencies.md` §2.1):
/// a `name` bound to a typed source. v1 ships the `image` kind — another workspace image's output.
/// Resolved, per consuming cell, to the producer's paired-cell published artifact and referenced in
/// `config:` as `${inputs.<name>}`; the substituted path is content-hashed into the fingerprint.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InputSpec {
    pub name: String,
    /// The producer workspace image (the `image` input kind).
    pub image: String,
    /// Which producer output to consume (format name); optional iff the producer is single-output.
    #[serde(default)]
    pub output: Option<OutputFormat>,
    /// Structured pins for producer axes the consumer lacks (§2.4).
    #[serde(default)]
    pub cell: BTreeMap<String, String>,
}

// ===== shared types (reference/types.md) =====

/// IC `--tools-dir` selection for package-manager userspace. The tools-dir is always bound writable
/// (a per-cell disposable copy on `runtime.buildDirBase`): IC rewrites `resolv.conf` inside the tools
/// chroot for package operations, so a read-only bind cannot work for the sealed bases this targets.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolsDir {
    pub source: ToolsDirSourceRef,
}

/// How an image selects its tools-dir source: a named workspace source or inline container ref.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ToolsDirSourceRef {
    Id(String),
    Inline(ToolsDirSourceInline),
}

/// The base-image catalogue: named slots in `tailor.yaml`, each a local file plus an optional remote
/// source `tailor bases download` fills it from (`meta/docs/2026-06-29-base-image-catalogue.md` §3). Images
/// reference a slot by name with `base: { ref: <name> }`; the path lives once, here.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(transparent)]
pub struct BaseImageCatalogue(Vec<BaseImageSlot>);

impl BaseImageCatalogue {
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&BaseImageSlot> {
        self.0.iter().find(|slot| slot.name == name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &BaseImageSlot> {
        self.0.iter()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    fn validate(&self) -> Result<(), ConfigError> {
        ensure_unique_names("baseImages", self.0.iter().map(|slot| slot.name.as_str()))
    }
}

impl From<Vec<BaseImageSlot>> for BaseImageCatalogue {
    fn from(slots: Vec<BaseImageSlot>) -> Self {
        Self(slots)
    }
}

impl FromIterator<BaseImageSlot> for BaseImageCatalogue {
    fn from_iter<T: IntoIterator<Item = BaseImageSlot>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// One catalogue slot: the local `path` (build input **and** `download` output, workspace-root-relative),
/// an optional `arch` that reconciles with the referencing cell (`meta/docs/2026-06-29-arch-and-platform.md` §3),
/// and an optional remote `source` (`oci`/`azureLinux`); a sourceless slot is filled out-of-band (CI feed).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BaseImageSlot {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub arch: Option<Arch>,
    #[serde(default)]
    pub source: Option<BaseImageSource>,
}

fn ensure_unique_names<'a>(
    catalogue: &str,
    names: impl IntoIterator<Item = &'a str>,
) -> Result<(), ConfigError> {
    let mut seen = BTreeSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(ConfigError::DuplicateCatalogueName {
                catalogue: catalogue.to_owned(),
                name: name.to_owned(),
            });
        }
    }
    Ok(())
}

/// A slot's remote source — `oci: { uri }` or `azureLinux: { version, variant }`, pulled for
/// `linux/<arch>`. Untagged so the YAML reads `{ oci: … }` / `{ azureLinux: … }`, reusing the base
/// source structs.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BaseImageSource {
    Oci {
        oci: OciBase,
    },
    #[serde(rename_all = "camelCase")]
    AzureLinux {
        azure_linux: AzureLinuxBase,
    },
}

/// A base OS image source. Exactly one of four kinds, keyed by property name (a schema `oneOf`).
///
/// Modeled as an untagged enum so the YAML surface is `{ path: … }` / `{ oci: … }` /
/// `{ azureLinux: … }` / `{ ref: … }` (serde's externally-tagged form would instead require YAML `!tags`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum BaseSource {
    /// A local base image file, resolved relative to the image directory, with an optional `arch`
    /// that supplies the cell arch when no `arch` matrix axis is declared
    /// (`meta/docs/2026-06-29-arch-and-platform.md` §3).
    Path {
        path: PathBuf,
        #[serde(default)]
        arch: Option<Arch>,
    },
    /// A base image pulled from an OCI registry.
    Oci { oci: OciBase },
    /// Microsoft Container Registry sugar for an Azure Linux base.
    #[serde(rename_all = "camelCase")]
    AzureLinux { azure_linux: AzureLinuxBase },
    /// A reference (`ref`) to a named `baseImages` catalogue slot
    /// (`meta/docs/2026-06-29-base-image-catalogue.md` §4) — resolves to the slot's local file, like a `path`
    /// base, but the path lives once in `tailor.yaml`.
    Ref {
        #[serde(rename = "ref")]
        reference: String,
    },
    /// A base produced by another **workspace image** (`meta/docs/2026-09-09-inter-image-dependencies.md`).
    /// Resolves, per consumer cell, to that image's published artifact for the paired cell (§2.4),
    /// then behaves like a `path` base. Creates a build-order dependency edge.
    #[serde(rename_all = "camelCase")]
    Image {
        image: String,
        /// Which producer output to consume (format name). Optional iff the producer is single-output.
        #[serde(default)]
        output: Option<OutputFormat>,
        /// Structured pins for producer axes the consumer lacks (§2.4).
        #[serde(default)]
        cell: BTreeMap<String, String>,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OciBase {
    pub uri: String,
    #[serde(default)]
    pub platform: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AzureLinuxBase {
    pub version: String,
    pub variant: String,
}

/// One output: a format plus optional knobs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OutputSpec {
    pub format: OutputFormat,
    #[serde(default)]
    pub cosi_compression_level: Option<u8>,
    /// Post-build compression tailor applies to the artifact (e.g. `zstd` ⇒ `img.vhd.zst`). Invalid
    /// for `cosi` (already compressed), `iso` (breaks bootability), and the `pxe-*` formats.
    #[serde(default)]
    pub compression: Option<Compression>,
    #[serde(default)]
    pub name: Option<String>,
}

/// A selector value: an axis pinned to a single value or to a list of values.
///
/// Deserializes from either a YAML scalar (`arch: amd64`) or a sequence (`arch: [amd64, arm64]`).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OneOrMany {
    One(String),
    Many(Vec<String>),
}

impl OneOrMany {
    /// Does this value-set contain `value`?
    pub fn contains(&self, value: &str) -> bool {
        self.iter().any(|v| v == value)
    }

    /// Iterate the pinned value(s).
    pub fn iter(&self) -> std::slice::Iter<'_, String> {
        match self {
            OneOrMany::One(v) => std::slice::from_ref(v).iter(),
            OneOrMany::Many(vs) => vs.iter(),
        }
    }
}

impl<'a> IntoIterator for &'a OneOrMany {
    type Item = &'a String;
    type IntoIter = std::slice::Iter<'a, String>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// A **selector** (a section / sub-cube of the matrix): a partial assignment over the axes. Each
/// listed axis is pinned to a value or a list of values; **omitted axes match all** their values.
pub type Selector = IndexMap<String, OneOrMany>;

/// The `selectors:` block — which cells of the `matrix:` product to actually build.
///
/// `include` (allowlist) unions its sub-cubes into the build set (or the full product when empty);
/// `exclude` (denylist) then removes its sub-cubes. Both lists are order-independent; `exclude`
/// always wins over `include` (`meta/docs/2026-06-29-matrix-constraints.md`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Selectors {
    #[serde(default)]
    pub include: Vec<Selector>,
    #[serde(default)]
    pub exclude: Vec<Selector>,
}

/// An image's matrix axes: user-defined, order-preserving `name -> [values]`. The cartesian product
/// (in declaration order, which the cell slug depends on — `meta/docs/2026-06-22-design.md` §10) is the set of
/// candidate cells; selection logic lives separately in `selectors:`.
pub type AxisValues = IndexMap<String, Vec<String>>;

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    #[test]
    fn engine_deserializes_lowercase_tokens() {
        assert_eq!(
            serde_yaml_ng::from_str::<Engine>("docker").unwrap(),
            Engine::Docker
        );
        assert_eq!(
            serde_yaml_ng::from_str::<Engine>("podman").unwrap(),
            Engine::Podman
        );
        assert_eq!(
            serde_yaml_ng::from_str::<Engine>("auto").unwrap(),
            Engine::Auto
        );
        assert!(serde_yaml_ng::from_str::<Engine>("rkt").is_err());
    }

    #[test]
    fn engine_default_is_docker() {
        assert_eq!(Engine::default(), Engine::Docker);
        assert_eq!(Engine::Podman.as_str(), "podman");
        assert_eq!(Engine::Auto.to_string(), "auto");
    }

    #[test]
    fn runtime_parses_engine_and_host() {
        let runtime: Runtime = serde_yaml_ng::from_str(
            "engine: podman\nhost: unix:///run/user/1000/podman/podman.sock\n",
        )
        .unwrap();
        assert_eq!(runtime.engine, Some(Engine::Podman));
        assert_eq!(
            runtime.host.as_deref(),
            Some("unix:///run/user/1000/podman/podman.sock")
        );
    }

    #[test]
    fn runtime_engine_host_default_to_none() {
        let runtime: Runtime = serde_yaml_ng::from_str("privileged: true\n").unwrap();
        assert_eq!(runtime.engine, None);
        assert_eq!(runtime.host, None);
    }

    #[test]
    fn pull_policy_parses_for_toolchains_and_tools_dir_sources() {
        let tool: ToolConfig = serde_yaml_ng::from_str(
            r"
schemaVersion: 1
toolchains:
  default: ic
  entries:
    - name: ic
      container: registry.example/ic
    - name: pinned
      container: registry.example/pinned
      pull: always
    - name: local
      container: acl-imagecustomizer
      tag: local
      pull: never
toolsDirSources:
  - name: acl
    container: acl-tools
  - name: local-tools
    container: acl-tools-local
    tag: local
    pull: never
",
        )
        .unwrap();

        assert_eq!(tool.toolchains.get("ic").unwrap().pull, PullPolicy::Missing);
        assert_eq!(
            tool.toolchains.get("pinned").unwrap().pull,
            PullPolicy::Always
        );
        assert_eq!(
            tool.toolchains.get("local").unwrap().pull,
            PullPolicy::Never
        );
        assert_eq!(tool.tools_dir_sources[0].pull, PullPolicy::Missing);
        assert_eq!(tool.tools_dir_sources[1].pull, PullPolicy::Never);

        let inline: ToolsDirSourceInline =
            serde_yaml_ng::from_str("container: acl-tools-inline\ntag: local\npull: never\n")
                .unwrap();
        assert_eq!(inline.pull, PullPolicy::Never);
    }

    #[test]
    fn runtime_parses_build_dir_base() {
        let runtime: Runtime =
            serde_yaml_ng::from_str("buildDirBase: /mnt/tailor-build\n").unwrap();
        assert_eq!(
            runtime.build_dir_base.as_deref(),
            Some(Path::new("/mnt/tailor-build"))
        );
    }

    #[test]
    fn runtime_rejects_old_build_dir_field() {
        let err = serde_yaml_ng::from_str::<Runtime>("buildDir: /mnt/tailor-build\n").unwrap_err();
        assert!(
            err.to_string().contains("buildDir"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn runtime_mounts_extra_paths_parse_default_ro_and_explicit_rw() {
        let runtime: Runtime = serde_yaml_ng::from_str(indoc::indoc! {"
            mounts:
              extraPaths:
                - path: shared/scripts
                - path: /data/scratch
                  access: rw
        "})
        .unwrap();
        let mounts = runtime.mounts.unwrap();

        assert_eq!(mounts.extra_paths.len(), 2);
        assert_eq!(mounts.extra_paths[0].path, Path::new("shared/scripts"));
        assert_eq!(mounts.extra_paths[0].access, Access::Ro);
        assert_eq!(mounts.extra_paths[1].path, Path::new("/data/scratch"));
        assert_eq!(mounts.extra_paths[1].access, Access::Rw);
    }

    // ----- signing -----

    fn signing_config() -> SigningConfig {
        serde_yaml_ng::from_str(
            "default: test-ca\n\
             profiles:\n\
            \x20 test-ca:\n\
            \x20   backend: local-test-ca\n\
            \x20   publishCaCert: ./artifacts/ca_cert.pem\n\
            \x20 byo:\n\
            \x20   backend: keypair\n\
            \x20   key: ./secrets/db.key\n\
            \x20   cert: ./secrets/db.crt\n",
        )
        .unwrap()
    }

    #[test]
    fn signing_ref_parses_bool_and_string() {
        assert_eq!(
            serde_yaml_ng::from_str::<SigningRef>("true").unwrap(),
            SigningRef::Enabled(true)
        );
        assert_eq!(
            serde_yaml_ng::from_str::<SigningRef>("test-ca").unwrap(),
            SigningRef::Profile("test-ca".to_owned())
        );
    }

    #[test]
    fn resolve_signing_none_when_unsigned() {
        let config = signing_config();
        assert!(resolve_signing(None, Some(&config)).unwrap().is_none());
        assert!(
            resolve_signing(Some(&SigningRef::Enabled(false)), Some(&config))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn resolve_signing_true_uses_default_profile() {
        let config = signing_config();
        let (id, profile) = resolve_signing(Some(&SigningRef::Enabled(true)), Some(&config))
            .unwrap()
            .unwrap();
        assert_eq!(id, "test-ca");
        assert_eq!(profile.backend, SigningBackend::LocalTestCa);
    }

    #[test]
    fn resolve_signing_named_profile() {
        let config = signing_config();
        let (id, profile) =
            resolve_signing(Some(&SigningRef::Profile("byo".to_owned())), Some(&config))
                .unwrap()
                .unwrap();
        assert_eq!(id, "byo");
        assert_eq!(profile.backend, SigningBackend::Keypair);
    }

    #[test]
    fn resolve_signing_unknown_profile_errors() {
        let config = signing_config();
        let err = resolve_signing(Some(&SigningRef::Profile("nope".to_owned())), Some(&config))
            .unwrap_err();
        assert!(matches!(
            err,
            crate::ConfigError::UnknownSigningProfile { .. }
        ));
    }

    #[test]
    fn resolve_signing_without_workspace_block_errors() {
        let err = resolve_signing(Some(&SigningRef::Enabled(true)), None).unwrap_err();
        assert!(matches!(
            err,
            crate::ConfigError::SigningMisconfigured { .. }
        ));
    }

    #[test]
    fn resolve_signing_true_without_default_errors() {
        let config: SigningConfig =
            serde_yaml_ng::from_str("profiles:\n  test-ca:\n    backend: local-test-ca\n").unwrap();
        let err = resolve_signing(Some(&SigningRef::Enabled(true)), Some(&config)).unwrap_err();
        assert!(matches!(
            err,
            crate::ConfigError::SigningMisconfigured { .. }
        ));
    }

    #[test]
    fn keypair_profile_missing_cert_is_invalid() {
        let config: SigningConfig =
            serde_yaml_ng::from_str("profiles:\n  byo:\n    backend: keypair\n    key: ./db.key\n")
                .unwrap();
        let err = resolve_signing(Some(&SigningRef::Profile("byo".to_owned())), Some(&config))
            .unwrap_err();
        assert!(matches!(
            err,
            crate::ConfigError::InvalidSigningProfile { .. }
        ));
    }

    // ----- base-image catalogue -----

    #[test]
    fn parses_base_image_catalogue_with_both_source_kinds() {
        let tool: super::ToolConfig = serde_yaml_ng::from_str(indoc::indoc! {"
            schemaVersion: 1
            toolchains:
              default: ic
              entries:
                - name: ic
                  container: registry.example/ic
                  version: 1.0.0
            baseImages:
              - name: baremetal
                path: bases/baremetal.vhdx
                arch: amd64
                source:
                  azureLinux:
                    version: '3.0'
                    variant: baremetal
              - name: edge
                path: bases/edge.vhdx
                arch: arm64
                source:
                  oci:
                    uri: mcr.example/base:edge
              - name: feed
                path: bases/feed.vhdx
        "})
        .unwrap();
        let cat = tool.base_images.unwrap();
        assert_eq!(cat.len(), 3);
        assert_eq!(cat.get("baremetal").unwrap().arch, Some(super::Arch::Amd64));
        assert!(matches!(
            &cat.get("baremetal").unwrap().source,
            Some(super::BaseImageSource::AzureLinux { .. })
        ));
        assert!(matches!(
            &cat.get("edge").unwrap().source,
            Some(super::BaseImageSource::Oci { .. })
        ));
        assert!(
            cat.get("feed").unwrap().source.is_none(),
            "sourceless feed slot"
        );
    }

    #[test]
    fn toolchains_get_returns_matching_entry() {
        let toolchains: Toolchains = serde_yaml_ng::from_str(indoc::indoc! {"
            default: ic
            entries:
              - name: ic
                container: registry.example/ic
              - name: old
                container: registry.example/old
        "})
        .unwrap();
        assert_eq!(
            toolchains.get("ic").unwrap().container,
            "registry.example/ic"
        );
        assert!(toolchains.get("missing").is_none());
    }

    #[test]
    fn base_image_catalogue_get_returns_matching_slot() {
        let cat: BaseImageCatalogue = serde_yaml_ng::from_str(indoc::indoc! {"
            - name: baremetal
              path: bases/baremetal.vhdx
            - name: edge
              path: bases/edge.vhdx
        "})
        .unwrap();
        assert_eq!(
            cat.get("baremetal").unwrap().path,
            Path::new("bases/baremetal.vhdx")
        );
        assert!(cat.get("missing").is_none());
    }

    #[test]
    fn base_image_catalogue_deserializes_from_bare_list() {
        let cat: BaseImageCatalogue = serde_yaml_ng::from_str(indoc::indoc! {"
            - name: baremetal
              path: bases/baremetal.vhdx
              arch: amd64
        "})
        .unwrap();
        assert_eq!(cat.len(), 1);
        assert_eq!(cat.iter().next().unwrap().name, "baremetal");
    }

    #[test]
    fn base_image_reference_parses_as_ref_source() {
        let base: super::BaseSource = serde_yaml_ng::from_str("ref: baremetal\n").unwrap();
        assert!(matches!(base, super::BaseSource::Ref { reference } if reference == "baremetal"));
    }
}
