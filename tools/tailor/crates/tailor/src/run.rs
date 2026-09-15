//! Verb dispatch and the composition root — wires concrete adapters into the core orchestrator
//! and implements each CLI verb (`meta/docs/2026-06-22-design.md` §11).

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, btree_map::Entry},
    env, fs,
    future::Future,
    io,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use indexmap::IndexMap;
use serde::Serialize;
use tailor_config::{
    Arch, BaseImageCatalogue, BaseSource, ImageDefinition, Operation, OutputArtifactsPolicy,
    OutputFormat, OutputSpec, PreviewFeature, PullPolicy, RenderedCell, Runtime, ToolConfig,
    ToolchainEntry, ToolsDirSourceInline, Workspace, discover, expand, find_manifest, merge_plan,
    render_image, write_golden,
};
use tailor_core::{
    BaseResolver, BuildOptions, BuildProgress, BuildSelection, Cell, CellSlug, ContainerRuntime,
    CoreError, ExecutionContext, Executor, LocalImage, LockedBase, LockedContainer, LockedRuntime,
    Lockfile, MissingPrerequisite, Orchestrator, ResolveError, ResolvedBase, ResolvedToolchain,
    ResolvedToolsDirSource, Selector, SignError, Signer, SigningRequirement, SlotSource,
    SlotSummary, Target, ado_matrix, cells, cells_selected, download, imagedep, is_valid_var_name,
    runtime_config, select_node_cells, summarize, toolchain_for, toolchain_key, tools_dir_key,
    verify,
};
use tailor_exec::{
    BollardRuntime, IcExecutor, NoopRuntime, ResolveInputs, WorktreeLock, ca_cert_name, resolve,
};
use tailor_resolve::{OciFetcher, OciResolver};
use tokio_util::sync::CancellationToken;

use crate::{
    cli::{
        AddCommand, BasesCommand, BuildArgs, Cli, Command, ConvertArgs, ExportArgs, InitArgs,
        InitTemplate, MatrixFormat, TimestampMode,
    },
    error::AppError,
    scaffold,
};

const LOCK_FILE: &str = "tailor.lock";
const ARTIFACTS_DIR: &str = "artifacts";
const TAILOR_STATE_DIR: &str = ".tailor";
const BASE_HASH_CACHE_DIR: &str = "base-hashes";
/// Default per-cell build/scratch directory under the output state dir, used when
/// `runtime.buildDirBase` is unset.
const BUILD_DIR: &str = "build";
const STATUS_VERB_WIDTH: usize = 12;
const ELAPSED_TIMESTAMP_PRECISION: usize = 2;
const SECS_PER_MINUTE: u64 = 60;
const SECS_PER_HOUR: u64 = 60 * SECS_PER_MINUTE;
const SECS_PER_DAY: u64 = 24 * SECS_PER_HOUR;

static STARTED: OnceLock<Instant> = OnceLock::new();
static TIMESTAMP_MODE: OnceLock<TimestampMode> = OnceLock::new();

/// A per-invocation engine/endpoint override from the global `--engine` / `--host` flags
/// (`meta/docs/2026-06-29-container-runtimes.md` §3). Both sit above the manifest in the precedence ladder.
#[derive(Clone, Default)]
pub(crate) struct EngineOverride {
    engine: Option<tailor_config::Engine>,
    host: Option<String>,
}

/// Per-invocation logging overrides from the global `--log-dir` / `--ic-log-level` flags
/// (`meta/docs/2026-06-29-logging.md` §5.1, §5.5).
#[derive(Clone, Default)]
pub(crate) struct LogOverrides {
    log_dir: Option<PathBuf>,
    ic_log_level: Option<tailor_config::LogLevel>,
}

/// The `TAILOR_LOG_DIR` environment variable: the CI/pipeline path to opt into on-disk IC logs.
const LOG_DIR_ENV: &str = "TAILOR_LOG_DIR";

/// Resolve the opt-in log directory by precedence (highest first): `--log-dir` flag, `TAILOR_LOG_DIR`
/// env, then `runtime.logDir` from the manifest (`meta/docs/2026-06-29-logging.md` §5.5). `None` keeps persistence
/// off (the default), so nothing is written to disk.
fn resolve_log_dir(
    flag: Option<PathBuf>,
    env: Option<PathBuf>,
    manifest: Option<PathBuf>,
) -> Option<PathBuf> {
    flag.or(env).or(manifest)
}

/// Read `TAILOR_LOG_DIR`, treating an empty value as unset.
fn log_dir_from_env() -> Option<PathBuf> {
    env::var_os(LOG_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Load the workspace, then dispatch the requested verb.
pub(crate) async fn dispatch(cli: Cli) -> Result<(), AppError> {
    init_timestamps(cli.timestamps);
    // `version` and `init` need no workspace — handle them before discovery so they work anywhere
    // (`init` is what creates a workspace in the first place).
    match &cli.command {
        Command::Version => {
            print_version();
            return Ok(());
        }
        Command::Notice => {
            print_notice();
            return Ok(());
        }
        Command::Init(args) => return init(args),
        Command::Add { what } => return add(what),
        Command::Convert(args) => {
            let engine = EngineOverride {
                engine: cli.engine.map(Into::into),
                host: cli.host.clone(),
            };
            return convert(args, &engine).await;
        }
        _ => {}
    }
    let workspace = load_workspace(&cli)?;
    let engine = EngineOverride {
        engine: cli.engine.map(Into::into),
        host: cli.host.clone(),
    };
    let logging = LogOverrides {
        log_dir: cli.log_dir.clone(),
        ic_log_level: cli.ic_log_level.map(Into::into),
    };
    match cli.command {
        Command::Version
        | Command::Notice
        | Command::Init(_)
        | Command::Add { .. }
        | Command::Convert(_) => {
            unreachable!("handled before workspace discovery")
        }
        Command::List => {
            list(&workspace);
            Ok(())
        }
        Command::Show { image, field } => show(&workspace, &image, field.as_deref()),
        Command::Validate(args) => {
            validate(&workspace, &args.images, &selector(&args.select, &[])?)
        }
        Command::Render(args) => render(&workspace, &args.images, &selector(&args.select, &[])?),
        Command::Export(args) => export(&workspace, &args),
        Command::Explain {
            image,
            select,
            with_config,
        } => explain(&workspace, &image, &selector(&select, &[])?, with_config),
        Command::Matrix(args) => matrix(
            &workspace,
            &args.images,
            &selector(&args.select, &[])?,
            args.format,
            args.ado.as_deref(),
        ),
        Command::Slugs(args) => matrix(
            &workspace,
            &args.images,
            &selector(&args.select, &[])?,
            MatrixFormat::Slugs,
            None,
        ),
        Command::Resolve(args) => resolve_verb(&workspace, &args.images, &engine).await,
        Command::Lock => lock(&workspace, &engine, false).await,
        Command::Update => lock(&workspace, &engine, true).await,
        Command::Build(args) => build(&workspace, args, &engine, &logging).await,
        Command::Clean(args) => {
            clean(
                &workspace,
                &args.images,
                &selector(&args.select, &[])?,
                &engine,
            )
            .await
        }
        Command::Bases { what } => bases(&workspace, &what).await,
    }
}

/// Build a [`Selector`] from the shared `-s/--cell` flags plus any verb-specific `--arch` values.
fn selector(args: &crate::cli::SelectArgs, arches: &[String]) -> Result<Selector, AppError> {
    Ok(Selector::parse(&args.select, &args.cell, arches)?)
}

/// Print the version exactly as `--version` does — clap renders both from the same source.
fn print_version() {
    use clap::CommandFactory;
    print!("{}", Cli::command().render_version());
}

/// Print tailor's own MIT license followed by the third-party notices for every dependency linked
/// into the binary. The third-party text is generated at build time (`build.rs`) from the crate
/// sources and embedded via `OUT_DIR`, so it always matches the built dependency set.
fn print_notice() {
    const TAILOR_LICENSE: &str =
        include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../LICENSE"));
    const THIRD_PARTY_NOTICES: &str =
        include_str!(concat!(env!("OUT_DIR"), "/third-party-notices.txt"));

    println!("tailor is distributed under the MIT License:\n");
    print!("{TAILOR_LICENSE}");
    if !TAILOR_LICENSE.ends_with('\n') {
        println!();
    }
    println!();
    print!("{THIRD_PARTY_NOTICES}");
}

fn load_workspace(cli: &Cli) -> Result<Workspace, AppError> {
    let cwd = env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;
    let start = match &cli.manifest {
        Some(path) if path.is_file() => path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
        Some(path) => path.clone(),
        None => cwd.clone(),
    };
    // Resolve to an absolute root so every derived path (image dirs, base, cache, outputs) is absolute
    // — relative paths break Docker bind mounts and the `/host` translation.
    Ok(discover(tailor_config::absolutize(start, &cwd))?)
}

// ───────────────────────────── pure verbs (no Docker / network) ─────────────────────────────

fn list(workspace: &Workspace) {
    println!("Images:");
    for image in &workspace.images {
        let cells = image.definition.matrix.as_ref().map_or(1, |m| {
            expand(m, image.definition.selectors.as_ref()).map_or(0, |c| c.len())
        });
        let marker = if image.definition.skip {
            "  (skip)"
        } else {
            ""
        };
        println!("  {:<28} {cells} cell(s){marker}", image.definition.name);
    }
    if let Some(tool) = &workspace.tool {
        println!("\nToolchains (default: {}):", tool.toolchains.default);
        for entry in &tool.toolchains.entries {
            println!(
                "  {:<10} {}:{}",
                entry.name,
                entry.container,
                entry.effective_tag()
            );
        }
    }
}

fn show(workspace: &Workspace, name: &str, field: Option<&str>) -> Result<(), AppError> {
    let image = workspace
        .image(name)
        .ok_or_else(|| AppError::Message(format!("unknown image `{name}`")))?;
    let definition = &image.definition;
    if let Some(field) = field {
        let value = match field {
            "name" => definition.name.clone(),
            "dir" => image.dir.display().to_string(),
            "outputs" => format!("{:?}", definition.outputs),
            "features" => definition.features.join(", "),
            other => return Err(AppError::Message(format!("unknown field `{other}`"))),
        };
        println!("{value}");
        return Ok(());
    }
    println!("name:    {}", definition.name);
    println!("dir:     {}", image.dir.display());
    if let Some(axes) = &definition.matrix {
        let cells = expand(axes, definition.selectors.as_ref()).map_or(0, |c| c.len());
        println!("matrix:  {cells} cell(s) across {} axis(es)", axes.len());
        let width = axes.keys().map(String::len).max().unwrap_or(0);
        for (axis, values) in axes {
            println!("  {axis:<width$} : {}", values.join(", "));
        }
        if let Some(selectors) = &definition.selectors {
            if !selectors.include.is_empty() {
                println!("  ({} include selector(s))", selectors.include.len());
            }
            if !selectors.exclude.is_empty() {
                println!("  ({} exclude selector(s))", selectors.exclude.len());
            }
        }
    } else {
        println!("matrix:  none (single cell)");
    }
    let outputs: Vec<&str> = definition
        .outputs
        .iter()
        .flatten()
        .map(|o| o.format.as_str())
        .collect();
    if !outputs.is_empty() {
        println!("outputs: {}", outputs.join(", "));
    }
    if !definition.features.is_empty() {
        println!("features: {}", definition.features.join(", "));
    }
    Ok(())
}

fn validate(workspace: &Workspace, names: &[String], selector: &Selector) -> Result<(), AppError> {
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, names)?;
    let all_members = all_member_targets(workspace)?;
    // Inter-image dependencies: catch unknown-image edges and cycles, then resolve each cell's
    // `base: { image }` so pairing/output/pin mismatches surface offline (no artifact is read — the
    // path is only constructed). `output_dir` is the default artifacts dir, used solely for that path.
    let closure = imagedep::dependency_closure(&targets, &all_members)?;
    imagedep::topological_order(&closure, &all_members)?;
    let output_dir = workspace.root.join(ARTIFACTS_DIR);
    for target in &targets {
        let mut cells = cells_selected(target, selector)?;
        imagedep::lower_image_bases(&mut cells, &all_members, &output_dir)?;
        imagedep::resolve_inputs(&mut cells, &all_members, &output_dir)?;
        println!("✓ {:<28} {} cell(s) valid", target.name(), cells.len());
    }
    // Surface signing prerequisites non-fatally (meta/docs/2026-06-29-signing.md §5.1) so they are discoverable
    // without starting a real build.
    let signing = signing_requirements(&targets, tool.signing.as_ref())?;
    ensure_signing_preview(&tool, &signing)?;
    report_signing(&build_signers(&signing, &workspace.root));
    Ok(())
}

fn render(workspace: &Workspace, names: &[String], selector: &Selector) -> Result<(), AppError> {
    let all_members = all_member_targets(workspace)?;
    // Resolve `${inputs.*}` so the rendered golden config carries concrete producer paths (a
    // downstream pipeline consuming the export needs real paths, not tokens).
    let output_dir = workspace.root.join(ARTIFACTS_DIR);
    for target in build_targets(workspace, names)? {
        let mut cells = cells_selected(&target, selector)?;
        imagedep::lower_image_bases(&mut cells, &all_members, &output_dir)?;
        imagedep::resolve_inputs(&mut cells, &all_members, &output_dir)?;
        for cell in cells {
            let path = write_golden(&target.dir, cell.slug.as_ref(), &cell.ic_config)?;
            println!("rendered {}", path.display());
        }
    }
    Ok(())
}

const EXPORT_YAML_EXT: &str = "yaml";

/// `tailor export` — render every selected cell's merged IC config to `<outputDir>/<slug>.yaml` for a
/// non-tailor pipeline to consume, or (`--check`) verify the committed exports are up to date.
/// Offline and pure: no base/toolchain resolution, no Docker (`meta/docs/2026-07-20-export-configs-only.md`).
fn export(workspace: &Workspace, args: &ExportArgs) -> Result<(), AppError> {
    let config = workspace
        .tool
        .as_ref()
        .and_then(|tool| tool.export.as_ref());
    let output_dir = args
        .output_dir
        .clone()
        .or_else(|| config.map(|export| export.output_dir.clone()))
        .ok_or_else(|| {
            AppError::Message(
                "no export output directory: set `export.outputDir` in tailor.yaml or pass --output-dir"
                    .to_owned(),
            )
        })?;
    let output_dir = tailor_config::absolutize(&output_dir, &workspace.root);

    let names = if args.images.is_empty() {
        config.map_or_else(Vec::new, |export| export.images.clone())
    } else {
        args.images.clone()
    };
    let selector = selector(&args.select, &[])?;

    let mut produced: BTreeMap<String, String> = BTreeMap::new();
    for target in build_targets(workspace, &names)? {
        for cell in cells_selected(&target, &selector)? {
            let yaml = serde_yaml_ng::to_string(&cell.ic_config).map_err(|source| {
                AppError::Message(format!(
                    "failed to serialize IC config for cell `{}`: {source}",
                    cell.slug.as_ref()
                ))
            })?;
            produced.insert(cell.slug.as_ref().to_owned(), yaml);
        }
    }
    if produced.is_empty() {
        return Err(AppError::Message(
            "export matched no cells for the selected images".to_owned(),
        ));
    }

    if args.check {
        check_export(&output_dir, &produced)
    } else {
        write_export(&output_dir, &produced)
    }
}

/// Write each `<slug>.yaml` and prune any top-level `*.yaml` in the output dir that no selected cell
/// produces, so a removed cell/axis never leaves an orphaned committed file.
fn write_export(output_dir: &Path, produced: &BTreeMap<String, String>) -> Result<(), AppError> {
    fs::create_dir_all(output_dir).map_err(|source| {
        AppError::Message(format!(
            "failed to create export dir {}: {source}",
            output_dir.display()
        ))
    })?;
    for (slug, yaml) in produced {
        let path = output_dir.join(format!("{slug}.{EXPORT_YAML_EXT}"));
        fs::write(&path, yaml).map_err(|source| {
            AppError::Message(format!("failed to write {}: {source}", path.display()))
        })?;
        println!("exported {}", path.display());
    }
    for path in stale_exports(output_dir, produced)? {
        fs::remove_file(&path).map_err(|source| {
            AppError::Message(format!("failed to prune {}: {source}", path.display()))
        })?;
        println!("pruned {}", path.display());
    }
    Ok(())
}

/// Verify the committed exports match `produced`: fail on any changed, missing, or extra (stale)
/// `<slug>.yaml`. Renders nothing to disk.
fn check_export(output_dir: &Path, produced: &BTreeMap<String, String>) -> Result<(), AppError> {
    let mut drift: Vec<String> = Vec::new();
    for (slug, yaml) in produced {
        let path = output_dir.join(format!("{slug}.{EXPORT_YAML_EXT}"));
        match fs::read_to_string(&path) {
            Ok(existing) if &existing == yaml => {}
            Ok(_) => drift.push(format!("changed: {}", path.display())),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {
                drift.push(format!("missing: {}", path.display()));
            }
            Err(source) => {
                return Err(AppError::Message(format!(
                    "failed to read {}: {source}",
                    path.display()
                )));
            }
        }
    }
    for path in stale_exports(output_dir, produced)? {
        drift.push(format!("extra:   {}", path.display()));
    }
    if drift.is_empty() {
        println!("export up to date ({} cell(s))", produced.len());
        return Ok(());
    }
    drift.sort();
    Err(AppError::Message(format!(
        "export is out of date ({} difference(s)); run `tailor export`:\n  {}",
        drift.len(),
        drift.join("\n  ")
    )))
}

/// Top-level `*.yaml` files under `output_dir` whose slug is not in `produced` (orphaned exports).
fn stale_exports(
    output_dir: &Path,
    produced: &BTreeMap<String, String>,
) -> Result<Vec<PathBuf>, AppError> {
    let entries = match fs::read_dir(output_dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(AppError::Message(format!(
                "failed to read export dir {}: {source}",
                output_dir.display()
            )));
        }
    };
    let mut stale = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|source| {
                AppError::Message(format!(
                    "failed to read export dir {}: {source}",
                    output_dir.display()
                ))
            })?
            .path();
        if path.extension().and_then(|ext| ext.to_str()) != Some(EXPORT_YAML_EXT) {
            continue;
        }
        let is_produced = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .is_some_and(|stem| produced.contains_key(stem));
        if !is_produced {
            stale.push(path);
        }
    }
    stale.sort();
    Ok(stale)
}

