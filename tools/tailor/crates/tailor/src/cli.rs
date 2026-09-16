//! The clap command-line surface (`meta/docs/2026-06-22-design.md` §11).

use std::{fmt, path::PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Full version string: the Cargo version plus SemVer build metadata (commit + build date),
/// computed by `build.rs` and shared by `--version` and the `version` subcommand.
pub(crate) const VERSION: &str = env!("TAILOR_VERSION");

/// Manifest-driven front-end for the Azure Linux Image Customizer.
#[derive(Debug, Parser)]
#[command(name = "tailor", version = VERSION, about)]
pub(crate) struct Cli {
    /// Path to `tailor.yaml` (default: walk up from the current directory).
    #[arg(long, global = true)]
    pub(crate) manifest: Option<PathBuf>,

    /// Promote authority/confinement warnings to errors.
    #[arg(long, global = true)]
    pub(crate) strict: bool,

    /// Increase verbosity (repeatable).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) verbose: u8,

    /// Decrease verbosity (repeatable).
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub(crate) quiet: u8,

    /// Leading timestamp mode for status and tracing lines.
    #[arg(long, global = true, value_enum, default_value_t = TimestampMode::Elapsed)]
    pub(crate) timestamps: TimestampMode,

    /// Container engine for this invocation: `docker` (default), `podman`, or `auto`. Overrides
    /// `runtime.engine` in `tailor.yaml`.
    #[arg(long, global = true, value_enum)]
    pub(crate) engine: Option<EngineArg>,

    /// Container engine endpoint for this invocation, e.g. `unix:///run/user/1000/podman/podman.sock`
    /// or `tcp://host:2375`. Overrides `runtime.host` and `DOCKER_HOST` / `CONTAINER_HOST`.
    #[arg(long, global = true, value_name = "ENDPOINT")]
    pub(crate) host: Option<String>,

    /// Persist each cell's full IC debug log to `<PATH>/<slug>.log` (off by default). Highest
    /// precedence of the three opt-ins, above `TAILOR_LOG_DIR` and `runtime.logDir`.
    #[arg(long, global = true, value_name = "PATH")]
    pub(crate) log_dir: Option<PathBuf>,

    /// Set IC's own `--log-level` (default `debug`), independent of `-v`/`-q`. Overrides
    /// `runtime.logLevel`.
    #[arg(long, global = true, value_enum, value_name = "LEVEL")]
    pub(crate) ic_log_level: Option<IcLogLevel>,

    #[command(subcommand)]
    pub(crate) command: Command,
}

/// Leading timestamp mode for status and tracing lines.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub(crate) enum TimestampMode {
    /// Seconds elapsed since tailor started.
    #[default]
    Elapsed,
    /// Compact wall-clock time (`HH:MM:SS`).
    Time,
    /// Suppress live timestamps.
    Off,
}

impl fmt::Display for TimestampMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Elapsed => "elapsed",
            Self::Time => "time",
            Self::Off => "off",
        })
    }
}

/// IC log levels for `--ic-log-level` (mirrors [`tailor_config::LogLevel`]).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum IcLogLevel {
    Panic,
    Fatal,
    Error,
    Warn,
    Info,
    Debug,
    Trace,
}

impl From<IcLogLevel> for tailor_config::LogLevel {
    fn from(value: IcLogLevel) -> Self {
        match value {
            IcLogLevel::Panic => tailor_config::LogLevel::Panic,
            IcLogLevel::Fatal => tailor_config::LogLevel::Fatal,
            IcLogLevel::Error => tailor_config::LogLevel::Error,
            IcLogLevel::Warn => tailor_config::LogLevel::Warn,
            IcLogLevel::Info => tailor_config::LogLevel::Info,
            IcLogLevel::Debug => tailor_config::LogLevel::Debug,
            IcLogLevel::Trace => tailor_config::LogLevel::Trace,
        }
    }
}

