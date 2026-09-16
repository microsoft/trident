use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use tailor_config::{Access, BaseSource, ExtraParam, Operation};
use tailor_core::{Cell, ExecError, ExecutionContext, RuntimeConfig, artifact_name, output_slug};

use crate::{guard, path_translate, working_copy};

const CONTAINER_TMP_BUILD_DIR: &str = "/tmp";
const DOCKER: &str = "docker";
const DOCKER_RUN: &str = "run";
const FLAG_RM: &str = "--rm";
const FLAG_PRIVILEGED: &str = "--privileged";
const FLAG_PLATFORM: &str = "--platform";
const FLAG_VOLUME: &str = "-v";
pub(crate) const DEV_BIND: &str = "/dev:/dev";
const FLAG_BUILD_DIR: &str = "--build-dir";
const FLAG_CONFIG_FILE: &str = "--config-file";
const FLAG_COSI_COMPRESSION_LEVEL: &str = "--cosi-compression-level";
const FLAG_IMAGE: &str = "--image";
const FLAG_IMAGE_CACHE_DIR: &str = "--image-cache-dir";
const FLAG_IMAGE_FILE: &str = "--image-file";
const FLAG_LOG_LEVEL: &str = "--log-level";
const FLAG_LOG_FORMAT: &str = "--log-format";
const FLAG_LOG_COLOR: &str = "--log-color";
const FLAG_LOG_FILE: &str = "--log-file";
const LOG_FORMAT_JSON: &str = "json";
const LOG_COLOR_NEVER: &str = "never";
/// IC's `--log-level` default when the manifest/flags set none: run IC at full `debug` so the
/// in-memory capture and any on-disk log always have the full story (`meta/docs/2026-06-29-logging.md` §5.1).
const DEFAULT_IC_LOG_LEVEL: &str = "debug";
const FLAG_OUTPUT_IMAGE_FILE: &str = "--output-image-file";
const FLAG_OUTPUT_IMAGE_FORMAT: &str = "--output-image-format";
const FLAG_RPM_SOURCE: &str = "--rpm-source";
const FLAG_TOOLS_DIR: &str = "--tools-dir";
const OCI_PREFIX: &str = "oci:";
const ACCESS_READ_ONLY: &str = "ro";
const ACCESS_READ_WRITE: &str = "rw";
const SUBCOMMAND_CONVERT: &str = "convert";
const SUBCOMMAND_CUSTOMIZE: &str = "customize";
const SUBCOMMAND_INJECT_FILES: &str = "inject-files";
/// The intermediate image format a signed `customize` writes; the final format is produced by the
/// `inject-files` pass (`meta/docs/2026-06-29-signing.md` §5).
const OUTPUT_FORMAT_RAW: &str = "raw";

pub fn build_ic_args(cell: &Cell, context: &ExecutionContext) -> Result<Vec<String>, ExecError> {
    let operation = cell
        .target
        .definition
        .operation
        .unwrap_or(Operation::Customize);
    let mut args = vec![subcommand(operation).to_owned()];

    if operation == Operation::Customize {
        args.extend(flag_value(
            FLAG_CONFIG_FILE,
            path_translate::to_container_path(
                &working_copy::working_copy_path(cell, context.clone_index),
                &context.runtime.host_root,
            ),
        ));
    }

    // Always run IC structured (`meta/docs/2026-06-29-logging.md` §5.1): JSON on stderr, no ANSI, at `debug`
    // (or the configured level) so the in-memory capture and any on-disk log have the full story.
    args.extend(flag_value(FLAG_LOG_FORMAT, LOG_FORMAT_JSON.to_owned()));
    args.extend(flag_value(FLAG_LOG_COLOR, LOG_COLOR_NEVER.to_owned()));
    args.extend(flag_value(
        FLAG_LOG_LEVEL,
        context
            .runtime
            .log_level
            .clone()
            .unwrap_or_else(|| DEFAULT_IC_LOG_LEVEL.to_owned()),
    ));
    // Opt-in on-disk persistence (§5.5): write the per-cell debug log into the container path that
    // maps to `<log-dir>/<slug>.log` on the host.
    if let Some(log_file) = log_file_path(cell, context) {
        args.extend(flag_value(
            FLAG_LOG_FILE,
            path_translate::to_container_path(&log_file, &context.runtime.host_root),
        ));
    }

    args.extend(flag_value(
        FLAG_BUILD_DIR,
        build_dir_arg_value(cell, context)?,
    ));
    args.extend(base_args(
        &cell.base,
        context.base_ref.as_deref(),
        &cell.target.dir,
        &context.runtime,
        operation,
    ));
    args.extend(flag_value(
        FLAG_OUTPUT_IMAGE_FORMAT,
        cell.output.format.as_str().to_owned(),
    ));
    args.extend(flag_value(
        FLAG_OUTPUT_IMAGE_FILE,
        path_translate::to_container_path(
            &artifact_path(cell, context),
            &context.runtime.host_root,
        ),
    ));

    if let Some(level) = cell.output.cosi_compression_level {
        args.extend(flag_value(FLAG_COSI_COMPRESSION_LEVEL, level.to_string()));
    }

    if operation == Operation::Customize {
        push_tools_dir_arg(&mut args, context);
        for source in &cell.rpm_sources {
            args.extend(flag_value(
                FLAG_RPM_SOURCE,
                path_translate::to_container_path(
                    &tailor_config::absolutize(source, &cell.target.dir),
                    &context.runtime.host_root,
                ),
            ));
        }
    }

    if let Some(cache_dir) = &context.runtime.image_cache_dir {
        args.extend(flag_value(
            FLAG_IMAGE_CACHE_DIR,
            path_translate::to_container_path(cache_dir, &context.runtime.host_root),
        ));
    }

    push_extra_params(&mut args, &cell.extra_params)?;

    Ok(args)
}