/// Print the merge order (the ordered fragment files) for each selected cell, so the precedence model
/// is legible. With `with_config`, also print the merged IC config. Read-only and offline.
fn explain(
    workspace: &Workspace,
    name: &str,
    selector: &Selector,
    with_config: bool,
) -> Result<(), AppError> {
    let targets = build_targets(workspace, std::slice::from_ref(&name.to_owned()))?;
    let target = targets
        .first()
        .ok_or_else(|| AppError::Message(format!("unknown image `{name}`")))?;
    let cells = cells_selected(target, selector)?;

    // One merge plan per axis-cell (outputs don't change fragment selection), in first-seen order.
    let mut seen: BTreeSet<BTreeMap<String, String>> = BTreeSet::new();
    for cell in &cells {
        if !seen.insert(cell.axes.clone()) {
            continue;
        }
        let coordinate: Vec<String> = cell
            .axes
            .iter()
            .map(|(axis, value)| format!("{axis}={value}"))
            .collect();
        println!("cell  {}   ({})", cell.slug.as_ref(), coordinate.join(", "));
        println!("\nmerge order (top = base, bottom wins):");
        let plan = merge_plan(&cell.target.definition, &cell.target.dir, &cell.axes)?;
        let width = plan.iter().map(|s| s.label.len()).max().unwrap_or(0);
        for (i, step) in plan.iter().enumerate() {
            println!(
                "  {:>2}  {:<width$}  {}",
                i + 1,
                step.label,
                step.reason,
                width = width
            );
            for include in &step.includes {
                println!("        └─ $include {include}");
            }
        }
        if with_config {
            let yaml = serde_yaml_ng::to_string(&cell.ic_config)
                .map_err(|e| AppError::Message(format!("serialize: {e}")))?;
            println!("\nmerged config:\n{yaml}");
        }
        println!();
    }
    Ok(())
}

#[derive(Serialize)]
struct MatrixEntry {
    image: String,
    slug: String,
    axes: BTreeMap<String, String>,
    format: String,
    #[serde(rename = "baseImage", skip_serializing_if = "Option::is_none")]
    base_image: Option<String>,
}

/// The ADO logging command that publishes a cross-stage output variable (`meta/docs/2026-06-29-ado-matrix.md` §3):
/// `##vso[task.setvariable variable=<NAME>;isOutput=true]<compact-json>`.
const ADO_SETVAR_PREFIX: &str = "##vso[task.setvariable variable=";
const ADO_SETVAR_SUFFIX: &str = ";isOutput=true]";

fn matrix(
    workspace: &Workspace,
    names: &[String],
    selector: &Selector,
    format: MatrixFormat,
    ado_var: Option<&str>,
) -> Result<(), AppError> {
    // `--ado <VAR>` is `--format ado` plus the setvariable wrapper.
    let format = if ado_var.is_some() {
        MatrixFormat::Ado
    } else {
        format
    };
    let mut cells = Vec::new();
    for target in build_targets(workspace, names)? {
        match cells_selected(&target, selector) {
            Ok(mut selected) => cells.append(&mut selected),
            // ADO tolerates an empty slice: `--format ado` prints `{}`, `--ado` fails clearly below.
            Err(CoreError::NoCellsSelected { .. }) if matches!(format, MatrixFormat::Ado) => {}
            Err(e) => return Err(e.into()),
        }
    }
    match format {
        MatrixFormat::Json => {
            let entries: Vec<MatrixEntry> = cells
                .iter()
                .map(|cell| MatrixEntry {
                    image: cell.target.name().to_owned(),
                    slug: cell.slug.to_string(),
                    axes: cell.axes.clone(),
                    format: cell.output.format.as_str().to_owned(),
                    base_image: cell.base_image.clone(),
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&entries)?);
        }
        MatrixFormat::Slugs => {
            for cell in &cells {
                println!("{}", cell.slug);
            }
        }
        MatrixFormat::Ado => {
            let matrix = ado_matrix(&cells);
            let json = serde_json::to_string(&matrix)?;
            match ado_var {
                Some(var) => {
                    if !is_valid_var_name(var) {
                        return Err(AppError::Message(format!(
                            "invalid --ado variable name `{var}` (expected [A-Za-z_][A-Za-z0-9_]*)"
                        )));
                    }
                    if cells.is_empty() {
                        return Err(AppError::Message(
                            "--ado selection matched no cells; ADO cannot expand an empty matrix"
                                .to_owned(),
                        ));
                    }
                    println!("{ADO_SETVAR_PREFIX}{var}{ADO_SETVAR_SUFFIX}{json}");
                    eprint_ado_legs(var, &matrix);
                }
                None => println!("{json}"),
            }
        }
    }
    Ok(())
}

/// `--ado <VAR>` emits only the `##vso[task.setvariable …]` line, which the ADO agent consumes and
/// hides from the build log. Echo a human-readable leg roster to stderr (which the log *does* show)
/// so an operator can see which legs were published and map each ADO leg key back to its cell slug.
fn eprint_ado_legs(var: &str, matrix: &BTreeMap<String, BTreeMap<String, String>>) {
    eprintln!(
        "Published {} matrix leg(s) to ADO variable {var}:",
        matrix.len()
    );
    let width = matrix.keys().map(String::len).max().unwrap_or(0);
    for (key, vars) in matrix {
        let slug = vars.get("slug").map_or("", String::as_str);
        eprintln!("  {key:<width$}  {slug}");
    }
}

// ───────────────────────────── base-image catalogue verbs ─────────────────────────────

/// `tailor bases download|verify`: manage the `baseImages:` catalogue (`meta/docs/2026-06-29-base-image-catalogue.md`
/// §5). Download pulls slots from their `source`; verify asserts referenced slot files are present.
async fn bases(workspace: &Workspace, what: &BasesCommand) -> Result<(), AppError> {
    let catalogue = workspace
        .tool
        .as_ref()
        .and_then(|t| t.base_images.clone())
        .ok_or_else(|| {
            AppError::Message("no `baseImages:` catalogue defined in tailor.yaml".to_owned())
        })?;
    match what {
        BasesCommand::List => {
            print_bases(&summarize(&catalogue, &workspace.root));
            Ok(())
        }
        BasesCommand::Download { names, force } => {
            let reports = download(
                &catalogue,
                &workspace.root,
                &OciFetcher::new(),
                names,
                *force,
            )
            .await?;
            for report in &reports {
                status(report.outcome.verb(), &describe_artifact(&report.path));
            }
            println!("processed {} slot(s)", reports.len());
            Ok(())
        }
        BasesCommand::Verify { names } => {
            let referenced = if names.is_empty() {
                referenced_slots(workspace)?
            } else {
                names.iter().cloned().collect()
            };
            verify(&catalogue, &workspace.root, &referenced)?;
            println!("verified {} slot(s) present", referenced.len());
            Ok(())
        }
    }
}

/// Render `tailor bases list`: one aligned line per slot with its arch, source, presence, and path.
fn print_bases(slots: &[SlotSummary]) {
    if slots.is_empty() {
        println!("No base-image slots defined.");
        return;
    }
    let width = slots.iter().map(|slot| slot.name.len()).max().unwrap_or(0);
    println!("Base images:");
    for slot in slots {
        let presence = if slot.present { "present" } else { "missing" };
        let source = match &slot.source {
            SlotSource::Oci(uri) => format!("oci:{uri}"),
            SlotSource::AzureLinux { version, variant } => {
                format!("azureLinux:{version}/{variant}")
            }
            SlotSource::OutOfBand => "out-of-band".to_owned(),
        };
        println!(
            "  {name:<width$}  {arch:<5}  {presence:<7}  {source:<28}  {path}",
            name = slot.name,
            arch = slot.arch,
            path = slot.path.display(),
        );
    }
}

/// The catalogue slot names every workspace cell binds to via `base: { ref: <name> }` — verify's
/// default scope, so an empty `tailor bases verify` checks exactly what builds will consume.
fn referenced_slots(workspace: &Workspace) -> Result<BTreeSet<String>, AppError> {
    let mut names = BTreeSet::new();
    for target in build_targets(workspace, &[])? {
        for cell in cells_selected(&target, &Selector::default())? {
            if let Some(name) = cell.base_image {
                names.insert(name);
            }
        }
    }
    Ok(names)
}

// ───────────────────────────── network verbs (registry resolution) ─────────────────────────────