/// The container engine selector for `--engine` (mirrors [`tailor_config::Engine`]).
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum EngineArg {
    /// The Docker daemon (default).
    Docker,
    /// Podman via its Docker-compatible API.
    Podman,
    /// Probe known Docker/Podman sockets and use the first that answers.
    Auto,
}

impl From<EngineArg> for tailor_config::Engine {
    fn from(value: EngineArg) -> Self {
        match value {
            EngineArg::Docker => tailor_config::Engine::Docker,
            EngineArg::Podman => tailor_config::Engine::Podman,
            EngineArg::Auto => tailor_config::Engine::Auto,
        }
    }
}

/// The verb to run.
#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Resolve and run Image Customizer for the given images (default: all).
    Build(BuildArgs),
    /// Convert a single image file to another format via Image Customizer (`convert`), with no
    /// workspace or config required — a thin wrapper over `imagecustomizer convert`.
    Convert(ConvertArgs),
    /// Render the final IC config per cell, writing golden snapshots.
    Render(ImagesArgs),
    /// Export committed IC config YAMLs for a non-tailor pipeline; `--check` fails on drift.
    Export(ExportArgs),
    /// List images (and toolchains).
    List,
    /// Show resolved information for one image.
    Show {
        image: String,
        field: Option<String>,
    },
    /// Remove generated artifacts and build stamps (sudo-free via the janitor).
    Clean(ImagesArgs),
    /// Resolve digests/hashes without building.
    Resolve(ImagesArgs),
    /// Freeze `tailor.lock`: pin any new inputs, keeping already-locked digests unchanged.
    Lock,
    /// Re-resolve every input to its latest digest and rewrite `tailor.lock`.
    Update,
    /// Validate image definitions (renders every cell) without building.
    Validate(ImagesArgs),
    /// Show the merge order (the ordered fragment files) for a cell; `--with-config` also prints the
    /// merged IC config.
    Explain {
        image: String,
        #[command(flatten)]
        select: SelectArgs,
        /// Also print the merged Image Customizer config after the merge order.
        #[arg(long)]
        with_config: bool,
    },
    /// Emit the build matrix (all viable cells) for the selected images.
    Matrix(MatrixArgs),
    /// Print one cell slug per line for the selected images (same as `matrix --format slugs`).
    Slugs(ImagesArgs),
    /// Print version information (identical to `--version`).
    Version,
    /// Print tailor's license and the third-party software notices for its dependencies.
    Notice,
    /// Scaffold a new tailor project (manifest and/or image definition).
    Init(InitArgs),
    /// Add an image or axis to an existing tailor project.
    Add {
        #[command(subcommand)]
        what: AddCommand,
    },
    /// Manage base-image catalogue slots: pull them from their source, or check they are present.
    Bases {
        #[command(subcommand)]
        what: BasesCommand,
    },
}

/// Subcommands of `tailor bases`.
#[derive(Debug, Subcommand)]
pub(crate) enum BasesCommand {
    /// List catalogue slots: each slot's arch, source, on-disk presence, and path.
    List,
    /// Download catalogue slots from their `source` (default: every sourced slot whose file is
    /// missing). Naming a sourceless slot is an error; `--force` re-pulls present files.
    Download {
        /// Slot names to download (default: all sourced slots).
        names: Vec<String>,
        /// Re-download even when the slot file already exists.
        #[arg(long)]
        force: bool,
    },
    /// Verify catalogue slot files exist on disk, failing with the missing names/paths.
    Verify {
        /// Slot names to check (default: all slots).
        names: Vec<String>,
    },
}

/// Subcommands of `tailor add`.
#[derive(Debug, Subcommand)]
pub(crate) enum AddCommand {
    /// Scaffold a new member image and register it in the workspace `tailor.yaml`.
    Image {
        /// The new image's name (also its directory, created in the current directory).
        name: String,
    },
    /// Append a new axis to an image's matrix and create its `by-<axis>/` directory.
    ///
    /// Pass just `<axis>` when the workspace has one image, or `<image> <axis>` to choose one.
    Axis {
        /// The axis name, or the image name when a second argument is given.
        first: String,
        /// The axis name (when the first argument is the image name).
        second: Option<String>,
    },
}