/// The `customize` pass of a signed build: identical to a normal customize except it writes a **raw
/// intermediate** (`<slug>.intermediate.raw`) with no cosi compression — the final format is produced
/// by the `inject-files` pass (`meta/docs/2026-06-29-signing.md` §5). The user's relocated `output.artifacts`
/// still rides along in the working copy, so IC extracts the boot artifacts + `inject-files.yaml`.
pub(crate) fn build_signed_customize_args(
    cell: &Cell,
    context: &ExecutionContext,
    intermediate: &Path,
) -> Result<Vec<String>, ExecError> {
    let mut args = vec![SUBCOMMAND_CUSTOMIZE.to_owned()];
    args.extend(flag_value(
        FLAG_CONFIG_FILE,
        path_translate::to_container_path(
            &working_copy::working_copy_path(cell, context.clone_index),
            &context.runtime.host_root,
        ),
    ));
    push_log_flags(&mut args, cell, context);
    args.extend(flag_value(
        FLAG_BUILD_DIR,
        build_dir_arg_value(cell, context)?,
    ));
    args.extend(base_args(
        &cell.base,
        context.base_ref.as_deref(),
        &cell.target.dir,
        &context.runtime,
        Operation::Customize,
    ));
    args.extend(flag_value(
        FLAG_OUTPUT_IMAGE_FORMAT,
        OUTPUT_FORMAT_RAW.to_owned(),
    ));
    args.extend(flag_value(
        FLAG_OUTPUT_IMAGE_FILE,
        path_translate::to_container_path(intermediate, &context.runtime.host_root),
    ));
    push_tools_dir_arg(&mut args, context);
    for source in &cell.rpm_sources {
        args.extend(flag_value(
            FLAG_RPM_SOURCE,
            path_translate::to_container_path(
                &tailor_config::absolutize(source, &cell.target.dir),
                &context.runtime.host_root,
            ),
        ));
    }
    if let Some(cache_dir) = &context.runtime.image_cache_dir {
        args.extend(flag_value(
            FLAG_IMAGE_CACHE_DIR,
            path_translate::to_container_path(cache_dir, &context.runtime.host_root),
        ));
    }
    push_extra_params(&mut args, &cell.extra_params)?;
    Ok(args)
}

fn push_tools_dir_arg(args: &mut Vec<String>, context: &ExecutionContext) {
    let Some(tools_dir) = &context.tools_dir else {
        return;
    };
    let value = path_translate::to_container_path(&tools_dir.mount_dir, &context.runtime.host_root);
    if value != "/" {
        args.extend(flag_value(FLAG_TOOLS_DIR, value));
    }
}

/// The `inject-files` pass of a signed build: re-inject the now-signed artifacts from
/// `inject-files.yaml` into the raw intermediate and produce the cell's **final** output format
/// (`meta/docs/2026-06-29-signing.md` §5). `cosiCompressionLevel` applies only here.
pub(crate) fn build_inject_files_args(
    cell: &Cell,
    context: &ExecutionContext,
    intermediate: &Path,
    inject_files_yaml: &Path,
    final_image: &Path,
) -> Result<Vec<String>, ExecError> {
    let translate =
        |path: &Path| path_translate::to_container_path(path, &context.runtime.host_root);
    let mut args = vec![SUBCOMMAND_INJECT_FILES.to_owned()];
    args.extend(flag_value(
        FLAG_BUILD_DIR,
        build_dir_arg_value(cell, context)?,
    ));
    // `inject-files` reuses `--config-file` for the inject-files.yaml manifest.
    args.extend(flag_value(FLAG_CONFIG_FILE, translate(inject_files_yaml)));
    args.extend(flag_value(FLAG_IMAGE_FILE, translate(intermediate)));
    push_log_flags(&mut args, cell, context);
    args.extend(flag_value(
        FLAG_OUTPUT_IMAGE_FORMAT,
        cell.output.format.as_str().to_owned(),
    ));
    args.extend(flag_value(FLAG_OUTPUT_IMAGE_FILE, translate(final_image)));
    if let Some(level) = cell.output.cosi_compression_level {
        args.extend(flag_value(FLAG_COSI_COMPRESSION_LEVEL, level.to_string()));
    }
    Ok(args)
}

/// IC's structured-logging flags (`meta/docs/2026-06-29-logging.md` §5.1), shared by every pass.
fn push_log_flags(args: &mut Vec<String>, cell: &Cell, context: &ExecutionContext) {
    args.extend(flag_value(FLAG_LOG_FORMAT, LOG_FORMAT_JSON.to_owned()));
    args.extend(flag_value(FLAG_LOG_COLOR, LOG_COLOR_NEVER.to_owned()));
    args.extend(flag_value(
        FLAG_LOG_LEVEL,
        context
            .runtime
            .log_level
            .clone()
            .unwrap_or_else(|| DEFAULT_IC_LOG_LEVEL.to_owned()),
    ));
    if let Some(log_file) = log_file_path(cell, context) {
        args.extend(flag_value(
            FLAG_LOG_FILE,
            path_translate::to_container_path(&log_file, &context.runtime.host_root),
        ));
    }
}