async fn resolve_verb(
    workspace: &Workspace,
    names: &[String],
    engine: &EngineOverride,
) -> Result<(), AppError> {
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, names)?;
    let runtime = establish_runtime(engine, &tool).await?;
    let lock = build_lock(
        &tool,
        &targets,
        &runtime,
        &OciResolver::new(),
        &Lockfile::default(),
    )
    .await?;
    let yaml = serde_yaml_ng::to_string(&lock)
        .map_err(|e| AppError::Message(format!("serialize lock: {e}")))?;
    print!("{yaml}");
    Ok(())
}

/// Resolve and write `tailor.lock`. When `refresh` is false (`tailor lock`), inputs already pinned in
/// the current lock keep their digests — only new inputs are resolved, so the freeze is idempotent.
/// When `refresh` is true (`tailor update`), every input is re-resolved to the latest digest.
async fn lock(
    workspace: &Workspace,
    engine: &EngineOverride,
    refresh: bool,
) -> Result<(), AppError> {
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, &[])?;
    let runtime = establish_runtime(engine, &tool).await?;
    let path = workspace.root.join(LOCK_FILE);
    // `lock` reuses the existing pins (freeze current); `update` starts from an empty lock so
    // everything re-resolves to the latest digest.
    let base_lock = if refresh {
        Lockfile::default()
    } else {
        Lockfile::read(&path)?
    };
    let lock = build_lock(&tool, &targets, &runtime, &OciResolver::new(), &base_lock).await?;
    lock.write(&path)?;
    println!("wrote {}", path.display());
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedContainerImage {
    image_ref: String,
    digest: String,
    pull: bool,
    lock_digest: Option<String>,
    architecture: Option<String>,
}

async fn resolve_container_image<R, F, Fut>(
    id: Option<&str>,
    reference: &str,
    pull: PullPolicy,
    runtime: &R,
    resolve_digest: F,
    lock_digest: Option<&str>,
) -> Result<ResolvedContainerImage, AppError>
where
    R: ContainerRuntime,
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<String, ResolveError>>,
{
    if let Some(digest) = lock_digest {
        return Ok(ResolvedContainerImage {
            image_ref: digest_ref(reference, digest),
            digest: digest.to_owned(),
            pull: true,
            lock_digest: Some(digest.to_owned()),
            architecture: None,
        });
    }

    match pull {
        PullPolicy::Always => {
            let digest = resolve_digest().await?;
            Ok(ResolvedContainerImage {
                image_ref: digest_ref(reference, &digest),
                digest: digest.clone(),
                pull: true,
                lock_digest: Some(digest),
                architecture: None,
            })
        }
        PullPolicy::Missing | PullPolicy::Never => {
            match local_container_image(runtime, reference).await? {
                Some(local) => Ok(local),
                None if pull == PullPolicy::Missing => {
                    let digest = resolve_digest().await?;
                    Ok(ResolvedContainerImage {
                        image_ref: digest_ref(reference, &digest),
                        digest: digest.clone(),
                        pull: true,
                        lock_digest: Some(digest),
                        architecture: None,
                    })
                }
                None => {
                    let name =
                        id.map_or_else(|| reference.to_owned(), std::borrow::ToOwned::to_owned);
                    Err(CoreError::Resolve(ResolveError::Other(format!(
                        "image `{reference}` (source `{name}`) not found locally and pull policy is never"
                    )))
                    .into())
                }
            }
        }
    }
}

async fn local_container_image<R: ContainerRuntime>(
    runtime: &R,
    reference: &str,
) -> Result<Option<ResolvedContainerImage>, AppError> {
    let Some(image) = runtime.inspect_image(reference).await? else {
        return Ok(None);
    };
    if let Some(repo_digest) = image.repo_digests.first()
        && let Some(digest) = digest_from_repo_digest(repo_digest)
    {
        return Ok(Some(ResolvedContainerImage {
            image_ref: digest_ref(reference, digest),
            digest: digest.to_owned(),
            pull: false,
            lock_digest: Some(digest.to_owned()),
            architecture: known_local_architecture(&image),
        }));
    }
    let architecture = known_local_architecture(&image);
    Ok(Some(ResolvedContainerImage {
        image_ref: image.id.clone(),
        digest: image.id,
        pull: false,
        lock_digest: None,
        architecture,
    }))
}

fn known_local_architecture(image: &LocalImage) -> Option<String> {
    (!image.architecture.is_empty()).then(|| image.architecture.clone())
}

fn digest_ref(reference: &str, digest: &str) -> String {
    format!("{}@{digest}", repository(reference))
}

fn digest_from_repo_digest(reference: &str) -> Option<&str> {
    reference.split_once('@').map(|(_, digest)| digest)
}

fn repository(reference: &str) -> &str {
    let without_digest = reference.split('@').next().unwrap_or(reference);
    match without_digest.rfind('/') {
        Some(slash) => match without_digest[slash..].find(':') {
            Some(colon) => &without_digest[..slash + colon],
            None => without_digest,
        },
        None => without_digest.split(':').next().unwrap_or(without_digest),
    }
}

async fn resolve_toolchain_ref<R: ContainerRuntime, S: BaseResolver>(
    id: &str,
    entry: &ToolchainEntry,
    runtime: &R,
    resolver: &S,
    lock: &Lockfile,
) -> Result<ResolvedContainerImage, AppError> {
    let reference = toolchain_key(entry);
    resolve_container_image(
        Some(id),
        &reference,
        entry.pull,
        runtime,
        || resolver.resolve_toolchain(entry),
        lock.toolchain_digest(id),
    )
    .await
}

async fn resolve_tools_dir_ref<R: ContainerRuntime, S: BaseResolver>(
    id: Option<&str>,
    source: &ToolsDirSourceInline,
    runtime: &R,
    resolver: &S,
    lock: &Lockfile,
) -> Result<ResolvedContainerImage, AppError> {
    let reference = tools_dir_key(source);
    resolve_container_image(
        id,
        &reference,
        source.pull,
        runtime,
        || resolver.resolve_tools_dir(source),
        id.and_then(|id| lock.tools_dir_digest(id)),
    )
    .await
}

async fn resolve_toolchains<R: ContainerRuntime, S: BaseResolver>(
    targets: &[Arc<Target>],
    tool: &ToolConfig,
    runtime: &R,
    resolver: &S,
    lock: &Lockfile,
) -> Result<BTreeMap<String, ResolvedToolchain>, AppError> {
    let mut images = BTreeMap::new();
    for target in targets {
        let (id, entry) = toolchain_for(target, tool)?;
        if let Entry::Vacant(slot) = images.entry(toolchain_key(&entry)) {
            let image = resolve_toolchain_ref(&id, &entry, runtime, resolver, lock).await?;
            slot.insert(ResolvedToolchain {
                ic_image_ref: image.image_ref,
                pull: image.pull,
                architecture: image.architecture,
            });
        }
    }
    Ok(images)
}

fn preflight_toolchain_arches(
    targets: &[Arc<Target>],
    tool: &ToolConfig,
    toolchains: &BTreeMap<String, ResolvedToolchain>,
    selector: &Selector,
) -> Result<(), AppError> {
    for target in targets {
        let (toolchain_id, entry) = toolchain_for(target, tool)?;
        let key = toolchain_key(&entry);
        let resolved = toolchains
            .get(&key)
            .ok_or_else(|| CoreError::MissingResolvedToolchain {
                id: toolchain_id.clone(),
                reference: key.clone(),
            })?;
        let Some(architecture) = resolved.architecture.as_deref() else {
            continue;
        };
        for cell in cells_selected(target, selector)? {
            preflight_toolchain_cell_arch(
                &toolchain_id,
                Some(architecture),
                cell.arch,
                cell.slug.as_ref(),
            )?;
        }
    }
    Ok(())
}

fn preflight_toolchain_cell_arch(
    toolchain_id: &str,
    architecture: Option<&str>,
    cell_arch: Arch,
    cell_slug: &str,
) -> Result<(), AppError> {
    let Some(architecture) = architecture else {
        return Ok(());
    };
    let Some(image_arch) = parse_arch(architecture) else {
        return Ok(());
    };
    if image_arch == cell_arch {
        return Ok(());
    }
    Err(CoreError::Resolve(ResolveError::Other(format!(
        "toolchain `{toolchain_id}` local image is `{architecture}` but cell `{cell_slug}` targets `{cell_arch}`; no local image for that arch and pull policy won't fetch it"
    )))
    .into())
}

async fn resolve_tools_dir_sources<R: ContainerRuntime, S: BaseResolver>(
    targets: &[Arc<Target>],
    runtime: &R,
    resolver: &S,
    lock: &Lockfile,
) -> Result<BTreeMap<String, ResolvedToolsDirSource>, AppError> {
    let mut images = BTreeMap::new();
    for target in targets {
        for cell in cells_selected(target, &Selector::default())? {
            let Some(tools_dir) = &cell.tools_dir else {
                continue;
            };
            if let Entry::Vacant(slot) = images.entry(tools_dir_key(&tools_dir.source)) {
                let image = resolve_tools_dir_ref(
                    tools_dir.source_name.as_deref(),
                    &tools_dir.source,
                    runtime,
                    resolver,
                    lock,
                )
                .await?;
                slot.insert(ResolvedToolsDirSource {
                    image_ref: image.image_ref,
                    digest: image.digest,
                    pull: image.pull,
                });
            }
        }
    }
    Ok(images)
}

async fn build_lock(
    tool: &ToolConfig,
    targets: &[Arc<Target>],
    runtime: &impl ContainerRuntime,
    resolver: &OciResolver,
    base_lock: &Lockfile,
) -> Result<Lockfile, AppError> {
    let mut lock = Lockfile::default();
    for entry in &tool.toolchains.entries {
        let image = resolve_toolchain_ref(&entry.name, entry, runtime, resolver, base_lock).await?;
        if let Some(digest) = image.lock_digest {
            lock.toolchains.insert(
                entry.name.clone(),
                LockedContainer {
                    container: entry.container.clone(),
                    version: entry.version.as_ref().map(ToString::to_string),
                    tag: Some(entry.effective_tag()),
                    digest,
                },
            );
        }
    }
    for source in &tool.tools_dir_sources {
        let inline = source.inline();
        let image =
            resolve_tools_dir_ref(Some(&source.name), &inline, runtime, resolver, base_lock)
                .await?;
        if let Some(digest) = image.lock_digest {
            lock.tools_dirs.insert(
                source.name.clone(),
                LockedContainer {
                    container: source.container.clone(),
                    version: None,
                    tag: Some(source.effective_tag()),
                    digest,
                },
            );
        }
    }
    // Pin the ownership-janitor image when one is explicitly configured, so a build normalizes IC's
    // root-owned outputs with the exact reviewed image. The default (unconfigured) janitor is
    // tailor's built-in and is not locked. `lock` reuses the frozen digest; `update` re-resolves.
    if let Some(janitor) = tool.runtime.as_ref().and_then(|r| r.janitor_image.as_ref()) {
        let tag = janitor.tag.clone().unwrap_or_else(|| "latest".to_owned());
        let frozen = base_lock
            .runtime
            .as_ref()
            .and_then(|runtime| runtime.janitor_image.as_ref());
        let digest = if let Some(locked) = frozen {
            locked.digest.clone()
        } else {
            let entry = ToolchainEntry {
                name: "janitor".to_owned(),
                container: janitor.container.clone(),
                version: None,
                tag: Some(tag.clone()),
                pull: PullPolicy::Always,
            };
            resolver.resolve_toolchain(&entry).await?
        };
        lock.runtime = Some(LockedRuntime {
            janitor_image: Some(LockedContainer {
                container: janitor.container.clone(),
                version: None,
                tag: Some(tag),
                digest,
            }),
        });
    }
    for target in targets {
        for cell in render_image(&target.definition, &target.dir)? {
            if !matches!(
                cell.base,
                BaseSource::Oci { .. } | BaseSource::AzureLinux { .. }
            ) {
                continue;
            }
            for arch in cell_arches(&cell) {
                if let ResolvedBase::Oci {
                    reference,
                    platform,
                    digest,
                } = resolver.resolve(&cell.base, arch, &target.dir).await?
                {
                    // Freeze: keep an already-pinned digest for this base rather than the freshly
                    // resolved one, so `tailor lock` never drifts a moving tag (`tailor update`
                    // passes an empty base lock, so this always takes the fresh digest).
                    let digest = base_lock
                        .base_digest(&reference, &platform)
                        .map_or(digest, ToOwned::to_owned);
                    lock.upsert_base(LockedBase {
                        reference,
                        platform,
                        digest,
                    });
                }
            }
        }
    }
    Ok(lock)
}

// ───────────────────────────── execution verbs (Docker) ─────────────────────────────

/// A cancellation token wired to Ctrl+C / `SIGTERM`. The **first** signal cancels the token so the
/// in-flight container is stopped and removed (rather than orphaned in the daemon); a **second**
/// signal falls back to an immediate exit as an escape hatch if teardown hangs. Without this the
/// process would just die on Ctrl+C and leave the IC container running.
fn cancel_on_signal() -> CancellationToken {
    let token = CancellationToken::new();
    let signal_token = token.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        eprintln!(
            "\ninterrupt received — stopping the container and cleaning up (press Ctrl+C again to force quit)"
        );
        signal_token.cancel();
        // A second signal means "I don't want to wait for cleanup" — exit now (128 + SIGINT).
        wait_for_shutdown_signal().await;
        std::process::exit(130);
    });
    token
}