/// Args for `tailor init`.
#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// The image name (also the member directory for the `base`/`advanced` templates).
    pub(crate) name: String,

    /// Which scaffold to generate (default: `base`).
    #[arg(value_enum, default_value_t = InitTemplate::Base)]
    pub(crate) template: InitTemplate,
}

/// Scaffold template for `tailor init`.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub(crate) enum InitTemplate {
    /// A workspace `tailor.yaml` plus a basic `<name>/image.yaml`.
    #[default]
    Base,
    /// A single standalone `image.yaml` in the current directory (no `tailor.yaml`).
    Simple,
    /// Like `base`, but the image declares two example axes with `by-*` fragments.
    Advanced,
}

/// Args for verbs that operate on a set of images and (optionally) a cell selection.
#[derive(Debug, Args)]
pub(crate) struct ImagesArgs {
    /// Image names to operate on (default: all in the workspace).
    pub(crate) images: Vec<String>,

    #[command(flatten)]
    pub(crate) select: SelectArgs,
}

/// Args for `tailor export`. With an `export:` block in `tailor.yaml`, all fields are optional, so
/// `tailor export` and `tailor export --check` run with no arguments (ideal for a pre-commit/CI gate).
#[derive(Debug, Args)]
pub(crate) struct ExportArgs {
    /// Image names to export (default: the `export.images` config, else all images).
    pub(crate) images: Vec<String>,

    /// Verify the committed exports match freshly rendered configs; exit non-zero on drift. No writes.
    #[arg(long)]
    pub(crate) check: bool,

    /// Output directory (default: `export.outputDir` from `tailor.yaml`); relative to the workspace root.
    #[arg(long, value_name = "DIR")]
    pub(crate) output_dir: Option<PathBuf>,

    #[command(flatten)]
    pub(crate) select: SelectArgs,
}

/// Reusable cell-selection flags. Pin some axes for a slice, all axes for one
/// cell, or name exact cells by slug. Axis values are `[A-Za-z0-9.-]+`, so `,` and `=` are safe
/// delimiters.
#[derive(Debug, Args)]
pub(crate) struct SelectArgs {
    /// Constrain matrix axes, e.g. `-s variant=grub,arch=amd64` (repeatable). Unset axes expand
    /// fully, so `-s arch=amd64` builds every amd64 cell.
    #[arg(short, long = "select", value_name = "AXIS=VALUE")]
    pub(crate) select: Vec<String>,

    /// Select exact cells by slug (repeatable); matches the `slug` field of `tailor matrix` output.
    #[arg(long = "cell", value_name = "SLUG")]
    pub(crate) cell: Vec<String>,
}

/// Output format for `tailor matrix`.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum MatrixFormat {
    /// A JSON array of cell objects (`image`, `slug`, `axes`, `format`).
    Json,
    /// One cell slug per line — feeds `tailor build --cell <slug>` directly.
    Slugs,
    /// The bare Azure DevOps matrix object (`{ leg: { var: string, … } }`); see `--ado` to wrap it.
    Ado,
}

/// Args for `tailor matrix`.
#[derive(Debug, Args)]
pub(crate) struct MatrixArgs {
    /// Image names to enumerate (default: all in the workspace).
    pub(crate) images: Vec<String>,

    #[command(flatten)]
    pub(crate) select: SelectArgs,

    /// How to print the matrix.
    #[arg(long, value_enum, default_value_t = MatrixFormat::Json)]
    pub(crate) format: MatrixFormat,

    /// Emit the ADO matrix as a `task.setvariable` line setting <VAR_NAME> (e.g. `BUILD_MATRIX`):
    /// `--format ado` plus the logging-command wrapper. Empty selection fails non-zero.
    #[arg(long, value_name = "VAR_NAME", conflicts_with = "format")]
    pub(crate) ado: Option<String>,
}

/// Args for `tailor build`.
#[derive(Debug, Args)]
pub(crate) struct BuildArgs {
    /// Image names to build (default: all).
    pub(crate) images: Vec<String>,