/// The host path of a signed build's raw intermediate image, `<output_dir>/<slug>.intermediate.raw`
/// (clone-suffixed so clones never collide).
pub(crate) fn intermediate_path(cell: &Cell, context: &ExecutionContext) -> PathBuf {
    let slug = output_slug(cell.slug.as_ref(), context.clone_index);
    context.output_dir.join(format!("{slug}.intermediate.raw"))
}
pub(crate) fn artifact_path(cell: &Cell, context: &ExecutionContext) -> PathBuf {
    let slug = output_slug(cell.slug.as_ref(), context.clone_index);
    context
        .output_dir
        .join(artifact_name(&slug, cell.output.format))
}

/// The host path of the per-cell IC log when on-disk persistence is enabled, else `None`
/// (`meta/docs/2026-06-29-logging.md` §5.5). A `--clones` run suffixes the slug so clones never collide.
pub(crate) fn log_file_path(cell: &Cell, context: &ExecutionContext) -> Option<PathBuf> {
    context.runtime.log_dir.as_ref().map(|dir| {
        let name = match context.clone_index {
            Some(clone) => format!("{}_clone{clone}.log", cell.slug.as_ref()),
            None => format!("{}.log", cell.slug.as_ref()),
        };
        dir.join(name)
    })
}

fn subcommand(operation: Operation) -> &'static str {
    match operation {
        Operation::Customize => SUBCOMMAND_CUSTOMIZE,
        Operation::Convert => SUBCOMMAND_CONVERT,
    }
}

fn base_args(
    base: &BaseSource,
    base_ref: Option<&str>,
    image_dir: &std::path::Path,
    runtime: &RuntimeConfig,
    operation: Operation,
) -> Vec<String> {
    match (base, operation) {
        (BaseSource::Path { path, .. }, _) => flag_value(
            FLAG_IMAGE_FILE,
            path_translate::to_container_path(
                &tailor_config::absolutize(path, image_dir),
                &runtime.host_root,
            ),
        ),
        // A registry base: pass the digest-pinned `oci:<repo>@<digest>` resolved during planning
        // (reproducible). `--dry-run` resolves no digest, so fall back to the un-pinned reference for
        // the preview only.
        (BaseSource::Oci { oci }, Operation::Customize) => flag_value(
            FLAG_IMAGE,
            base_ref.map_or_else(|| format!("{OCI_PREFIX}{}", oci.uri), ToOwned::to_owned),
        ),
        (BaseSource::AzureLinux { azure_linux }, Operation::Customize) => flag_value(
            FLAG_IMAGE,
            base_ref.map_or_else(
                || {
                    format!(
                        "{OCI_PREFIX}mcr.microsoft.com/azurelinux/{}/image/{}",
                        azure_linux.version, azure_linux.variant
                    )
                },
                ToOwned::to_owned,
            ),
        ),
        (BaseSource::Oci { oci }, Operation::Convert) => {
            flag_value(FLAG_IMAGE_FILE, oci.uri.clone())
        }
        (BaseSource::AzureLinux { azure_linux }, Operation::Convert) => flag_value(
            FLAG_IMAGE_FILE,
            format!(
                "mcr.microsoft.com/azurelinux/{}/image/{}",
                azure_linux.version, azure_linux.variant
            ),
        ),
        // A catalogue reference is collapsed to a `path` base during cell expansion, so it never
        // reaches the argument builder; treat its absolute path like a `path` base if it does.
        (BaseSource::Ref { reference }, _) => flag_value(FLAG_IMAGE_FILE, reference.clone()),
        // `base: { image }` is lowered to a `path` base before the build (orchestrator), so it never
        // reaches the argument builder.
        (BaseSource::Image { image, .. }, _) => {
            unreachable!(
                "base: {{ image: {image} }} must be lowered to a path base before arg building"
            )
        }
    }
}

fn flag_value(flag: &str, value: String) -> Vec<String> {
    vec![flag.to_owned(), value]
}

/// Flags tailor emits itself; an `extraParams` entry naming one is rejected so a user-supplied flag
/// can never silently duplicate or fight a tailor-managed argument.
const RESERVED_FLAGS: &[&str] = &[
    FLAG_CONFIG_FILE,
    FLAG_LOG_FORMAT,
    FLAG_LOG_COLOR,
    FLAG_LOG_LEVEL,
    FLAG_LOG_FILE,
    FLAG_BUILD_DIR,
    FLAG_OUTPUT_IMAGE_FORMAT,
    FLAG_OUTPUT_IMAGE_FILE,
    FLAG_COSI_COMPRESSION_LEVEL,
    FLAG_TOOLS_DIR,
    FLAG_RPM_SOURCE,
    FLAG_IMAGE_CACHE_DIR,
    FLAG_IMAGE,
    FLAG_IMAGE_FILE,
];

/// Append the image's `extraParams` verbatim after every tailor-managed flag: each entry becomes one
/// argv token — `--flag` alone, or `--flag=value` when a `value` is set. A flag tailor already
/// manages is rejected (`RESERVED_FLAGS`).
fn push_extra_params(args: &mut Vec<String>, extra_params: &[ExtraParam]) -> Result<(), ExecError> {
    for extra in extra_params {
        let flag = extra.param.split('=').next().unwrap_or(&extra.param).trim();
        if RESERVED_FLAGS.contains(&flag) {
            return Err(ExecError::ReservedParam {
                param: extra.param.clone(),
            });
        }
        match &extra.value {
            Some(value) => args.push(format!("{}={value}", extra.param)),
            None => args.push(extra.param.clone()),
        }
    }
    Ok(())
}

fn build_dir_arg_value(cell: &Cell, context: &ExecutionContext) -> Result<String, ExecError> {
    let Some(build_dir) = build_dir_path(cell, context) else {
        return Ok(CONTAINER_TMP_BUILD_DIR.to_owned());
    };
    guard::ensure_safe_build_dir(&build_dir)?;
    Ok(path_translate::to_container_path(
        &build_dir,
        &context.runtime.host_root,
    ))
}