/// Resolve when the process receives an interactive interrupt (Ctrl+C) or a termination request
/// (`SIGTERM` on Unix). Installing tokio's handler also overrides the default disposition, so the
/// first signal is delivered here instead of killing the process outright.
async fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// The default Image Customizer container for a workspace-free `tailor convert`.
const DEFAULT_CONVERT_CONTAINER: &str = "mcr.microsoft.com/azurelinux/imagecustomizer:latest";

/// `tailor convert <input> --to <format>` — a one-shot `imagecustomizer convert` with no workspace or
/// config. Synthesizes a single convert cell and runs it directly through the executor, reusing the
/// same container/janitor machinery as `build` (so the output is sudo-free). See
/// `meta/docs/2026-06-22-design.md` §7.4 (the `convert` operation).
async fn convert(args: &ConvertArgs, engine: &EngineOverride) -> Result<(), AppError> {
    let cwd = env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;
    let input = tailor_config::absolutize(&args.input, &cwd);
    if !input.is_file() {
        return Err(AppError::Message(format!(
            "input image `{}` does not exist (or is not a file)",
            input.display()
        )));
    }
    let arch = match args.arch.as_deref() {
        None | Some("amd64") => Arch::Amd64,
        Some("arm64") => Arch::Arm64,
        Some(other) => {
            return Err(AppError::Message(format!(
                "unknown architecture `{other}` (expected `amd64` or `arm64`)"
            )));
        }
    };

    // Default the output beside the input (its name with the target extension); an explicit `-o` is
    // resolved against the current directory.
    let default_name = format!(
        "{}.{}",
        input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("converted"),
        format_extension(args.to)
    );
    let input_parent = input
        .parent()
        .map_or_else(|| cwd.clone(), Path::to_path_buf);
    let final_output = match &args.output {
        Some(path) => tailor_config::absolutize(path, &cwd),
        None => input_parent.join(&default_name),
    };
    let output_parent = final_output
        .parent()
        .map_or_else(|| cwd.clone(), Path::to_path_buf);
    let slug = final_output
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| AppError::Message("output path has no file name".to_owned()))?
        .to_owned();
    // Stage IC's output in a subdir of the destination: a subdir is always a safe read-write mount
    // (the guard only refuses mounting `cwd`/an ancestor), and it keeps the (potentially huge)
    // converted image on the destination filesystem so the final move is an instant rename.
    let staging = output_parent.join(format!(".tailor-convert-{}", std::process::id()));

    // Scratch base for IC's own `--build-dir`. Only honor an explicit `--build-dir-base` (absolutized
    // and subject to the separate-device build-dir guard, since it is bound read-write on the host).
    // When omitted, leave it unset so IC uses its container-internal `/tmp` — no host bind, no guard —
    // exactly like a default workspace build. A default under the system temp dir would trip the
    // build-dir guard on every system where the temp dir shares a device with `/` (e.g. CI runners).
    let build_dir_base = args
        .build_dir_base
        .clone()
        .map(|dir| tailor_config::absolutize(dir, &cwd));

    let container = args
        .container
        .clone()
        .unwrap_or_else(|| DEFAULT_CONVERT_CONTAINER.to_owned());

    // Reuse the default runtime config (host-root `/host`, privileged, default janitor) and override
    // the workspace root (so the input's directory is bind-mounted read-only) and the scratch base.
    let mut runtime = runtime_config(
        &tailor_config::defaults::default_tool_config(),
        &Lockfile::default(),
        &cwd,
    );
    runtime.workspace_root = input_parent;
    runtime.build_dir_base = build_dir_base;

    let cell = convert_cell(&input, &runtime.workspace_root, arch, args.to, slug);
    let context = ExecutionContext {
        output_dir: staging.clone(),
        ic_image_ref: container.clone(),
        base_ref: None,
        tools_dir: None,
        platform: format!("linux/{}", arch.as_str()),
        clone_index: None,
        dry_run: args.dry_run,
        pull: true,
        signer: None,
        runtime,
    };

    if args.dry_run {
        let result = IcExecutor::new(NoopRuntime)
            .execute(&cell, &context, CancellationToken::new())
            .await
            .map_err(AppError::from)?;
        println!("{}", result.logs);
        return Ok(());
    }

    status(
        "Converting",
        &format!("{} → {}", input.display(), args.to.as_str()),
    );
    let cancel = cancel_on_signal();
    let executor = IcExecutor::new(
        establish_runtime(engine, &tailor_config::defaults::default_tool_config()).await?,
    );
    let result = executor.execute(&cell, &context, cancel).await;
    // Publish the produced image out of staging to the final destination (an instant, same-filesystem
    // rename), then remove the staging dir regardless of outcome.
    let published = match &result {
        Ok(r) if r.exit_code == 0 => fs::rename(&r.artifact_path, &final_output).map_err(|e| {
            AppError::Message(format!(
                "move converted image to `{}`: {e}",
                final_output.display()
            ))
        }),
        _ => Ok(()),
    };
    let _ = fs::remove_dir_all(&staging);
    let result = result.map_err(AppError::from)?;
    if result.exit_code != 0 {
        return Err(AppError::Message(format!(
            "Image Customizer convert failed (exit {})",
            result.exit_code
        )));
    }
    published?;
    status("Converted", &final_output.display().to_string());
    Ok(())
}

/// The output-file extension for a format (mirrors [`artifact_name`]); `convert` never produces the
/// directory/`pxe` forms, so every convert format has a real extension.
fn format_extension(format: OutputFormat) -> &'static str {
    match format {
        OutputFormat::Cosi => "cosi",
        OutputFormat::Vhd | OutputFormat::VhdFixed => "vhd",
        OutputFormat::Vhdx => "vhdx",
        OutputFormat::Qcow2 => "qcow2",
        OutputFormat::Raw | OutputFormat::BaremetalImage => "raw",
        OutputFormat::Iso => "iso",
        OutputFormat::PxeTar => "tar.gz",
        OutputFormat::PxeDir => "",
    }
}

/// Synthesize the single `Cell` for a workspace-free convert: a local-file base, `operation: convert`,
/// no config, one output. `dir` (the base-relative root) is the input's directory.
fn convert_cell(input: &Path, dir: &Path, arch: Arch, format: OutputFormat, slug: String) -> Cell {
    let definition = ImageDefinition {
        name: "convert".to_owned(),
        toolchain: None,
        skip: false,
        tools_dir: None,
        matrix: None,
        selectors: None,
        outputs: None,
        base: None,
        features: Vec::new(),
        params: IndexMap::new(),
        rpm_sources: Vec::new(),
        operation: Some(Operation::Convert),
        output_artifacts: None,
        signing: None,
        extra_dependencies: Vec::new(),
        depends_on: Vec::new(),
        inputs: Vec::new(),
        extra_params: Vec::new(),
        config: None,
    };
    let target = Target {
        definition,
        dir: dir.to_path_buf(),
        default_outputs: Vec::new(),
        output_artifacts: OutputArtifactsPolicy::default(),
        root: dir.to_path_buf(),
        base_images: BaseImageCatalogue::default(),
        tools_dir_sources: Vec::new(),
    };
    let output = OutputSpec {
        format,
        cosi_compression_level: None,
        compression: None,
        name: None,
    };
    Cell {
        target: Arc::new(target),
        axes: BTreeMap::new(),
        arch,
        output,
        slug: CellSlug(slug),
        ic_config: serde_yaml_ng::Value::Null,
        base: BaseSource::Path {
            path: input.to_path_buf(),
            arch: Some(arch),
        },
        base_image: None,
        rpm_sources: Vec::new(),
        extra_params: Vec::new(),
        input_deps: Vec::new(),
        tools_dir: None,
        skip: false,
        skip_pins: Vec::new(),
    }
}

async fn build(
    workspace: &Workspace,
    args: BuildArgs,
    engine: &EngineOverride,
    logging: &LogOverrides,
) -> Result<(), AppError> {
    let mut tool = tool_config(workspace);
    resolve_image_cache_dir(&mut tool, &workspace.root);
    apply_log_overrides(&mut tool, logging, &workspace.root)?;
    apply_build_dir_base_override(&mut tool, args.build_dir_base.as_deref())?;
    // All workspace images supply producer definitions for resolving `base: { image }` references,
    // and let a positional cell slug resolve to its owning image.
    let all_members = all_member_targets(workspace)?;
    // Positionals may name images *or* cell slugs: a slug builds exactly that cell (of its owning
    // image), so `tailor build <slug>` works without naming the image.
    let (image_names, positional_slugs) = classify_build_positionals(&args.images, &all_members)?;
    let targets = build_targets(workspace, &image_names)?;
    // Build the selected images plus their transitive producers, in topological order, so a consumer
    // is planned *after* its producers (its base hashes then reflect the fresh artifacts —
    // `meta/docs/2026-09-09-inter-image-dependencies.md` §4). This also detects dependency cycles. Both the
    // dry-run and real-build paths schedule over this order.
    let closure = imagedep::dependency_closure(&targets, &all_members)?;
    let ordered = imagedep::topological_order(&closure, &all_members)?;
    // Positional slugs join any explicit `--cell` selection.
    let mut cell_selectors = args.select.cell.clone();
    cell_selectors.extend(positional_slugs);
    let selection = Selector::parse(&args.select.select, &cell_selectors, &args.arch)?;
    // The user selector applies only to the explicitly requested images; a transitively pulled-in
    // producer instead builds exactly the cells its consumers need. Walk the schedule upstream
    // (reverse topological order) so each consumer registers the producer coordinates it requires
    // *before* those producers are planned, so a `-s`/`--cell` selection meant for a consumer never
    // drops (or errors on) a producer's paired cell (`inter-image-dependencies.md` §4).
    let requested: BTreeSet<String> = targets.iter().map(|t| t.name().to_owned()).collect();
    let mut required: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for node in ordered.iter().rev() {
        let apply_selector = requested.contains(node.name());
        let node_cells =
            select_node_cells(node, &selection, apply_selector, required.get(node.name()))?;
        for cell in &node_cells {
            for (producer, slug) in imagedep::required_producers(cell, &all_members)? {
                required.entry(producer).or_default().insert(slug);
            }
        }
    }
    let build_selection = BuildSelection {
        selector: &selection,
        requested,
        required,
    };
    let output_dir = tailor_config::absolutize(
        args.output_dir
            .clone()
            .unwrap_or_else(|| workspace.root.join(ARTIFACTS_DIR)),
        env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?,
    );
    // Default the build directory to a tailor-owned location under the output directory when
    // `runtime.buildDirBase` is unset, so builds — including tools-dir images, which need a writable
    // scratch dir — work with no host-specific path. The output directory is a real, bound
    // filesystem (never the container's overlayfs), so IC's read-only/`/etc` overlays do not stack
    // overlay-on-overlay.
    if tool
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.build_dir_base.as_ref())
        .is_none()
    {
        tool.runtime
            .get_or_insert_with(Runtime::default)
            .build_dir_base = Some(output_dir.join(TAILOR_STATE_DIR).join(BUILD_DIR));
    }
    let signing = signing_requirements(&targets, tool.signing.as_ref())?;
    ensure_signing_preview(&tool, &signing)?;
    // Build one signer per required profile (a shared CA per build); `signer_for` resolves a cell to
    // its signer via the image name (meta/docs/2026-06-29-signing.md §6). Empty when nothing signs.
    let signers = build_signers(&signing, &workspace.root);
    let image_signer: HashMap<&str, Arc<dyn Signer>> = signers
        .iter()
        .flat_map(|(requirement, signer)| {
            requirement
                .images
                .iter()
                .map(move |image| (image.as_str(), Arc::clone(signer)))
        })
        .collect();
    let signer_for = |cell: &Cell| image_signer.get(cell.target.name()).cloned();

    // Dry-run prints each selected cell's container invocation (the signed 3-pass for signed cells)
    // without resolving digests, running, or contacting any engine — daemon-free via a no-op runtime.
    if args.dry_run {
        let orchestrator = Orchestrator::new(IcExecutor::new(NoopRuntime), OciResolver::new());
        let results = orchestrator
            .dry_run(
                &ordered,
                &all_members,
                &tool,
                &build_selection,
                &workspace.root,
                &output_dir,
                &signer_for,
            )
            .await?;
        println!("{} cell(s) (dry-run)\n", results.len());
        for result in results {
            println!("{}\n", result.logs);
        }
        if !signing.is_empty() {
            report_signing(&signers);
        }
        return Ok(());
    }

    // Fail fast on every signing prerequisite — including the `openssl`/`sbsign` binaries — before any
    // (slow, privileged) IC run (meta/docs/2026-06-29-signing.md §5.1).
    preflight_signers(&signers)?;

    // Admit a single build per output directory: a build writes shared state there (stamps, the hash
    // cache, artifacts), so two concurrent builds would race on those writes. Held for the rest of
    // the build and released automatically on exit (even on a crash).
    let _build_lock = match WorktreeLock::acquire(output_dir.join(TAILOR_STATE_DIR)) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            return Err(AppError::Message(format!(
                "another tailor build is already running for output directory `{}`; wait for it to \
                 finish or use a separate `--output-dir`",
                output_dir.display()
            )));
        }
        Err(source) => {
            return Err(AppError::Message(format!(
                "failed to acquire the build lock under `{}`: {source}",
                output_dir.join(TAILOR_STATE_DIR).display()
            )));
        }
    };

    // Wire Ctrl+C / SIGTERM to a cancellation token so an interrupted build tears down its running
    // container instead of orphaning it in the daemon (the container runtime removes the container on
    // cancel — `container/runtime.rs`).
    let cancel = cancel_on_signal();

    // A real build contacts the engine: resolve it and fail fast now if it is missing or
    // unreachable (`meta/docs/2026-06-29-container-runtimes.md` §4-§5).
    let runtime = establish_runtime(engine, &tool).await?;
    let base_hash_cache_dir = output_dir.join(TAILOR_STATE_DIR).join(BASE_HASH_CACHE_DIR);
    let _ = fs::create_dir_all(&base_hash_cache_dir);
    let resolver = OciResolver::with_cache_dir(base_hash_cache_dir.clone());
    let lock = Lockfile::read(&workspace.root.join(LOCK_FILE))?;
    let toolchains = resolve_toolchains(&targets, &tool, &runtime, &resolver, &lock).await?;
    preflight_toolchain_arches(&targets, &tool, &toolchains, &selection)?;
    let tools_dir_sources = resolve_tools_dir_sources(&targets, &runtime, &resolver, &lock).await?;
    let orchestrator = Orchestrator::new(IcExecutor::new(runtime), resolver);

    status("Toolchain", &describe_toolchains(&tool));
    let started = Instant::now();
    let clones = args.clones.max(1);
    let mut built = 0usize;
    let mut selected_cells = 0usize;
    for node in &ordered {
        let plan = orchestrator
            .plan(
                std::slice::from_ref(node),
                &all_members,
                &tool,
                &lock,
                &toolchains,
                &tools_dir_sources,
                &build_selection,
                &output_dir,
                Some(&base_hash_cache_dir),
            )
            .await?;
        selected_cells += plan.cells.len();
        // Base descriptions are looked up per slug from the plan for the progress line.
        let bases: BTreeMap<&str, String> = plan
            .cells
            .iter()
            .map(|planned| {
                (
                    planned.cell.slug.as_ref(),
                    describe_base(&planned.cell.base),
                )
            })
            .collect();
        for clone in 0..clones {
            let options = BuildOptions {
                force: args.force,
                dry_run: false,
                clone_index: (clones > 1).then_some(clone),
            };
            let mut on_progress = |event: BuildProgress<'_>| match event {
                BuildProgress::Building { slug } => {
                    let base = bases.get(slug).map_or("", String::as_str);
                    status("Customizing", &format!("{slug}  ({base})"));
                }
                BuildProgress::Built { artifact, .. } => {
                    built += 1;
                    status("Built", &describe_artifact(artifact));
                }
            };
            orchestrator
                .build(
                    &plan,
                    &tool,
                    &lock,
                    &toolchains,
                    &workspace.root,
                    &output_dir,
                    &options,
                    cancel.clone(),
                    &mut on_progress,
                    &signer_for,
                )
                .await?;
        }
    }
    let _ = selected_cells;
    status(
        "Finished",
        &format!(
            "{built} artifact(s) in {}",
            format_duration(started.elapsed())
        ),
    );
    Ok(())
}