    #[command(flatten)]
    pub(crate) select: SelectArgs,

    /// Require a complete `tailor.lock`; fail on a missing entry or drift.
    #[arg(long)]
    pub(crate) locked: bool,

    /// Ignore incremental up-to-date checks.
    #[arg(long)]
    pub(crate) force: bool,

    /// Restrict the build to the given architecture(s).
    #[arg(long)]
    pub(crate) arch: Vec<String>,

    /// Where to write artifacts (default: `<workspace>/artifacts`).
    #[arg(long)]
    pub(crate) output_dir: Option<PathBuf>,

    /// Override `runtime.buildDirBase`: place each cell's build scratch under this directory (which
    /// must not be `/` or on the same filesystem as `/`). Lets CI point scratch at a pool-specific
    /// filesystem without editing the committed `tailor.yaml`.
    #[arg(long, value_name = "PATH")]
    pub(crate) build_dir_base: Option<PathBuf>,

    /// Render every selected cell's container invocation without running it.
    #[arg(long)]
    pub(crate) dry_run: bool,

    /// Max parallel matrix cells (reserved for a future release; builds are currently sequential).
    #[arg(long, hide = true)]
    pub(crate) jobs: Option<usize>,

    /// Build N clones of each cell: distinct artifacts that share all meaningful content but differ
    /// in incidental details (fresh UUIDs, timestamps). Each is published as `<slug>_clone<n>`.
    #[arg(long, default_value_t = 1)]
    pub(crate) clones: u32,
}

/// Args for `tailor convert` — a one-shot, workspace-free `imagecustomizer convert`.
#[derive(Debug, Args)]
pub(crate) struct ConvertArgs {
    /// The input image file to convert (a local path).
    pub(crate) input: PathBuf,

    /// The target output format. One of the convert-supported formats: `vhd`, `vhd-fixed`, `vhdx`,
    /// `qcow2`, `raw`, `cosi`, `baremetal-image`.
    #[arg(long, value_name = "FORMAT", value_parser = parse_convert_format)]
    pub(crate) to: tailor_config::OutputFormat,

    /// Where to write the converted image (default: the input's name with the target extension,
    /// beside the input).
    #[arg(short, long, value_name = "PATH")]
    pub(crate) output: Option<PathBuf>,

    /// The Image Customizer container to run (default: `mcr.microsoft.com/azurelinux/imagecustomizer:latest`).
    #[arg(long, value_name = "REF")]
    pub(crate) container: Option<String>,

    /// The image architecture, driving `--platform linux/<arch>` (default: `amd64`).
    #[arg(long, value_name = "ARCH")]
    pub(crate) arch: Option<String>,

    /// Host filesystem base for IC's per-run scratch (must not be `/` or on the same filesystem as
    /// `/`). Default: a unique directory under the system temp dir.
    #[arg(long, value_name = "PATH")]
    pub(crate) build_dir_base: Option<PathBuf>,

    /// Render the container invocation without running it.
    #[arg(long)]
    pub(crate) dry_run: bool,
}

/// Parse and restrict `--to` to the formats IC `convert` supports (it takes an already-built image,
/// so the OS-build-only formats `iso`/`pxe-*` are excluded).
fn parse_convert_format(value: &str) -> Result<tailor_config::OutputFormat, String> {
    use tailor_config::OutputFormat;
    let format = match value {
        "vhd" => OutputFormat::Vhd,
        "vhd-fixed" => OutputFormat::VhdFixed,
        "vhdx" => OutputFormat::Vhdx,
        "qcow2" => OutputFormat::Qcow2,
        "raw" => OutputFormat::Raw,
        "cosi" => OutputFormat::Cosi,
        "baremetal-image" => OutputFormat::BaremetalImage,
        other => {
            return Err(format!(
                "`{other}` is not a convert-supported format (use one of: vhd, vhd-fixed, vhdx, \
                 qcow2, raw, cosi, baremetal-image)"
            ));
        }
    };
    Ok(format)
}