pub(crate) fn build_dir_path(cell: &Cell, context: &ExecutionContext) -> Option<PathBuf> {
    context.runtime.build_dir_base.as_ref().map(|base| {
        absolute_source(
            &base.join(cell.slug.as_ref()),
            &context.runtime.workspace_root,
        )
    })
}

pub(crate) fn container_binds(
    cell: &Cell,
    context: &ExecutionContext,
    extra_rw: &[PathBuf],
) -> Result<Vec<String>, ExecError> {
    let mut binds = Vec::new();
    push_bind(
        &mut binds,
        &context.runtime.workspace_root,
        Access::Ro,
        context,
    )?;
    push_bind(&mut binds, &context.output_dir, Access::Rw, context)?;
    if let Some(cache_dir) = &context.runtime.image_cache_dir {
        push_bind(&mut binds, cache_dir, Access::Rw, context)?;
    }
    if let Some(build_dir) = build_dir_path(cell, context) {
        guard::ensure_safe_build_dir(&build_dir)?;
        push_bind(&mut binds, &build_dir, Access::Rw, context)?;
    }
    if let Some(log_dir) = &context.runtime.log_dir {
        push_bind(&mut binds, log_dir, Access::Rw, context)?;
    }
    if let Some(tools_dir) = &context.tools_dir {
        guard::ensure_safe_build_dir(&tools_dir.mount_dir)?;
        push_bind_keep(&mut binds, &tools_dir.mount_dir, Access::Rw, context)?;
    }
    for path in extra_rw {
        push_bind(&mut binds, path, Access::Rw, context)?;
    }
    push_out_of_workspace_inputs(&mut binds, cell, context)?;
    for mount in &context.runtime.extra_paths {
        let source = absolute_source(&mount.path, &context.runtime.workspace_root);
        if mount.access == Access::Rw {
            guard::ensure_safe_rw_target(&source)?;
        }
        push_bind(&mut binds, &source, mount.access, context)?;
    }

    let mut values = normalize_binds(binds, &context.runtime.host_root);
    if context.runtime.mount_dev {
        values.push(DEV_BIND.to_owned());
    }
    Ok(values)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BindSpec {
    source: PathBuf,
    access: Access,
    keep_nested: bool,
}

fn push_out_of_workspace_inputs(
    binds: &mut Vec<BindSpec>,
    cell: &Cell,
    context: &ExecutionContext,
) -> Result<(), ExecError> {
    let workspace = absolute_source(
        &context.runtime.workspace_root,
        &context.runtime.workspace_root,
    );
    if let BaseSource::Path { path, .. } = &cell.base {
        let base = absolute_source(path, &cell.target.dir);
        let parent = base.parent().ok_or_else(|| {
            ExecError::Other(format!("base path `{}` has no parent", base.display()))
        })?;
        if !base.starts_with(&workspace) {
            let source = if parent == Path::new("/") {
                base.as_path()
            } else {
                parent
            };
            push_bind(binds, source, Access::Ro, context)?;
        }
    }
    for source in &cell.rpm_sources {
        let source = absolute_source(source, &cell.target.dir);
        if !source.starts_with(&workspace) {
            push_bind(binds, &source, Access::Ro, context)?;
        }
    }
    Ok(())
}

fn push_bind(
    binds: &mut Vec<BindSpec>,
    source: &Path,
    access: Access,
    context: &ExecutionContext,
) -> Result<(), ExecError> {
    push_bind_inner(binds, source, access, context, false)
}

fn push_bind_keep(
    binds: &mut Vec<BindSpec>,
    source: &Path,
    access: Access,
    context: &ExecutionContext,
) -> Result<(), ExecError> {
    push_bind_inner(binds, source, access, context, true)
}

fn push_bind_inner(
    binds: &mut Vec<BindSpec>,
    source: &Path,
    access: Access,
    context: &ExecutionContext,
    keep_nested: bool,
) -> Result<(), ExecError> {
    let source = absolute_source(source, &context.runtime.workspace_root);
    if source == Path::new("/") {
        return Err(ExecError::UnsafeDir {
            path: source,
            reason: "must not bind the filesystem root into the IC container".to_owned(),
        });
    }
    if access == Access::Rw {
        guard::ensure_safe_rw_target(&source)?;
    }
    binds.push(BindSpec {
        source,
        access,
        keep_nested,
    });
    Ok(())
}

fn normalize_binds(mut binds: Vec<BindSpec>, host_root: &Path) -> Vec<String> {
    let mut by_source = BTreeMap::<PathBuf, (Access, bool)>::new();
    for bind in binds.drain(..) {
        by_source
            .entry(bind.source)
            .and_modify(|(access, keep_nested)| {
                if bind.access == Access::Rw {
                    *access = Access::Rw;
                }
                *keep_nested |= bind.keep_nested;
            })
            .or_insert((bind.access, bind.keep_nested));
    }
    let mut specs = by_source
        .into_iter()
        .map(|(source, (access, keep_nested))| BindSpec {
            source,
            access,
            keep_nested,
        })
        .collect::<Vec<_>>();
    specs.sort_by(|left, right| {
        source_depth(&left.source)
            .cmp(&source_depth(&right.source))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| access_mode(left.access).cmp(access_mode(right.access)))
    });

    let mut kept: Vec<BindSpec> = Vec::new();
    'specs: for spec in specs {
        for existing in &kept {
            if !spec.keep_nested
                && spec.source.starts_with(&existing.source)
                && spec.access == existing.access
            {
                continue 'specs;
            }
        }
        kept.push(spec);
    }

    kept.into_iter()
        .map(|bind| {
            format!(
                "{}:{}:{}",
                bind.source.display(),
                path_translate::to_container_path(&bind.source, host_root),
                access_mode(bind.access)
            )
        })
        .collect()
}