async fn clean(
    workspace: &Workspace,
    names: &[String],
    selector: &Selector,
    engine: &EngineOverride,
) -> Result<(), AppError> {
    let tool = tool_config(workspace);
    let targets = build_targets(workspace, names)?;
    let output_dir = workspace.root.join(ARTIFACTS_DIR);
    let lock = Lockfile::read(&workspace.root.join(LOCK_FILE))?;

    let mut paths = Vec::new();
    for target in &targets {
        // A signed image also drops an enrollable CA cert beside each cell's image (§6); remove it
        // too. Lenient on config: `validate` surfaces signing errors; cleanup should not fail on them.
        let signed = tailor_config::resolve_signing(
            target.definition.signing.as_ref(),
            tool.signing.as_ref(),
        )
        .ok()
        .flatten()
        .is_some();
        for cell in cells_selected(target, selector)? {
            paths.push(output_dir.join(tailor_core::artifact_name(
                cell.slug.as_ref(),
                cell.output.format,
            )));
            if signed {
                paths.push(output_dir.join(ca_cert_name(cell.slug.as_ref())));
            }
        }
    }

    let executor = IcExecutor::new(establish_runtime(engine, &tool).await?);
    executor
        .clean(
            &paths,
            &runtime_config(&tool, &lock, &workspace.root),
            CancellationToken::new(),
        )
        .await?;
    println!("cleaned {} artifact path(s)", paths.len());
    Ok(())
}

/// Gather the connection-resolution inputs (`meta/docs/2026-06-29-container-runtimes.md` §3): the per-invocation
/// flags, the engine environment variables, and the manifest `runtime.engine` / `runtime.host`.
fn resolve_inputs(engine: &EngineOverride, tool: &ToolConfig) -> ResolveInputs {
    let runtime = tool.runtime.as_ref();
    ResolveInputs {
        flag_engine: engine.engine,
        flag_host: engine.host.clone(),
        env_docker_host: non_empty_env("DOCKER_HOST"),
        env_container_host: non_empty_env("CONTAINER_HOST"),
        manifest_engine: runtime.and_then(|runtime| runtime.engine),
        manifest_host: runtime.and_then(|runtime| runtime.host.clone()),
        xdg_runtime_dir: non_empty_env("XDG_RUNTIME_DIR"),
    }
}

/// Resolve the engine/endpoint, connect, and run the fail-fast preflight (§4-§5).
async fn establish_runtime(
    engine: &EngineOverride,
    tool: &ToolConfig,
) -> Result<BollardRuntime, AppError> {
    let plan = resolve(&resolve_inputs(engine, tool));
    Ok(BollardRuntime::establish(&plan).await?)
}

/// A set, non-empty environment variable, else `None`.
fn non_empty_env(key: &str) -> Option<String> {
    env::var(key).ok().filter(|value| !value.is_empty())
}

// ───────────────────────────── build reporting (cargo-style) ─────────────────────────────

/// Whether to emit ANSI color — the shared, process-global decision (`NO_COLOR` off, `CLICOLOR_FORCE`
/// on, else stderr is a terminal). Re-exported so tailor's status output, error prefix, and the
/// pass-through of Image Customizer's colored logs all agree.
pub(crate) use tailor_exec::color_enabled as use_color;

/// The live timestamp prefix shared by status lines and tracing.
pub(crate) fn timestamp_prefix() -> String {
    match *TIMESTAMP_MODE.get_or_init(TimestampMode::default) {
        TimestampMode::Elapsed => fmt_elapsed(STARTED.get_or_init(Instant::now).elapsed()),
        TimestampMode::Time => fmt_wall_clock(SystemTime::now()),
        TimestampMode::Off => String::new(),
    }
}

fn fmt_elapsed(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64();
    format!("{seconds:.ELAPSED_TIMESTAMP_PRECISION$}s")
}

fn fmt_wall_clock(now: SystemTime) -> String {
    let secs = now
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
        % SECS_PER_DAY;
    format!(
        "{:02}:{:02}:{:02}",
        secs / SECS_PER_HOUR,
        (secs % SECS_PER_HOUR) / SECS_PER_MINUTE,
        secs % SECS_PER_MINUTE
    )
}

#[cfg(test)]
fn timestamp_prefix_for(mode: TimestampMode, elapsed: Duration, now: SystemTime) -> String {
    match mode {
        TimestampMode::Elapsed => fmt_elapsed(elapsed),
        TimestampMode::Time => fmt_wall_clock(now),
        TimestampMode::Off => String::new(),
    }
}

/// Print a cargo-style status line to stderr: an optional leading timestamp, then a right-aligned
/// (bold green) verb plus a message.
fn status(verb: &str, message: &str) {
    eprintln!("{}", status_line(verb, message, use_color()));
}

/// Render a status line (factored out of [`status`] so the formatting is testable).
fn status_line(verb: &str, message: &str, color: bool) -> String {
    render_status_line(&timestamp_prefix(), verb, message, color)
}

fn render_status_line(prefix: &str, verb: &str, message: &str, color: bool) -> String {
    let padding = " ".repeat(STATUS_VERB_WIDTH.saturating_sub(verb.chars().count()));
    let leading = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}  ")
    };
    if color {
        format!("{leading}{padding}\u{1b}[1;32m{verb}\u{1b}[0m {message}")
    } else {
        format!("{leading}{padding}{verb} {message}")
    }
}

/// The IC container in use (the workspace default toolchain; per-image overrides show in `list`).
fn describe_toolchains(tool: &ToolConfig) -> String {
    let id = &tool.toolchains.default;
    tool.toolchains.get(id).map_or_else(
        || id.clone(),
        |entry| format!("{}:{}", entry.container, entry.effective_tag()),
    )
}

/// A short, human description of a base source.
fn describe_base(base: &BaseSource) -> String {
    match base {
        BaseSource::Path { path, .. } => format!("path: {}", path.display()),
        BaseSource::Oci { oci } => format!("oci: {}", oci.uri),
        BaseSource::AzureLinux { azure_linux } => {
            format!("azureLinux {}/{}", azure_linux.version, azure_linux.variant)
        }
        BaseSource::Ref { reference } => format!("ref: {reference}"),
        BaseSource::Image { image, output, .. } => match output {
            Some(format) => format!("image: {image} ({})", format.as_str()),
            None => format!("image: {image}"),
        },
    }
}

/// `<artifact> (<size>)`, with the artifact shown relative to the current directory when possible.
fn describe_artifact(artifact: &Path) -> String {
    let shown = env::current_dir()
        .ok()
        .and_then(|cwd| artifact.strip_prefix(&cwd).ok().map(Path::to_path_buf))
        .unwrap_or_else(|| artifact.to_path_buf());
    match fs::metadata(artifact) {
        Ok(meta) => format!("{} ({})", shown.display(), format_bytes(meta.len())),
        Err(_) => shown.display().to_string(),
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    #[expect(
        clippy::cast_precision_loss,
        reason = "human-readable size, precision irrelevant"
    )]
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

fn format_duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs >= 60 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{:.1}s", elapsed.as_secs_f64())
    }
}

/// Resolve `runtime.imageCacheDir` to an **absolute** path: a manifest-relative value is taken
/// relative to the workspace root, and an absent value defaults to `<workspace>/.tailor/cache` (IC
/// refuses to download `oci`/`azureLinux` bases without a cache dir). Absolutizing here is required:
/// the path is both translated into the `/host` mount for IC and used as a verbatim container bind by
/// the janitor sweep, and Docker rejects a relative bind source (e.g. `./.tailor/cache`).
fn resolve_image_cache_dir(tool: &mut ToolConfig, workspace_root: &Path) {
    let runtime = tool.runtime.get_or_insert_with(Runtime::default);
    let dir = runtime
        .image_cache_dir
        .clone()
        .unwrap_or_else(|| PathBuf::from(tailor_config::defaults::DEFAULT_IMAGE_CACHE_DIR));
    runtime.image_cache_dir = Some(tailor_config::absolutize(dir, workspace_root));
}

/// Apply the global `--log-dir` / `--ic-log-level` overrides onto the manifest's `runtime` settings,
/// resolving the log directory by precedence (flag > `TAILOR_LOG_DIR` > manifest;
/// `meta/docs/2026-06-29-logging.md` §5.5). Only mutates `runtime` when there is an override to apply, so a plain
/// `tailor build` keeps persistence off.
fn apply_log_overrides(
    tool: &mut ToolConfig,
    logging: &LogOverrides,
    workspace_root: &Path,
) -> Result<(), AppError> {
    // Absolutize each source against its own base BEFORE it reaches path translation: a relative log
    // dir would otherwise become `/host/` + `./…` = the host root's directory (the same host-root
    // escape class as the wipe). Flag/env resolve against the CWD, the manifest against the workspace.
    let cwd = env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;
    let flag = logging
        .log_dir
        .clone()
        .map(|dir| tailor_config::absolutize(dir, &cwd));
    let env = log_dir_from_env().map(|dir| tailor_config::absolutize(dir, &cwd));
    let manifest = tool
        .runtime
        .as_ref()
        .and_then(|r| r.log_dir.clone())
        .map(|dir| tailor_config::absolutize(dir, workspace_root));
    let log_dir = resolve_log_dir(flag, env, manifest);
    if log_dir.is_none() && logging.ic_log_level.is_none() {
        return Ok(());
    }
    let runtime = tool.runtime.get_or_insert_with(Runtime::default);
    runtime.log_dir = log_dir;
    if let Some(level) = logging.ic_log_level {
        runtime.log_level = Some(level);
    }
    Ok(())
}

/// Apply the `tailor build --build-dir-base` override: set `runtime.buildDirBase` from the flag,
/// absolutized against the CWD (matching the other CLI dir overrides). Lets CI point per-cell build
/// scratch at a pool-specific filesystem without mutating the committed `tailor.yaml`. The
/// fail-closed build-dir guard still runs on the effective dir at execution (`arg_builder`), so an
/// unsafe path (`/` or on the same filesystem as `/`) is rejected regardless of source.
fn apply_build_dir_base_override(
    tool: &mut ToolConfig,
    flag: Option<&Path>,
) -> Result<(), AppError> {
    let Some(base) = flag else {
        return Ok(());
    };
    let cwd = env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;
    tool.runtime
        .get_or_insert_with(Runtime::default)
        .build_dir_base = Some(tailor_config::absolutize(base, &cwd));
    Ok(())
}

/// Gate the signing preview feature: signing is not yet part of the stable contract, so a workspace
/// that uses `signing:` must opt in with `previewFeatures: [signing]` in `tailor.yaml`. A signed
/// build without the opt-in is a hard error (`meta/docs/2026-09-10-tailor-1.0-implementation-plan.md`).
fn ensure_signing_preview(
    tool: &ToolConfig,
    signing: &[SigningRequirement<'_>],
) -> Result<(), AppError> {
    if !signing.is_empty() && !tool.preview_enabled(PreviewFeature::Signing) {
        let images = signing
            .iter()
            .flat_map(|requirement| requirement.images.iter())
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(AppError::Message(format!(
            "signing is a preview feature, but it is not enabled: {images} request signing. Add \
             `previewFeatures:\n  - signing` to tailor.yaml to opt in."
        )));
    }
    Ok(())
}

/// Gather the distinct signing profiles the selected images require, tracking which images need
/// each (`meta/docs/2026-06-29-signing.md` §5.1). Resolution also validates each referenced profile.
fn signing_requirements<'a>(
    targets: &[Arc<Target>],
    workspace_signing: Option<&'a tailor_config::SigningConfig>,
) -> Result<Vec<SigningRequirement<'a>>, AppError> {
    let mut requirements: Vec<SigningRequirement<'a>> = Vec::new();
    for target in targets {
        let resolved =
            tailor_config::resolve_signing(target.definition.signing.as_ref(), workspace_signing)?;
        if let Some((profile_id, profile)) = resolved {
            let image = target.definition.name.clone();
            if let Some(existing) = requirements
                .iter_mut()
                .find(|requirement| requirement.profile_id == profile_id)
            {
                existing.images.push(image);
            } else {
                requirements.push(SigningRequirement {
                    profile_id,
                    profile,
                    images: vec![image],
                });
            }
        }
    }
    Ok(requirements)
}

/// A required signing profile paired with its resolved [`Signer`].
type ProfileSigner<'a> = (&'a SigningRequirement<'a>, Arc<dyn Signer>);

/// Build one [`Signer`] per required profile — a shared CA per build (`meta/docs/2026-06-29-signing.md` §6).
/// Relative `keypair` key/cert paths resolve against the workspace root.
fn build_signers<'a>(
    requirements: &'a [SigningRequirement<'a>],
    root: &Path,
) -> Vec<ProfileSigner<'a>> {
    requirements
        .iter()
        .map(|requirement| {
            (
                requirement,
                tailor_sign::build_signer(&requirement.profile_id, requirement.profile, root),
            )
        })
        .collect()
}