fn source_depth(path: &Path) -> usize {
    path.components().count()
}

fn access_mode(access: Access) -> &'static str {
    match access {
        Access::Ro => ACCESS_READ_ONLY,
        Access::Rw => ACCESS_READ_WRITE,
    }
}

fn absolute_source(path: &Path, base: &Path) -> PathBuf {
    let anchor = if base.is_absolute() {
        base.to_path_buf()
    } else {
        std::env::current_dir().map_or_else(|_| base.to_path_buf(), |cwd| cwd.join(base))
    };
    tailor_config::absolutize(path, anchor)
}

/// The full container invocation tailor runs for a cell: `docker run … <image> <ic-args…>`.
///
/// This is what `--dry-run` prints. It mirrors what the bollard runtime executes (the same binds,
/// privilege, platform, image, and Image Customizer argument vector) so the preview is faithful;
/// the ephemeral `--name` is omitted for readability.
pub(crate) fn build_run_command(
    cell: &Cell,
    context: &ExecutionContext,
) -> Result<Vec<String>, ExecError> {
    let mut argv = docker_prelude(cell, context, &[])?;
    argv.extend(build_ic_args(cell, context)?);
    Ok(argv)
}

/// The `docker run` prelude (`--rm [--privileged] --platform … -v … <ic-image>`) that precedes any
/// IC arg vector.
fn docker_prelude(
    cell: &Cell,
    context: &ExecutionContext,
    extra_rw: &[PathBuf],
) -> Result<Vec<String>, ExecError> {
    let mut argv = vec![DOCKER.to_owned(), DOCKER_RUN.to_owned(), FLAG_RM.to_owned()];
    if context.runtime.privileged {
        argv.push(FLAG_PRIVILEGED.to_owned());
    }
    argv.extend([FLAG_PLATFORM.to_owned(), context.platform.clone()]);
    for bind in container_binds(cell, context, extra_rw)? {
        argv.extend([FLAG_VOLUME.to_owned(), bind]);
    }
    argv.push(context.ic_image_ref.clone());
    Ok(argv)
}

/// Render a signed cell's three-pass for `--dry-run` (`meta/docs/2026-06-29-signing.md` §5): the real
/// `customize` → raw-intermediate `docker run`, then a note describing the host sign step and the
/// `inject-files` → final-format pass. No daemon is contacted.
pub(crate) fn render_signed_dry_run(
    cell: &Cell,
    context: &ExecutionContext,
) -> Result<String, ExecError> {
    let intermediate = intermediate_path(cell, context);
    let mut customize = docker_prelude(cell, context, &[])?;
    customize.extend(build_signed_customize_args(cell, context, &intermediate)?);
    let ca_cert = context
        .output_dir
        .join(crate::output_artifacts::ca_cert_name(cell.slug.as_ref()));
    Ok(format!(
        "# {slug} — signed 3-pass (meta/docs/2026-06-29-signing.md §5)\n\
         # pass 1/3: customize -> raw intermediate ({intermediate})\n{customize}\n\n\
         # pass 2/3: host-side sign the staged boot artifacts (openssl + sbsign); publish CA -> {ca}\n\
         # pass 3/3: inject-files -> final {fmt} ({final})",
        slug = cell.slug.as_ref(),
        intermediate = intermediate.display(),
        customize = render_command(&customize),
        ca = ca_cert.display(),
        fmt = cell.output.format.as_str(),
        final = artifact_path(cell, context).display(),
    ))
}

/// Render an argv as a copy-pasteable multiline shell command (bash backslash continuations), with
/// `docker run` on the first line and each flag/value pair or bare token indented on its own line.
pub(crate) fn render_command(argv: &[String]) -> String {
    if argv.len() < 2 {
        return argv.join(" ");
    }
    let mut out = format!("{} {}", argv[0], argv[1]);
    for unit in group_tokens(&argv[2..]) {
        out.push_str(" \\\n  ");
        out.push_str(&unit);
    }
    out
}