/// Fail-fast signing gate (`meta/docs/2026-06-29-signing.md` §5.1): run every signer's preflight (tool binaries
/// and key material), aggregating all unmet prerequisites — with the requesting images — into one
/// error, so the user fixes them in a single pass before any IC run.
fn preflight_signers(signers: &[ProfileSigner<'_>]) -> Result<(), AppError> {
    let mut missing: Vec<MissingPrerequisite> = Vec::new();
    for (requirement, signer) in signers {
        match signer.preflight() {
            Ok(()) => {}
            Err(SignError::Preflight { missing: unmet }) => {
                for mut item in unmet {
                    // The signer does not know which images requested it; fill that in for the report.
                    item.images.clone_from(&requirement.images);
                    missing.push(item);
                }
            }
            Err(SignError::Execution { detail }) => missing.push(MissingPrerequisite {
                profile_id: requirement.profile_id.clone(),
                detail,
                images: requirement.images.clone(),
            }),
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(SignError::Preflight { missing }.into())
    }
}

/// Report signing-preflight status **without failing** — for `validate` and `--dry-run`
/// (`meta/docs/2026-06-29-signing.md` §5.1). Runs each signer's preflight so missing `openssl`/`sbsign` binaries
/// surface too.
fn report_signing(signers: &[ProfileSigner<'_>]) {
    for (requirement, signer) in signers {
        let images = requirement.images.join(", ");
        match signer.preflight() {
            Ok(()) => println!(
                "✓ signing profile `{}` ready (image(s): {images})",
                requirement.profile_id
            ),
            Err(SignError::Preflight { missing }) => {
                for item in missing {
                    println!(
                        "⚠ signing profile `{}` not ready: {} (image(s): {images})",
                        requirement.profile_id, item.detail
                    );
                }
            }
            Err(SignError::Execution { detail }) => println!(
                "⚠ signing profile `{}` not ready: {detail} (image(s): {images})",
                requirement.profile_id
            ),
        }
    }
}

// ───────────────────────────── helpers ─────────────────────────────

fn tool_config(workspace: &Workspace) -> ToolConfig {
    match &workspace.tool {
        Some(tool) => tool.clone(),
        None => tailor_config::defaults::default_tool_config(),
    }
}

/// Every workspace image as a `Target`, **including `skip: true` images** — the resolution universe
/// for inter-image dependencies (a producer may be a skipped image built only transitively).
fn all_member_targets(workspace: &Workspace) -> Result<Vec<Arc<Target>>, AppError> {
    let names: Vec<String> = workspace
        .images
        .iter()
        .map(|image| image.definition.name.clone())
        .collect();
    build_targets(workspace, &names)
}

/// Split `build` positionals into image names and cell slugs. A positional that names a workspace
/// image is an image target; one that matches a cell slug resolves to that cell (its owning image is
/// built, restricted to that slug). Anything matching neither is an error. Empty input yields empty
/// lists (build every image).
fn classify_build_positionals(
    names: &[String],
    all_members: &[Arc<Target>],
) -> Result<(Vec<String>, Vec<String>), AppError> {
    let mut image_names: Vec<String> = Vec::new();
    let mut cell_slugs: Vec<String> = Vec::new();
    for name in names {
        if all_members.iter().any(|target| target.name() == name) {
            if !image_names.iter().any(|existing| existing == name) {
                image_names.push(name.clone());
            }
            continue;
        }
        // Not an image name — resolve it as a cell slug across all members (including `skip`ped ones).
        let mut owner = None;
        for target in all_members {
            if cells(target)?.iter().any(|cell| cell.slug.as_ref() == name) {
                owner = Some(target.name().to_owned());
                break;
            }
        }
        match owner {
            Some(image) => {
                cell_slugs.push(name.clone());
                if !image_names.contains(&image) {
                    image_names.push(image);
                }
            }
            None => {
                return Err(AppError::Message(format!(
                    "no matching image or cell slug: `{name}` (run `tailor slugs` to list cells)"
                )));
            }
        }
    }
    Ok((image_names, cell_slugs))
}

fn build_targets(workspace: &Workspace, names: &[String]) -> Result<Vec<Arc<Target>>, AppError> {
    let defaults = workspace.tool.as_ref().and_then(|t| t.defaults.as_ref());
    let default_outputs = defaults
        .and_then(|d| d.outputs.clone())
        .unwrap_or_else(default_cosi);
    let default_output_artifacts = defaults
        .and_then(|d| d.output_artifacts)
        .unwrap_or_default();
    let base_images = workspace
        .tool
        .as_ref()
        .and_then(|t| t.base_images.clone())
        .unwrap_or_default();

    let mut targets = Vec::new();
    for image in &workspace.images {
        let explicitly_named = names.iter().any(|n| n == &image.definition.name);
        if !names.is_empty() && !explicitly_named {
            continue;
        }
        // A `skip: true` image is excluded from bulk selection (no image named) so the pipeline never
        // grabs it; naming it explicitly overrides the skip so it can still be built/iterated locally.
        if image.definition.skip && !explicitly_named {
            continue;
        }
        targets.push(Arc::new(Target {
            definition: image.definition.clone(),
            dir: image.dir.clone(),
            default_outputs: default_outputs.clone(),
            output_artifacts: image
                .definition
                .output_artifacts
                .unwrap_or(default_output_artifacts),
            root: workspace.root.clone(),
            base_images: base_images.clone(),
            tools_dir_sources: workspace
                .tool
                .as_ref()
                .map_or_else(Vec::new, |tool| tool.tools_dir_sources.clone()),
        }));
    }
    if !names.is_empty() && targets.is_empty() {
        return Err(AppError::Message(format!(
            "no matching images: {}",
            names.join(", ")
        )));
    }
    Ok(targets)
}

/// Capture the process-global zero point and timestamp mode. Later calls are no-ops so tests and
/// alternate entry points cannot move the clock once live output has started.
pub(crate) fn init_timestamps(mode: TimestampMode) {
    let _ = STARTED.set(Instant::now());
    let _ = TIMESTAMP_MODE.set(mode);
}

/// The built-in default output (`cosi`) when neither the image nor the workspace declares one.
fn default_cosi() -> Vec<OutputSpec> {
    vec![OutputSpec {
        format: OutputFormat::Cosi,
        cosi_compression_level: None,
        compression: None,
        name: None,
    }]
}

fn cell_arches(cell: &RenderedCell) -> Vec<Arch> {
    if let Some(arch) = cell.tuple.get("arch").and_then(parse_arch) {
        return vec![arch];
    }
    // No `arch` axis: a multi-arch registry base pins its arch via `oci.platform`; else `amd64`.
    let base_arch = match &cell.base {
        BaseSource::Oci { oci } => oci
            .platform
            .as_deref()
            .and_then(|platform| platform.split('/').nth(1))
            .and_then(parse_arch),
        _ => None,
    };
    vec![base_arch.unwrap_or(Arch::Amd64)]
}

fn parse_arch(value: &str) -> Option<Arch> {
    match value {
        "amd64" => Some(Arch::Amd64),
        "arm64" => Some(Arch::Arm64),
        _ => None,
    }
}

// ───────────────────────────── init (scaffolding) ─────────────────────────────

const IMAGE_FILE: &str = "image.yaml";
const MANIFEST_FILE: &str = "tailor.yaml";
/// Token replaced with the image name in the templates below.
const NAME_TOKEN: &str = "__IMAGE_NAME__";

/// Scaffold a new tailor project from a template (`tailor init <name> [base|simple|advanced]`).
fn init(args: &InitArgs) -> Result<(), AppError> {
    let name = args.name.trim();
    validate_name(name)?;
    let cwd = env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;

    match args.template {
        InitTemplate::Simple => {
            // A single standalone image. The render pipeline keys off `image.yaml`, so the file is
            // named `image.yaml` (in the cwd) rather than `<name>.yaml`; the image's `name:` is set.
            write_new(&cwd.join(IMAGE_FILE), &fill_template(SIMPLE_IMAGE, name))?;
            println!("\nScaffolded standalone image `{name}`. Try: tailor validate");
        }
        InitTemplate::Base => {
            write_new(&cwd.join(MANIFEST_FILE), TAILOR_MANIFEST)?;
            write_new(
                &cwd.join(name).join(IMAGE_FILE),
                &fill_template(BASE_IMAGE, name),
            )?;
            println!("\nScaffolded workspace with image `{name}`. Try: tailor list");
        }
        InitTemplate::Advanced => {
            let image_dir = cwd.join(name);
            write_new(&cwd.join(MANIFEST_FILE), TAILOR_MANIFEST)?;
            write_new(
                &image_dir.join(IMAGE_FILE),
                &fill_template(ADVANCED_IMAGE, name),
            )?;
            write_new(
                &image_dir.join("by-variant/minimal.yaml"),
                BY_VARIANT_MINIMAL,
            )?;
            write_new(&image_dir.join("by-variant/full.yaml"), BY_VARIANT_FULL)?;
            write_new(&image_dir.join("by-arch/amd64.yaml"), BY_ARCH_AMD64)?;
            write_new(&image_dir.join("by-arch/arm64.yaml"), BY_ARCH_ARM64)?;
            println!(
                "\nScaffolded workspace with matrix image `{name}`. Try: tailor matrix {name}"
            );
        }
    }
    Ok(())
}

/// Substitute the image name into a template.
fn fill_template(template: &str, name: &str) -> String {
    template.replace(NAME_TOKEN, name)
}

/// Reject names that are empty or that would escape their directory.
fn validate_name(name: &str) -> Result<(), AppError> {
    if name.is_empty() || name.contains(['/', '\\']) || name.starts_with('.') {
        return Err(AppError::Message(format!(
            "invalid name `{name}`: use a bare name without path separators",
        )));
    }
    Ok(())
}

// ───────────────────────────── add (image / axis) ─────────────────────────────

/// Placeholder value seeded for a freshly-added axis so the matrix stays valid until edited.
const AXIS_PLACEHOLDER: &str = "todo";

fn add(what: &AddCommand) -> Result<(), AppError> {
    match what {
        AddCommand::Image { name } => add_image(name),
        // `add axis <axis>` (one arg) or `add axis <image> <axis>` (two args).
        AddCommand::Axis {
            first,
            second: Some(axis),
        } => add_axis(Some(first), axis),
        AddCommand::Axis {
            first: axis,
            second: None,
        } => add_axis(None, axis),
    }
}

/// `tailor add image <name>` — scaffold a new member image and register it in the workspace manifest.
fn add_image(name: &str) -> Result<(), AppError> {
    validate_name(name)?;
    let cwd = env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;
    let manifest = find_manifest(&cwd).ok_or_else(|| {
        AppError::Message(
            "no tailor.yaml in this directory or any parent — run `tailor init` first".to_owned(),
        )
    })?;
    let root = manifest.parent().unwrap_or(cwd.as_path());
    let image_dir = cwd.join(name);
    write_new(
        &image_dir.join(IMAGE_FILE),
        &fill_template(BASE_IMAGE, name),
    )?;

    let rel = image_dir
        .strip_prefix(root)
        .unwrap_or(&image_dir)
        .to_string_lossy()
        .replace('\\', "/");
    let text = fs::read_to_string(&manifest)
        .map_err(|e| AppError::Message(format!("read {}: {e}", manifest.display())))?;
    let updated = scaffold::register_member(&text, &rel).map_err(AppError::Message)?;
    if updated != text {
        fs::write(&manifest, updated)
            .map_err(|e| AppError::Message(format!("write {}: {e}", manifest.display())))?;
        println!("  updated {}", manifest.display());
    }
    println!("\nAdded image `{name}`. Try: tailor list");
    Ok(())
}

/// `tailor add axis [<image>] <axis>` — append an axis to an image's matrix and create its
/// `by-<axis>/` directory.
fn add_axis(image: Option<&str>, axis: &str) -> Result<(), AppError> {
    validate_name(axis)?;
    let cwd = env::current_dir().map_err(|e| AppError::Message(format!("current dir: {e}")))?;
    let workspace = discover(cwd)?;
    let target = match image {
        Some(name) => workspace
            .image(name)
            .ok_or_else(|| AppError::Message(format!("unknown image `{name}`")))?,
        None => match workspace.images.as_slice() {
            [only] => only,
            [] => return Err(AppError::Message("no image found here".to_owned())),
            many => {
                let names = many
                    .iter()
                    .map(|i| i.definition.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(AppError::Message(format!(
                    "workspace has multiple images ({names}); name one: tailor add axis <image> {axis}",
                )));
            }
        },
    };

    let image_file = target.dir.join(IMAGE_FILE);
    let text = fs::read_to_string(&image_file)
        .map_err(|e| AppError::Message(format!("read {}: {e}", image_file.display())))?;
    let updated = scaffold::add_axis(&text, axis, AXIS_PLACEHOLDER).map_err(AppError::Message)?;
    fs::write(&image_file, updated)
        .map_err(|e| AppError::Message(format!("write {}: {e}", image_file.display())))?;
    println!("  updated {}", image_file.display());

    let by_dir = target.dir.join(format!("by-{axis}"));
    fs::create_dir_all(&by_dir)
        .map_err(|e| AppError::Message(format!("create {}: {e}", by_dir.display())))?;
    println!("  created {}/", by_dir.display());

    println!(
        "\nAdded axis `{axis}` to `{}` (placeholder value `{AXIS_PLACEHOLDER}`). Edit its values in \
         image.yaml and add by-{axis}/<value>.yaml fragments.",
        target.definition.name
    );
    Ok(())
}

/// Write a scaffold file, creating parent directories but refusing to overwrite an existing file.
fn write_new(path: &Path, contents: &str) -> Result<(), AppError> {
    if path.exists() {
        return Err(AppError::Message(format!(
            "refusing to overwrite existing {}",
            path.display()
        )));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| AppError::Message(format!("create {}: {e}", parent.display())))?;
    }
    fs::write(path, contents)
        .map_err(|e| AppError::Message(format!("write {}: {e}", path.display())))?;
    println!("  created {}", path.display());
    Ok(())
}

const TAILOR_MANIFEST: &str = "\
# tailor.yaml — the workspace root (Cargo-style). `tailor` walks up from any subdirectory to find
# this file; every `*/image.yaml` beneath it is an auto-discovered member image. Repo-wide settings
# live here: the Image Customizer toolchain(s) and the defaults every image inherits.
schemaVersion: 1

toolchains:
  default: ic
  entries:
    - name: ic
      container: mcr.microsoft.com/azurelinux/imagecustomizer
      # version: \"1.3.0\"   # pin a specific Image Customizer; omit to track the :latest tag

defaults:
  outputs:
    - format: cosi
";

const BASE_IMAGE: &str = "\
# __IMAGE_NAME__/image.yaml — an image definition. Top-level keys are tailor's (name, base, …);
# everything under `config:` is Image Customizer config, passed through to IC untouched.
#
# The output format is inherited from the workspace `defaults:`, the arch defaults to `amd64` (declare
# an `arch` matrix axis or a base `arch:` to change it), and the default toolchain is used — so this
# file only declares what makes the image itself.
name: __IMAGE_NAME__

base:
  azureLinux:
    version: \"3.0\"
    variant: minimal-os

config:
  previewFeatures:
    - input-image-oci   # lets IC download the azureLinux/oci base
  os:
    hostname: __IMAGE_NAME__
    bootloader:
      resetType: hard-reset
    packages:
      install:
        - openssh-server
    services:
      enable:
        - sshd
  # minimal-os ships tight on free space, so grow the rootfs for the package install above.
  # (Repartitioning here is why os.bootloader.resetType: hard-reset is required.)
  storage:
    bootType: efi
    disks:
      - partitionTableType: gpt
        maxSize: 4G
        partitions:
          - id: esp
            type: esp
            size: 8M
          - id: rootfs
            size: grow
    filesystems:
      - deviceId: esp
        type: fat32
        mountPoint:
          path: /boot/efi
          options: \"umask=0077\"
      - deviceId: rootfs
        type: ext4
        mountPoint:
          path: /
";

const SIMPLE_IMAGE: &str = "\
# image.yaml — a standalone image (there is no tailor.yaml). This single file is the whole
# definition. Top-level keys are tailor's; everything under `config:` is Image Customizer config.
#
# With no toolchain pinned, tailor uses its built-in default Image Customizer (the :latest tag).
name: __IMAGE_NAME__

outputs:
  - format: cosi

base:
  azureLinux:
    version: \"3.0\"
    variant: minimal-os

config:
  previewFeatures:
    - input-image-oci   # lets IC download the azureLinux/oci base
  os:
    hostname: __IMAGE_NAME__
    bootloader:
      resetType: hard-reset
    packages:
      install:
        - openssh-server
    services:
      enable:
        - sshd
  # minimal-os ships tight on free space, so grow the rootfs for the package install above.
  # (Repartitioning here is why os.bootloader.resetType: hard-reset is required.)
  storage:
    bootType: efi
    disks:
      - partitionTableType: gpt
        maxSize: 4G
        partitions:
          - id: esp
            type: esp
            size: 8M
          - id: rootfs
            size: grow
    filesystems:
      - deviceId: esp
        type: fat32
        mountPoint:
          path: /boot/efi
          options: \"umask=0077\"
      - deviceId: rootfs
        type: ext4
        mountPoint:
          path: /
";

const ADVANCED_IMAGE: &str = "\
# __IMAGE_NAME__/image.yaml — a MATRIX image. The `matrix:` block multiplies its axes into one build
# per combination (here variant × arch = 4 cells). Per-axis deltas live in `by-<axis>/<value>.yaml`
# and merge on top of this shared config (lists append, maps deep-merge).
#
# IMPORTANT: fragments apply in the order the axes are DECLARED below — later axes win conflicts and
# append last — so you control merge precedence by ordering axes here, not by directory names.
name: __IMAGE_NAME__

matrix:
  variant: [minimal, full]
  arch:    [amd64, arm64]

outputs:
  - format: cosi

base:
  # An azureLinux (MCR) base is a multi-arch manifest, so one entry covers both arches; tailor
  # resolves the per-arch digest at pull time. (Use by-arch/<arch>.yaml `base:` for per-arch bases.)
  azureLinux:
    version: \"3.0\"
    variant: minimal-os

config:
  previewFeatures:
    - input-image-oci   # lets IC download the azureLinux/oci base
  os:
    hostname: __IMAGE_NAME__
    bootloader:
      resetType: hard-reset
    packages:
      install:
        - openssh-server
        # ${efiArch} is a parameter set per-arch in by-arch/<arch>.yaml (x64 / aa64).
        - \"grub2-efi-${efiArch}\"
  # minimal-os ships tight on free space, so grow the rootfs for the package install above.
  # (Repartitioning here is why os.bootloader.resetType: hard-reset is required.)
  storage:
    bootType: efi
    disks:
      - partitionTableType: gpt
        maxSize: 4G
        partitions:
          - id: esp
            type: esp
            size: 8M
          - id: rootfs
            size: grow
    filesystems:
      - deviceId: esp
        type: fat32
        mountPoint:
          path: /boot/efi
          options: \"umask=0077\"
      - deviceId: rootfs
        type: ext4
        mountPoint:
          path: /
";

const BY_VARIANT_MINIMAL: &str = "\
# Applies to cells where variant=minimal. Deltas here merge on top of the shared config in image.yaml.
config:
  os:
    packages:
      install:
        - vim-minimal
";

const BY_VARIANT_FULL: &str = "\
# Applies to cells where variant=full.
config:
  os:
    packages:
      install:
        - vim
        - git
";

const BY_ARCH_AMD64: &str = "\
# Applies to cells where arch=amd64. `params:` are scalars you can interpolate into config with
# ${...} (here ${efiArch}, referenced by image.yaml's package list).
params:
  efiArch: x64
";

const BY_ARCH_ARM64: &str = "\
# Applies to cells where arch=arm64.
params:
  efiArch: aa64
";

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        time::{Duration, UNIX_EPOCH},
    };

    use tailor_config::{AzureLinuxBase, BaseSource, OciBase, defaults::default_tool_config};
    use tailor_core::{
        ContainerConfig, ContainerResult, DaemonInfo, ExecError, LocalImage, testing::FakeResolver,
    };
    use tokio_util::sync::CancellationToken;

    #[derive(Debug, Clone, Default)]
    struct MockRuntime {
        image: Option<LocalImage>,
    }

    impl ContainerRuntime for MockRuntime {
        async fn pull_image(&self, _reference: &str) -> Result<(), ExecError> {
            Ok(())
        }

        async fn inspect_image(&self, _reference: &str) -> Result<Option<LocalImage>, ExecError> {
            Ok(self.image.clone())
        }

        async fn create_and_run(
            &self,
            _config: ContainerConfig,
            _cancel: CancellationToken,
        ) -> Result<ContainerResult, ExecError> {
            Err(ExecError::Other("not used".to_owned()))
        }

        async fn daemon_info(&self) -> Result<DaemonInfo, ExecError> {
            Ok(DaemonInfo::default())
        }

        async fn export_container(
            &self,
            _image_ref: &str,
            _platform: &str,
            _pull: bool,
            _dest_dir: &Path,
            _cancel: CancellationToken,
        ) -> Result<(), ExecError> {
            Err(ExecError::Other("not used".to_owned()))
        }
    }

    fn local_entry(container: &str, pull: PullPolicy) -> ToolchainEntry {
        ToolchainEntry {
            name: "ic".to_owned(),
            container: container.to_owned(),
            version: None,
            tag: Some("local".to_owned()),
            pull,
        }
    }

    fn local_image(repo_digests: Vec<String>) -> LocalImage {
        LocalImage {
            id: "sha256:localid".to_owned(),
            repo_digests,
            architecture: "amd64".to_owned(),
            os: "linux".to_owned(),
        }
    }

    fn local_image_with_arch(architecture: &str) -> LocalImage {
        LocalImage {
            architecture: architecture.to_owned(),
            ..local_image(Vec::new())
        }
    }

    #[tokio::test]
    async fn missing_uses_local_toolchain_repo_digest_without_pull() {
        let runtime = MockRuntime {
            image: Some(local_image(vec![
                "registry.example/ic@sha256:localdigest".to_owned(),
            ])),
        };

        let resolved = resolve_toolchain_ref(
            "ic",
            &local_entry("registry.example/ic", PullPolicy::Missing),
            &runtime,
            &FakeResolver,
            &Lockfile::default(),
        )
        .await
        .unwrap();

        assert_eq!(resolved.image_ref, "registry.example/ic@sha256:localdigest");
        assert_eq!(resolved.lock_digest.as_deref(), Some("sha256:localdigest"));
        assert!(!resolved.pull);
    }

    #[tokio::test]
    async fn missing_uses_local_toolchain_id_without_locking() {
        let runtime = MockRuntime {
            image: Some(local_image(Vec::new())),
        };

        let resolved = resolve_toolchain_ref(
            "ic",
            &local_entry("acl-imagecustomizer", PullPolicy::Missing),
            &runtime,
            &FakeResolver,
            &Lockfile::default(),
        )
        .await
        .unwrap();

        assert_eq!(resolved.image_ref, "sha256:localid");
        assert_eq!(resolved.lock_digest, None);
        assert_eq!(resolved.architecture.as_deref(), Some("amd64"));
        assert!(!resolved.pull);
    }

    #[tokio::test]
    async fn missing_prefers_a_locked_digest_over_the_local_copy() {
        let runtime = MockRuntime {
            image: Some(local_image(vec![
                "registry.example/ic@sha256:localdigest".to_owned(),
            ])),
        };
        let mut lock = Lockfile::default();
        lock.toolchains.insert(
            "ic".to_owned(),
            LockedContainer {
                container: "registry.example/ic".to_owned(),
                version: None,
                tag: Some("local".to_owned()),
                digest: "sha256:locked".to_owned(),
            },
        );

        let resolved = resolve_toolchain_ref(
            "ic",
            &local_entry("registry.example/ic", PullPolicy::Missing),
            &runtime,
            &FakeResolver,
            &lock,
        )
        .await
        .unwrap();

        assert_eq!(resolved.image_ref, "registry.example/ic@sha256:locked");
        assert!(resolved.pull);
    }

    #[tokio::test]
    async fn missing_resolves_absent_toolchain_from_registry() {
        let runtime = MockRuntime::default();

        let resolved = resolve_toolchain_ref(
            "ic",
            &local_entry("registry.example/ic", PullPolicy::Missing),
            &runtime,
            &FakeResolver,
            &Lockfile::default(),
        )
        .await
        .unwrap();

        assert_eq!(
            resolved.image_ref,
            "registry.example/ic@sha256:faketoolchain"
        );
        assert_eq!(
            resolved.lock_digest.as_deref(),
            Some("sha256:faketoolchain")
        );
        assert!(resolved.pull);
    }

    #[tokio::test]
    async fn never_errors_when_toolchain_is_absent_locally() {
        let runtime = MockRuntime::default();

        let err = resolve_toolchain_ref(
            "ic",
            &local_entry("acl-imagecustomizer", PullPolicy::Never),
            &runtime,
            &FakeResolver,
            &Lockfile::default(),
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("not found locally and pull policy is never"),
            "got {err:?}"
        );
    }

    #[tokio::test]
    async fn tools_dir_missing_uses_local_image_id_without_locking() {
        let runtime = MockRuntime {
            image: Some(local_image(Vec::new())),
        };
        let source = ToolsDirSourceInline {
            container: "acl-tools".to_owned(),
            tag: Some("local".to_owned()),
            pull: PullPolicy::Missing,
        };

        let resolved = resolve_tools_dir_ref(
            Some("acl"),
            &source,
            &runtime,
            &FakeResolver,
            &Lockfile::default(),
        )
        .await
        .unwrap();

        assert_eq!(resolved.image_ref, "sha256:localid");
        assert_eq!(resolved.lock_digest, None);
        assert!(!resolved.pull);
    }

    #[tokio::test]
    async fn build_lock_skips_local_image_ids_without_repo_digests() {
        let runtime = MockRuntime {
            image: Some(local_image(Vec::new())),
        };
        let tool: ToolConfig = serde_yaml_ng::from_str(
            r"
schemaVersion: 1
toolchains:
  default: ic
  entries:
    - name: ic
      container: acl-imagecustomizer
      tag: local
      pull: never
toolsDirSources:
  - name: acl
    container: acl-tools
    tag: local
    pull: never
",
        )
        .unwrap();

        let lock = build_lock(
            &tool,
            &[],
            &runtime,
            &OciResolver::new(),
            &Lockfile::default(),
        )
        .await
        .unwrap();

        assert!(lock.toolchains.is_empty());
        assert!(lock.tools_dirs.is_empty());
    }

    #[test]
    fn timestamp_mode_defaults_to_elapsed() {
        assert!(matches!(TimestampMode::default(), TimestampMode::Elapsed));
    }

    #[test]
    fn timestamp_prefix_renders_elapsed_time_and_off() {
        const ELAPSED_SAMPLE_MS: u64 = 10_290;
        const ONE_HOUR_TWO_MINUTES_THREE_SECONDS: u64 = 3_723;

        assert_eq!(
            timestamp_prefix_for(
                TimestampMode::Elapsed,
                Duration::from_millis(ELAPSED_SAMPLE_MS),
                UNIX_EPOCH
            ),
            "10.29s"
        );
        assert_eq!(
            timestamp_prefix_for(
                TimestampMode::Time,
                Duration::default(),
                UNIX_EPOCH + Duration::from_secs(ONE_HOUR_TWO_MINUTES_THREE_SECONDS)
            ),
            "01:02:03"
        );
        assert_eq!(
            timestamp_prefix_for(TimestampMode::Off, Duration::default(), UNIX_EPOCH),
            ""
        );
    }

    #[tokio::test]
    async fn local_toolchain_arch_preflight_rejects_mismatch() {
        let workspace = discover(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("matrix"),
        )
        .unwrap();
        let tool = tool_config(&workspace);
        let targets = build_targets(&workspace, &[]).unwrap();
        let runtime = MockRuntime {
            image: Some(local_image_with_arch("amd64")),
        };
        let toolchains = resolve_toolchains(
            &targets,
            &tool,
            &runtime,
            &FakeResolver,
            &Lockfile::default(),
        )
        .await
        .unwrap();
        let selection = Selector::parse(&[], &[], &["arm64".to_owned()]).unwrap();

        let err = preflight_toolchain_arches(&targets, &tool, &toolchains, &selection).unwrap_err();

        assert!(
            err.to_string().contains(
                "toolchain `ic` local image is `amd64` but cell `gizmo_lite_arm64_stable_cosi` targets `arm64`"
            ),
            "got {err:?}"
        );
    }

    #[test]
    fn local_toolchain_arch_preflight_allows_matching_arch() {
        preflight_toolchain_cell_arch("ic", Some("arm64"), Arch::Arm64, "app_arm64_cosi").unwrap();
    }

    #[test]
    fn local_toolchain_arch_preflight_ignores_registry_toolchains() {
        preflight_toolchain_cell_arch("ic", None, Arch::Arm64, "app_arm64_cosi").unwrap();
    }

    #[test]
    fn status_line_right_aligns_and_colors_the_verb() {
        // Plain: 12-wide right-aligned verb, then the message.
        assert_eq!(
            render_status_line("", "Built", "img.cosi (1.0 MiB)", false),
            "       Built img.cosi (1.0 MiB)"
        );
        assert_eq!(
            render_status_line("10.29s", "Customizing", "x", false),
            "10.29s   Customizing x"
        );
        // Colored: a bold-green verb wrapped in ANSI, padding still outside the codes.
        assert_eq!(
            render_status_line("", "Built", "x", true),
            "       \u{1b}[1;32mBuilt\u{1b}[0m x"
        );
    }

    #[test]
    fn format_bytes_scales_units() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KiB");
        assert_eq!(format_bytes(5 * 1024 * 1024), "5.0 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn format_duration_switches_to_minutes() {
        assert_eq!(format_duration(Duration::from_secs_f64(34.0)), "34.0s");
        assert_eq!(format_duration(Duration::from_secs(72)), "1m12s");
        assert_eq!(format_duration(Duration::from_secs(605)), "10m05s");
    }

    #[test]
    fn resolve_log_dir_honors_precedence_flag_then_env_then_manifest() {
        let flag = || Some(PathBuf::from("/flag"));
        let env = || Some(PathBuf::from("/env"));
        let manifest = || Some(PathBuf::from("/manifest"));

        // Flag wins over everything.
        assert_eq!(
            resolve_log_dir(flag(), env(), manifest()),
            Some(PathBuf::from("/flag"))
        );
        // Env wins when there is no flag.
        assert_eq!(
            resolve_log_dir(None, env(), manifest()),
            Some(PathBuf::from("/env"))
        );
        // Manifest is the lowest-precedence opt-in.
        assert_eq!(
            resolve_log_dir(None, None, manifest()),
            Some(PathBuf::from("/manifest"))
        );
    }

    #[test]
    fn resolve_log_dir_off_by_default_when_nothing_is_set() {
        assert_eq!(resolve_log_dir(None, None, None), None);
    }

    #[test]
    fn apply_log_overrides_leaves_persistence_off_without_opt_in() {
        let mut tool = default_tool_config();
        apply_log_overrides(&mut tool, &LogOverrides::default(), Path::new("/ws")).unwrap();
        // No flag/env/manifest log dir → persistence stays off (no runtime.log_dir).
        assert!(
            tool.runtime
                .as_ref()
                .and_then(|r| r.log_dir.clone())
                .is_none()
        );
    }

    #[test]
    fn apply_log_overrides_threads_the_flag_log_dir_and_ic_level() {
        let mut tool = default_tool_config();
        let logging = LogOverrides {
            log_dir: Some(PathBuf::from("/flag/logs")),
            ic_log_level: Some(tailor_config::LogLevel::Trace),
        };
        apply_log_overrides(&mut tool, &logging, Path::new("/ws")).unwrap();
        let runtime = tool.runtime.as_ref().expect("runtime created");
        assert_eq!(runtime.log_dir.as_deref(), Some(Path::new("/flag/logs")));
        assert_eq!(runtime.log_level, Some(tailor_config::LogLevel::Trace));
    }

    #[test]
    fn apply_log_overrides_absolutizes_a_relative_manifest_log_dir() {
        // A relative `runtime.logDir` must be absolutized against the workspace root — never left
        // relative, or path translation turns it into `/host/./…` (the host root's dir).
        let mut tool = default_tool_config();
        tool.runtime = Some(Runtime {
            log_dir: Some(PathBuf::from("./.tailor/logs")),
            ..Runtime::default()
        });

        apply_log_overrides(&mut tool, &LogOverrides::default(), Path::new("/ws")).unwrap();

        let log_dir = tool.runtime.and_then(|r| r.log_dir).expect("log dir set");
        assert!(log_dir.is_absolute(), "expected absolute, got {log_dir:?}");
        assert_eq!(log_dir, Path::new("/ws/.tailor/logs"));
    }

    #[test]
    fn apply_log_overrides_absolutizes_a_relative_flag_log_dir_against_cwd() {
        // The `--log-dir` flag resolves against the CWD (not the workspace) and must be absolutized.
        let mut tool = default_tool_config();
        let logging = LogOverrides {
            log_dir: Some(PathBuf::from("mylogs")),
            ic_log_level: None,
        };

        apply_log_overrides(&mut tool, &logging, Path::new("/ws")).unwrap();

        let log_dir = tool.runtime.and_then(|r| r.log_dir).expect("log dir set");
        let expected = env::current_dir().unwrap().join("mylogs");
        assert_eq!(log_dir, expected);
    }

    #[test]
    fn build_dir_base_override_absent_leaves_runtime_untouched() {
        let mut tool = default_tool_config();
        apply_build_dir_base_override(&mut tool, None).unwrap();
        assert!(tool.runtime.is_none(), "no flag → runtime not created");
    }

    #[test]
    fn build_dir_base_override_sets_an_absolute_path() {
        let mut tool = default_tool_config();
        apply_build_dir_base_override(&mut tool, Some(Path::new("/mnt/pool/scratch"))).unwrap();
        assert_eq!(
            tool.runtime
                .and_then(|runtime| runtime.build_dir_base)
                .as_deref(),
            Some(Path::new("/mnt/pool/scratch"))
        );
    }

    #[test]
    fn build_dir_base_override_absolutizes_a_relative_flag_against_cwd() {
        let mut tool = default_tool_config();
        apply_build_dir_base_override(&mut tool, Some(Path::new("pool-scratch"))).unwrap();
        let expected = env::current_dir().unwrap().join("pool-scratch");
        assert_eq!(
            tool.runtime
                .and_then(|runtime| runtime.build_dir_base)
                .as_deref(),
            Some(expected.as_path())
        );
    }

    #[test]
    fn describe_base_covers_every_source() {
        assert_eq!(
            describe_base(&BaseSource::Path {
                path: PathBuf::from("./base.img"),
                arch: None,
            }),
            "path: ./base.img"
        );
        assert_eq!(
            describe_base(&BaseSource::Oci {
                oci: OciBase {
                    uri: "registry.example/base:edge".to_owned(),
                    platform: None,
                }
            }),
            "oci: registry.example/base:edge"
        );
        assert_eq!(
            describe_base(&BaseSource::AzureLinux {
                azure_linux: AzureLinuxBase {
                    version: "3.0".to_owned(),
                    variant: "minimal-os".to_owned(),
                }
            }),
            "azureLinux 3.0/minimal-os"
        );
        assert_eq!(
            describe_base(&BaseSource::Ref {
                reference: "baremetal".to_owned()
            }),
            "ref: baremetal"
        );
    }

    #[test]
    fn matrix_entry_emits_base_image_only_when_bound() {
        let bound = MatrixEntry {
            image: "mini".to_owned(),
            slug: "mini_amd64_cosi".to_owned(),
            axes: BTreeMap::new(),
            format: "cosi".to_owned(),
            base_image: Some("baremetal".to_owned()),
        };
        assert!(
            serde_json::to_string(&bound)
                .unwrap()
                .contains("\"baseImage\":\"baremetal\"")
        );
        let unbound = MatrixEntry {
            base_image: None,
            ..bound
        };
        assert!(
            !serde_json::to_string(&unbound)
                .unwrap()
                .contains("baseImage")
        );
    }

    #[test]
    fn describe_toolchains_uses_the_default_entry() {
        let tool = default_tool_config();
        assert_eq!(
            describe_toolchains(&tool),
            "mcr.microsoft.com/azurelinux/imagecustomizer:latest"
        );
    }

    #[test]
    fn resolve_image_cache_dir_defaults_absent_and_absolutizes_relative_against_workspace() {
        let mut tool = default_tool_config();
        resolve_image_cache_dir(&mut tool, Path::new("/ws"));
        assert_eq!(
            tool.runtime.as_ref().unwrap().image_cache_dir.as_deref(),
            Some(Path::new("/ws/.tailor/cache"))
        );

        // A manifest-relative value is absolutized against the workspace root (so it never reaches
        // Docker as a relative bind source like `./.tailor/cache`).
        let mut relative = default_tool_config();
        relative.runtime = Some(Runtime {
            image_cache_dir: Some(PathBuf::from("./.tailor/cache")),
            ..Default::default()
        });
        resolve_image_cache_dir(&mut relative, Path::new("/ws"));
        assert_eq!(
            relative.runtime.unwrap().image_cache_dir.as_deref(),
            Some(Path::new("/ws/.tailor/cache"))
        );

        // An absolute value is left untouched.
        let mut configured = default_tool_config();
        configured.runtime = Some(Runtime {
            image_cache_dir: Some(PathBuf::from("/custom/cache")),
            ..Default::default()
        });
        resolve_image_cache_dir(&mut configured, Path::new("/ws"));
        assert_eq!(
            configured.runtime.unwrap().image_cache_dir.as_deref(),
            Some(Path::new("/custom/cache"))
        );
    }
}