/// Group a token slice so a flag and its value share a line, while bare tokens stand alone.
fn group_tokens(tokens: &[String]) -> Vec<String> {
    let mut lines = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let token = &tokens[index];
        let next_is_value = tokens
            .get(index + 1)
            .is_some_and(|next| !next.starts_with('-'));
        if token.starts_with('-') && next_is_value {
            lines.push(format!("{token} {}", tokens[index + 1]));
            index += 2;
        } else {
            lines.push(token.clone());
            index += 1;
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
        sync::Arc,
    };

    use serde_yaml_ng::{Mapping, Value};
    use tailor_config::{
        Arch, BaseImageCatalogue, BaseSource, ImageDefinition, OciBase, Operation,
        OutputArtifactsPolicy, OutputFormat, OutputSpec,
    };
    use tailor_core::{Cell, CellSlug, ExecutionContext, RuntimeConfig, Target, ToolsDirPlan};

    #[test]
    fn builds_customize_args_with_translated_paths() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        let context = sample_context();

        let args = build_ic_args(&cell, &context).unwrap();

        assert_eq!(
            args,
            [
                "customize",
                "--config-file",
                "/host/images/.tailor-render.sample_cosi.ic.yaml",
                "--log-format",
                "json",
                "--log-color",
                "never",
                "--log-level",
                "debug",
                "--build-dir",
                "/tmp",
                "--image-file",
                "/host/base.raw",
                "--output-image-format",
                "cosi",
                "--output-image-file",
                "/host/out/sample_cosi.cosi",
                "--cosi-compression-level",
                "7",
                "--rpm-source",
                "/host/rpms/one",
                "--image-cache-dir",
                "/host/cache",
            ]
        );
    }

    #[test]
    fn appends_extra_params_after_managed_flags() {
        let mut cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        cell.extra_params = vec![
            ExtraParam {
                param: "--experimental-flag".to_owned(),
                value: Some("on".to_owned()),
            },
            ExtraParam {
                param: "--verbose-stage".to_owned(),
                value: None,
            },
        ];
        let context = sample_context();

        let args = build_ic_args(&cell, &context).unwrap();

        assert_eq!(
            &args[args.len() - 2..],
            ["--experimental-flag=on", "--verbose-stage"]
        );
    }

    #[test]
    fn extra_params_reject_a_tailor_managed_flag() {
        let mut cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        cell.extra_params = vec![ExtraParam {
            param: FLAG_BUILD_DIR.to_owned(),
            value: Some("/tmp/evil".to_owned()),
        }];
        let context = sample_context();

        let err = build_ic_args(&cell, &context).unwrap_err();
        assert!(
            matches!(err, ExecError::ReservedParam { ref param } if param == FLAG_BUILD_DIR),
            "got {err:?}"
        );
    }

    #[test]
    fn builds_convert_args_without_config_or_rpm_sources() {
        let cell = sample_cell(
            Operation::Convert,
            BaseSource::Path {
                path: "/base.vhdx".into(),
                arch: None,
            },
        );
        let context = sample_context();

        let args = build_ic_args(&cell, &context).unwrap();

        assert!(!args.iter().any(|arg| arg == FLAG_CONFIG_FILE));
        assert!(!args.iter().any(|arg| arg == FLAG_RPM_SOURCE));
        assert_eq!(args[0], SUBCOMMAND_CONVERT);
        assert!(
            args.windows(2)
                .any(|pair| pair == [FLAG_IMAGE_FILE, "/host/base.vhdx"])
        );
    }

    #[test]
    fn run_command_carries_the_docker_prelude() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        let context = sample_context();

        let argv = build_run_command(&cell, &context).unwrap();

        assert_eq!(&argv[..3], ["docker", "run", "--rm"]);
        assert!(argv.iter().any(|t| t == "--privileged"));
        assert!(argv.windows(2).any(|p| p == ["--platform", "linux/amd64"]));
        assert!(!argv.windows(2).any(|p| p == ["-v", "/:/host"]));
        assert!(
            argv.windows(2)
                .any(|p| p == ["-v", "/workspace:/host/workspace:ro"])
        );
        assert!(argv.windows(2).any(|p| p == ["-v", "/out:/host/out:rw"]));
        assert!(argv.windows(2).any(|p| p == ["-v", "/dev:/dev"]));
        // The IC image precedes its subcommand and arguments.
        let image = argv.iter().position(|t| t == "ic@sha256:abc").unwrap();
        let customize = argv.iter().position(|t| t == "customize").unwrap();
        assert!(image < customize);
    }

    #[test]
    fn unset_build_dir_base_keeps_container_tmp_without_host_root_bind() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        let context = sample_context();

        let args = build_ic_args(&cell, &context).unwrap();
        let binds = container_binds(&cell, &context, &[]).unwrap();

        assert!(
            args.windows(2)
                .any(|pair| pair == [FLAG_BUILD_DIR, CONTAINER_TMP_BUILD_DIR])
        );
        assert!(!binds.iter().any(|bind| bind == "/:/host"));
        assert!(!binds.iter().any(|bind| bind.starts_with("/:")));
    }

    #[test]
    fn build_dir_base_is_translated_and_bound_rw() {
        let Some(base) = separate_build_dir_base() else {
            return;
        };
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        let mut context = sample_context();
        context.runtime.build_dir_base = Some(base.clone());

        let args = build_ic_args(&cell, &context).unwrap();
        let binds = container_binds(&cell, &context, &[]).unwrap();
        let build_dir = base.join(cell.slug.as_ref());
        let build_dir_arg = format!("/host{}", build_dir.display());
        let build_dir_bind = format!(
            "{}:{}:rw",
            build_dir.display(),
            path_translate::to_container_path(&build_dir, &context.runtime.host_root)
        );

        assert!(
            args.windows(2)
                .any(|pair| pair == [FLAG_BUILD_DIR, build_dir_arg.as_str()])
        );
        assert!(binds.iter().any(|bind| bind == &build_dir_bind));
    }

    #[test]
    fn container_binds_include_workspace_carveouts_inputs_and_extra_paths() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/external/bases/base.raw".into(),
                arch: None,
            },
        );
        let mut context = sample_context();
        context.runtime.log_dir = Some(PathBuf::from("/logs"));
        context.runtime.extra_paths = vec![
            tailor_config::ExtraMount {
                path: PathBuf::from("/opt/shared"),
                access: Access::Ro,
            },
            tailor_config::ExtraMount {
                path: PathBuf::from("/extra-rw"),
                access: Access::Rw,
            },
        ];
        let staging = PathBuf::from("/stage/sample");

        let binds = container_binds(&cell, &context, std::slice::from_ref(&staging)).unwrap();

        assert!(
            binds
                .iter()
                .any(|bind| bind == "/workspace:/host/workspace:ro")
        );
        assert!(binds.iter().any(|bind| bind == "/out:/host/out:rw"));
        assert!(binds.iter().any(|bind| bind == "/cache:/host/cache:rw"));
        assert!(binds.iter().any(|bind| bind == "/logs:/host/logs:rw"));
        assert!(
            binds
                .iter()
                .any(|bind| bind == "/stage/sample:/host/stage/sample:rw")
        );
        assert!(
            binds
                .iter()
                .any(|bind| bind == "/external/bases:/host/external/bases:ro")
        );
        assert!(
            binds
                .iter()
                .any(|bind| bind == "/rpms/one:/host/rpms/one:ro")
        );
        assert!(
            binds
                .iter()
                .any(|bind| bind == "/opt/shared:/host/opt/shared:ro")
        );
        assert!(
            binds
                .iter()
                .any(|bind| bind == "/extra-rw:/host/extra-rw:rw")
        );
        assert!(binds.iter().any(|bind| bind == DEV_BIND));
        assert!(!binds.iter().any(|bind| bind.starts_with("/:")));
    }

    /// Regression (acl-iso): a directory RPM source becomes a tailor-built farm that the executor
    /// passes in `extra_rw` (createrepo writes `repodata/` into it). Even though the farm path is also
    /// an `rpm_sources` entry — which the out-of-workspace loop binds read-only — the overlapping
    /// read-write bind must win, so IC can create the repo. A `.repo`-file source (not a farm, not in
    /// `extra_rw`) stays read-only.
    #[test]
    fn a_writable_rpm_farm_overrides_the_read_only_rpm_bind() {
        let mut cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/images/base.raw".into(),
                arch: None,
            },
        );
        // Mimic the executor's post-`prepare_rpm_farms` cell: one directory farm + one `.repo`
        // passthrough, both out-of-workspace (so the read-only loop binds them).
        let farm = PathBuf::from("/rpms/.tailor-farm-sample-0");
        let repo_file = PathBuf::from("/rpms/extra.repo");
        cell.rpm_sources = vec![farm.clone(), repo_file];
        let context = sample_context();

        // The executor adds only the writable (directory) farm to `extra_rw`.
        let binds = container_binds(&cell, &context, std::slice::from_ref(&farm)).unwrap();

        assert!(
            binds
                .iter()
                .any(|bind| bind
                    == "/rpms/.tailor-farm-sample-0:/host/rpms/.tailor-farm-sample-0:rw"),
            "the directory farm must be bound read-write; got {binds:?}"
        );
        assert!(
            binds
                .iter()
                .any(|bind| bind == "/rpms/extra.repo:/host/rpms/extra.repo:ro"),
            "a .repo-file source must stay read-only; got {binds:?}"
        );
    }

    #[test]
    fn render_command_groups_flags_and_values_per_line() {
        let argv = vec![
            "docker".to_owned(),
            "run".to_owned(),
            "--rm".to_owned(),
            "--platform".to_owned(),
            "linux/amd64".to_owned(),
            "image".to_owned(),
            "customize".to_owned(),
            "--config-file".to_owned(),
            "/host/x.yaml".to_owned(),
        ];
        let rendered = render_command(&argv);
        let expected = "docker run \\\n  --rm \\\n  --platform linux/amd64 \\\n  image \\\n  \
             customize \\\n  --config-file /host/x.yaml";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn customize_registry_base_uses_image_flag() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Oci {
                oci: OciBase {
                    uri: "registry.example/base@sha256:abc".to_owned(),
                    platform: Some("linux/amd64".to_owned()),
                },
            },
        );
        let context = sample_context();

        let args = build_ic_args(&cell, &context).unwrap();

        assert!(
            args.windows(2)
                .any(|pair| pair == [FLAG_IMAGE, "oci:registry.example/base@sha256:abc"])
        );
    }

    #[test]
    fn customize_uses_the_pinned_base_ref_when_present() {
        // A real build threads the digest-pinned `base_ref`; it must win over the manifest uri.
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::AzureLinux {
                azure_linux: tailor_config::AzureLinuxBase {
                    version: "3.0".to_owned(),
                    variant: "minimal-os".to_owned(),
                },
            },
        );
        let mut context = sample_context();
        context.base_ref =
            Some("oci:mcr.microsoft.com/azurelinux/3.0/image/minimal-os@sha256:dead".to_owned());

        let args = build_ic_args(&cell, &context).unwrap();

        assert!(args.windows(2).any(|pair| pair
            == [
                FLAG_IMAGE,
                "oci:mcr.microsoft.com/azurelinux/3.0/image/minimal-os@sha256:dead"
            ]));
    }

    #[test]
    fn customize_emits_tools_dir_flag_and_never_root() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        let mut context = sample_context();
        context.tools_dir = Some(ToolsDirPlan {
            image_ref: "registry.example/tools@sha256:abc".to_owned(),
            digest: "sha256:abc".to_owned(),
            pull: true,
            cache_dir: PathBuf::from("/cache/tools-dirs/sha256_abc"),
            mount_dir: PathBuf::from("/build/gizmo/tools-dir"),
        });

        let args = build_ic_args(&cell, &context).unwrap();

        assert!(
            args.windows(2)
                .any(|pair| pair == [FLAG_TOOLS_DIR, "/host/build/gizmo/tools-dir"]),
            "got {args:?}"
        );
        assert!(!args.windows(2).any(|pair| pair == [FLAG_TOOLS_DIR, "/"]));
    }

    #[test]
    fn convert_and_inject_files_do_not_emit_tools_dir() {
        let convert = sample_cell(
            Operation::Convert,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        let mut context = sample_context();
        context.tools_dir = Some(ToolsDirPlan {
            image_ref: "registry.example/tools@sha256:abc".to_owned(),
            digest: "sha256:abc".to_owned(),
            pull: true,
            cache_dir: PathBuf::from("/cache/tools-dirs/sha256_abc"),
            mount_dir: PathBuf::from("/cache/tools-dirs/sha256_abc"),
        });

        let convert_args = build_ic_args(&convert, &context).unwrap();
        assert!(!convert_args.iter().any(|arg| arg == FLAG_TOOLS_DIR));

        let inject_args = build_inject_files_args(
            &convert,
            &context,
            Path::new("/out/intermediate.raw"),
            Path::new("/out/inject-files.yaml"),
            Path::new("/out/final.cosi"),
        )
        .unwrap();
        assert!(!inject_args.iter().any(|arg| arg == FLAG_TOOLS_DIR));
    }

    #[test]
    fn rw_tools_dir_binds_per_cell_copy_rw() {
        let Some(base) = separate_build_dir_base() else {
            return;
        };
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        let mut context = sample_context();
        let copy = base.join(cell.slug.as_ref()).join("tools-dir");
        context.runtime.build_dir_base = Some(base);
        context.tools_dir = Some(ToolsDirPlan {
            image_ref: "registry.example/tools@sha256:abc".to_owned(),
            digest: "sha256:abc".to_owned(),
            pull: true,
            cache_dir: PathBuf::from("/cache/tools-dirs/sha256_abc"),
            mount_dir: copy.clone(),
        });

        let binds = container_binds(&cell, &context, &[]).unwrap();
        let expected = format!(
            "{}:{}:rw",
            copy.display(),
            path_translate::to_container_path(&copy, &context.runtime.host_root)
        );

        assert!(binds.iter().any(|bind| bind == &expected), "got {binds:?}");
    }

    #[test]
    fn always_runs_ic_structured_at_debug_by_default() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        let mut context = sample_context();
        // No explicit log level: IC must still run structured at the `debug` default (§5.1).
        context.runtime.log_level = None;

        let args = build_ic_args(&cell, &context).unwrap();

        assert!(args.windows(2).any(|p| p == [FLAG_LOG_FORMAT, "json"]));
        assert!(args.windows(2).any(|p| p == [FLAG_LOG_COLOR, "never"]));
        assert!(args.windows(2).any(|p| p == [FLAG_LOG_LEVEL, "debug"]));
        // Persistence is off by default: no `--log-file`.
        assert!(!args.iter().any(|arg| arg == FLAG_LOG_FILE));
    }

    #[test]
    fn persists_per_cell_log_when_log_dir_is_set() {
        let cell = sample_cell(
            Operation::Customize,
            BaseSource::Path {
                path: "/base.raw".into(),
                arch: None,
            },
        );
        let mut context = sample_context();
        context.runtime.log_dir = Some(PathBuf::from("/logs"));

        let args = build_ic_args(&cell, &context).unwrap();

        assert!(
            args.windows(2)
                .any(|p| p == [FLAG_LOG_FILE, "/host/logs/sample_cosi.log"])
        );
    }

    fn sample_context() -> ExecutionContext {
        ExecutionContext {
            output_dir: PathBuf::from("/out"),
            ic_image_ref: "ic@sha256:abc".to_owned(),
            base_ref: None,
            tools_dir: None,
            platform: "linux/amd64".to_owned(),
            clone_index: None,
            dry_run: false,
            pull: true,
            signer: None,
            runtime: RuntimeConfig {
                host_root: PathBuf::from("/host"),
                workspace_root: PathBuf::from("/workspace"),
                privileged: true,
                mount_dev: true,
                build_dir_base: None,
                log_level: Some("debug".to_owned()),
                image_cache_dir: Some(PathBuf::from("/cache")),
                log_dir: None,
                extra_paths: Vec::new(),
                janitor_image: "janitor@sha256:def".to_owned(),
            },
        }
    }

    fn separate_build_dir_base() -> Option<PathBuf> {
        use std::os::unix::fs::MetadataExt;

        let candidate = PathBuf::from("/dev/shm/tailor-build");
        let parent = candidate.parent()?;
        if parent.exists()
            && std::fs::metadata(parent).ok()?.dev()
                != std::fs::metadata(Path::new("/")).ok()?.dev()
        {
            Some(candidate)
        } else {
            None
        }
    }

    fn sample_cell(operation: Operation, base: BaseSource) -> Cell {
        Cell {
            target: Arc::new(Target {
                definition: sample_definition(operation),
                dir: PathBuf::from("/images"),
                default_outputs: Vec::new(),
                output_artifacts: OutputArtifactsPolicy::default(),
                root: PathBuf::from("/images"),
                base_images: BaseImageCatalogue::default(),
                tools_dir_sources: Vec::new(),
            }),
            axes: BTreeMap::new(),
            arch: Arch::Amd64,
            output: OutputSpec {
                format: OutputFormat::Cosi,
                cosi_compression_level: Some(7),
                compression: None,
                name: None,
            },
            slug: CellSlug("sample_cosi".to_owned()),
            ic_config: Value::Mapping(Mapping::default()),
            base,
            base_image: None,
            rpm_sources: vec![PathBuf::from("/rpms/one")],
            extra_params: Vec::new(),
            input_deps: Vec::new(),
            tools_dir: None,
            skip: false,
            skip_pins: Vec::new(),
        }
    }

    fn sample_definition(operation: Operation) -> ImageDefinition {
        let operation = match operation {
            Operation::Customize => "customize",
            Operation::Convert => "convert",
        };
        serde_yaml_ng::from_str(&format!(
            r"
name: sample
operation: {operation}
"
        ))
        .unwrap()
    }
}
