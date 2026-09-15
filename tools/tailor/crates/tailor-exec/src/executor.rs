use std::{
    fs,
    io::{self, BufReader},
    path::{Path, PathBuf},
    slice,
    sync::Arc,
};

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use tailor_config::{Compression, OutputArtifactsPolicy, OutputFormat};
use tailor_core::{
    Cell, ContainerConfig, ContainerResult, ContainerRuntime, ExecError, ExecutionContext,
    ExecutionResult, Executor, RuntimeConfig, Signer, SigningPlan, ToolsDirPlan, artifact_name,
    atomic, output_slug, published_artifact_name,
};

use crate::{arg_builder, guard, janitor, output_artifacts, rpm_farm, working_copy};

const CONTAINER_NAME_PREFIX: &str = "tailor-ic";
const RPM_FARM_PREFIX: &str = ".tailor-farm";
/// The manifest IC's `customize` writes into the `output.artifacts` staging dir, describing the boot
/// artifacts to sign and re-inject (`meta/docs/2026-06-29-signing.md` §5).
const INJECT_FILES_MANIFEST: &str = "inject-files.yaml";

#[derive(Debug, Clone)]
pub struct IcExecutor<R> {
    runtime: R,
}

impl<R> IcExecutor<R> {
    pub fn new(runtime: R) -> Self {
        Self { runtime }
    }

    pub fn runtime(&self) -> &R {
        &self.runtime
    }
}

impl<R: ContainerRuntime> Executor for IcExecutor<R> {
    async fn execute(
        &self,
        cell: &Cell,
        context: &ExecutionContext,
        cancel: CancellationToken,
    ) -> Result<ExecutionResult, ExecError> {
        // Image Customizer writes the uncompressed artifact; when `compression:` is set, tailor
        // compresses it into `published_path` after the build (`ic_output_path` is then removed).
        // The output slug carries the clone index, so each `--clones` run publishes a distinct file.
        let out_slug = output_slug(cell.slug.as_ref(), context.clone_index);
        let ic_output_path = context
            .output_dir
            .join(artifact_name(&out_slug, cell.output.format));
        let published_path = context.output_dir.join(published_artifact_name(
            &out_slug,
            cell.output.format,
            cell.output.compression,
        ));
        if context.dry_run {
            let logs = if context.signer.is_some() {
                arg_builder::render_signed_dry_run(cell, context)?
            } else {
                let command =
                    arg_builder::render_command(&arg_builder::build_run_command(cell, context)?);
                format!("# {}\n{command}", cell.slug.as_ref())
            };
            info!(cell = %cell.slug, "dry-run container invocation");
            return Ok(ExecutionResult {
                artifact_path: published_path,
                exit_code: 0,
                logs,
            });
        }

        fs::create_dir_all(&context.output_dir).map_err(|source| ExecError::Io {
            context: format!(
                "failed to create output directory `{}`",
                context.output_dir.display()
            ),
            source,
        })?;
        if let Some(cache_dir) = &context.runtime.image_cache_dir {
            fs::create_dir_all(cache_dir).map_err(|source| ExecError::Io {
                context: format!(
                    "failed to create image cache directory `{}`",
                    cache_dir.display()
                ),
                source,
            })?;
        }
        // When on-disk persistence is opted in (§5.5), ensure the log directory exists so IC's
        // `--log-file` lands; IC writes the file itself, tailor just chowns and points at it.
        let log_file = arg_builder::log_file_path(cell, context);
        if let Some(log_dir) = &context.runtime.log_dir {
            fs::create_dir_all(log_dir).map_err(|source| ExecError::Io {
                context: format!("failed to create log directory `{}`", log_dir.display()),
                source,
            })?;
        }
        if let Some(build_dir) = arg_builder::build_dir_path(cell, context) {
            guard::ensure_safe_build_dir(&build_dir)?;
            fs::create_dir_all(&build_dir).map_err(|source| ExecError::Io {
                context: format!("failed to create build directory `{}`", build_dir.display()),
                source,
            })?;
        }

        let prepared_tools_dir = prepare_tools_dir(&self.runtime, context, cancel.clone()).await?;

        let farms = prepare_rpm_farms(cell, context)?;
        // Directory RPM farms are tailor-created and disposable; IC runs `createrepo_c` in place in
        // each, so they must be bound read-write (a `.repo`-file passthrough stays read-only). This is
        // the writable carve-out the read-only workspace bind requires (`meta/docs/2026-06-22-design.md` §7.8).
        let writable_farms = farms
            .iter()
            .filter(|farm| farm.writable)
            .map(|farm| farm.arg_path.clone())
            .collect::<Vec<_>>();
        let mut run_cell = cell.clone();
        run_cell.rpm_sources = farms
            .iter()
            .map(|farm| farm.arg_path.clone())
            .collect::<Vec<_>>();

        // Relocate IC's `output.artifacts` scratch to a tailor-owned path so it does not land
        // root-owned in the source tree (`meta/docs/2026-06-29-output-artifacts-staging.md`). A signed cell
        // forces `scratch` — the staging is interim, reclaimed after `inject-files` (`2026-06-29-signing.md` §5).
        let run_id = output_artifacts::run_id();
        let policy = if context.signer.is_some() {
            OutputArtifactsPolicy::Scratch
        } else {
            cell.target.output_artifacts
        };
        let staging = output_artifacts::apply(
            &mut run_cell.ic_config,
            policy,
            cell.slug.as_ref(),
            &run_id,
            &cell.target.dir,
            &context.output_dir,
            &context.runtime.host_root,
        );
        // Reclaim any staging dir an earlier crashed run of this cell left behind (§3.5). Safe: only
        // matches tailor's own `.tailor-stage.<slug>.*`, and same-cell concurrency in one worktree is
        // already unsupported (the working copy collides).
        if staging.is_some() {
            let stale = output_artifacts::stale_staging_dirs(&cell.target.dir, cell.slug.as_ref());
            if !stale.is_empty() {
                janitor::remove_paths(&self.runtime, &stale, &context.runtime, cancel.clone())
                    .await?;
            }
        }

        let rendered_config = working_copy::render_working_copy(&run_cell.ic_config)
            .map_err(|err| ExecError::Other(err.to_string()))?;
        let working_copy_path =
            working_copy::write_working_copy(&run_cell, &rendered_config, context.clone_index)
                .map_err(|source| ExecError::Io {
                    context: "failed to write Image Customizer working copy".to_owned(),
                    source,
                })?;

        // Run IC: the single-pass `customize`, or the signed three-pass (`customize` → sign →
        // `inject-files`, `meta/docs/2026-06-29-signing.md` §5). Both remove the working copy and reclaim the
        // staging tree before propagating any failure.
        let result = if let Some(signer) = context.signer.clone() {
            self.run_signed_passes(
                cell,
                context,
                &run_cell,
                staging.as_ref(),
                &writable_farms,
                &signer,
                &working_copy_path,
                log_file.as_ref(),
                cancel.clone(),
            )
            .await
        } else {
            let args = arg_builder::build_ic_args(&run_cell, context)?;
            let mut extra_rw = staging
                .as_ref()
                .map(|plan| plan.dir.clone())
                .into_iter()
                .collect::<Vec<_>>();
            extra_rw.extend(writable_farms.iter().cloned());
            let result = self
                .run_ic(
                    cell,
                    context,
                    args,
                    &extra_rw,
                    log_file.as_ref(),
                    cancel.clone(),
                )
                .await;
            let _ = fs::remove_file(&working_copy_path);

            // Reclaim IC's root-owned staging tree. On success: chown so the caller can read the
            // outputs, then remove if the policy is scratch. On an IC failure: reclaim best-effort,
            // subordinate to the IC error (§3.4, ACL shakeout #2).
            if let Some(plan) = &staging {
                if ic_run_failed(&result) {
                    self.reclaim_subordinate(
                        slice::from_ref(&plan.dir),
                        &context.runtime,
                        cancel.clone(),
                        true,
                    )
                    .await?;
                } else {
                    janitor::chown_paths(
                        &self.runtime,
                        slice::from_ref(&plan.dir),
                        &context.runtime,
                        cancel.clone(),
                    )
                    .await?;
                    if plan.reclaim {
                        janitor::remove_paths(
                            &self.runtime,
                            slice::from_ref(&plan.dir),
                            &context.runtime,
                            cancel.clone(),
                        )
                        .await?;
                    }
                }
            }
            result
        };
        // Reclaim the per-cell tools-dir copy (disposable scratch on `buildDirBase`). The janitor
        // binds the copy's PARENT and removes it as a child, so a successful build's cleanup no
        // longer hits `EBUSY` on the copy's own mountpoint (see janitor::remove_paths). The
        // subordination below still matters if IC genuinely crashed mid-chroot and left real
        // `proc`/`sys`/`dev` mounts under the copy — then a cleanup error must not bury the IC
        // failure (ACL shakeout #2).
        let ic_failed = ic_run_failed(&result);
        if let Some(copy) = &prepared_tools_dir.rw_copy {
            self.reclaim_subordinate(
                slice::from_ref(copy),
                &context.runtime,
                cancel.clone(),
                ic_failed,
            )
            .await?;
        }
        // Reclaim the disposable RPM farms (root-owned `repodata/` created by IC's createrepo).
        // Removing the whole farm — not just chowning its repodata — leaves no `.tailor-farm-*`
        // clutter next to the user's RPMs, and runs on the error path too (best-effort when IC failed).
        if !writable_farms.is_empty() {
            self.reclaim_subordinate(&writable_farms, &context.runtime, cancel.clone(), ic_failed)
                .await?;
        }
        let result = result?;

        if result.exit_code != 0 {
            return Err(ExecError::IcFailed {
                cell: cell.slug.as_ref().to_owned(),
                code: result.exit_code,
                dump: result.failure_dump.unwrap_or_default(),
            });
        }
        verify_artifact(&ic_output_path, cell.output.format)?;
        let mut managed_paths = vec![ic_output_path.clone()];
        // The image cache dir is written by IC inside the privileged container (root-owned); fold it
        // into the janitor sweep so the caller can read/clean it sudo-free.
        if let Some(cache_dir) = &context.runtime.image_cache_dir {
            managed_paths.push(cache_dir.clone());
        }
        // The per-cell IC log is written root-owned inside the container too; chown it so a runner can
        // read/upload it (`meta/docs/2026-06-29-logging.md` §5.5).
        if let Some(log_file) = &log_file {
            managed_paths.push(log_file.clone());
        }
        janitor::chown_paths(&self.runtime, &managed_paths, &context.runtime, cancel).await?;
        // Post-build compression (`meta/docs/2026-06-22-design.md` §10): IC has no notion of it, so
        // tailor streams its raw artifact through the codec into the published name and drops the
        // uncompressed original. Runs after the chown so the host user owns the input it reads.
        if let Some(compression) = cell.output.compression {
            compress_artifact(&ic_output_path, &published_path, compression)?;
        }
        Ok(ExecutionResult {
            artifact_path: published_path,
            exit_code: result.exit_code,
            logs: result.logs,
        })
    }

    async fn clean(
        &self,
        paths: &[PathBuf],
        runtime: &RuntimeConfig,
        cancel: CancellationToken,
    ) -> Result<(), ExecError> {
        janitor::remove_paths(&self.runtime, paths, runtime, cancel).await
    }
}

impl<R: ContainerRuntime> IcExecutor<R> {
    /// Run one IC container pass (`customize`/`inject-files`) with the computed workspace-scoped
    /// binds, returning the raw container result (the caller maps a non-zero exit to `IcFailed`).
    async fn run_ic(
        &self,
        cell: &Cell,
        context: &ExecutionContext,
        args: Vec<String>,
        extra_rw: &[PathBuf],
        log_file: Option<&PathBuf>,
        cancel: CancellationToken,
    ) -> Result<ContainerResult, ExecError> {
        // Pull policy resolution happens in the composition root; local-only images run without a
        // registry pull.
        if context.pull {
            self.runtime.pull_image(&context.ic_image_ref).await?;
        }
        self.runtime
            .create_and_run(
                ContainerConfig {
                    image_ref: context.ic_image_ref.clone(),
                    platform: context.platform.clone(),
                    name: container_name(cell, context),
                    args,
                    binds: arg_builder::container_binds(cell, context, extra_rw)?,
                    privileged: context.runtime.privileged,
                    cell_slug: cell.slug.as_ref().to_owned(),
                    log_source: tailor_core::LogSource::ImageCustomizer,
                    log_file: log_file.cloned(),
                },
                cancel,
            )
            .await
    }

    /// The signed three-pass (`meta/docs/2026-06-29-signing.md` §5): `customize` → raw intermediate → chown the
    /// staging → host-side `sign` → `inject-files` → final image. Returns the final pass's container
    /// result; always reclaims the staging tree and raw intermediate (best-effort) before returning.
    #[allow(
        clippy::too_many_arguments,
        reason = "the signed pass needs all of the run inputs"
    )]
    async fn run_signed_passes(
        &self,
        cell: &Cell,
        context: &ExecutionContext,
        run_cell: &Cell,
        staging: Option<&output_artifacts::StagingPlan>,
        writable_farms: &[PathBuf],
        signer: &Arc<dyn Signer>,
        working_copy_path: &Path,
        log_file: Option<&PathBuf>,
        cancel: CancellationToken,
    ) -> Result<ContainerResult, ExecError> {
        // A signed cell must stage `output.artifacts` — that is how IC emits the boot artifacts and
        // the `inject-files.yaml` manifest. No staging ⇒ the config declared no `output.artifacts`.
        let Some(plan) = staging else {
            let _ = fs::remove_file(working_copy_path);
            return Err(ExecError::Other(format!(
                "image `{}` requests `signing:` but its IC config declares no `output.artifacts` \
                 (add the `output-artifacts` preview feature and an `output.artifacts` block)",
                cell.slug.as_ref()
            )));
        };
        let intermediate = arg_builder::intermediate_path(cell, context);

        // Pass 1 — customize → raw intermediate. The staging dir and any writable RPM farms are
        // bound read-write (createrepo runs in the farms during customize).
        let mut pass1_rw = vec![plan.dir.clone()];
        pass1_rw.extend(writable_farms.iter().cloned());
        let customize = self
            .run_ic(
                cell,
                context,
                arg_builder::build_signed_customize_args(run_cell, context, &intermediate)?,
                &pass1_rw,
                log_file,
                cancel.clone(),
            )
            .await;
        let _ = fs::remove_file(working_copy_path);
        let customize = customize?;
        if customize.exit_code != 0 {
            self.reclaim_scratch(&plan.dir, &intermediate, &context.runtime, cancel.clone())
                .await;
            return Err(ExecError::IcFailed {
                cell: cell.slug.as_ref().to_owned(),
                code: customize.exit_code,
                dump: customize.failure_dump.unwrap_or_default(),
            });
        }

        // Chown the staging dir to the caller BEFORE signing — IC wrote it root-owned and the host
        // signer must read the unsigned artifacts and write signed replacements back into it.
        janitor::chown_paths(
            &self.runtime,
            slice::from_ref(&plan.dir),
            &context.runtime,
            cancel.clone(),
        )
        .await?;

        let inject_files_yaml = plan.dir.join(INJECT_FILES_MANIFEST);
        if !inject_files_yaml.exists() {
            self.reclaim_scratch(&plan.dir, &intermediate, &context.runtime, cancel.clone())
                .await;
            return Err(ExecError::Other(format!(
                "image `{}` requests `signing:` but IC emitted no `{INJECT_FILES_MANIFEST}` \
                 (its `output.artifacts` extracted nothing to sign)",
                cell.slug.as_ref()
            )));
        }

        // Sign in place on a blocking thread (openssl/sbsign are synchronous).
        let sign_plan = SigningPlan {
            inject_files_yaml: inject_files_yaml.clone(),
            artifacts_dir: plan.dir.clone(),
            leaf_id: leaf_id(cell, context),
            ca_cert_dest: context
                .output_dir
                .join(output_artifacts::ca_cert_name(cell.slug.as_ref())),
        };
        let signer = Arc::clone(signer);
        let sign_result = tokio::task::spawn_blocking(move || signer.sign(&sign_plan))
            .await
            .map_err(|err| ExecError::Other(format!("signing task failed: {err}")))?;
        if let Err(err) = sign_result {
            self.reclaim_scratch(&plan.dir, &intermediate, &context.runtime, cancel.clone())
                .await;
            return Err(ExecError::Other(err.to_string()));
        }

        // Pass 2 — inject-files → the cell's final output format.
        let final_image = arg_builder::artifact_path(cell, context);
        let inject = self
            .run_ic(
                cell,
                context,
                arg_builder::build_inject_files_args(
                    cell,
                    context,
                    &intermediate,
                    &inject_files_yaml,
                    &final_image,
                )?,
                slice::from_ref(&plan.dir),
                log_file,
                cancel.clone(),
            )
            .await;

        // Reclaim the interim scratch (staging + raw intermediate) regardless of the inject outcome.
        self.reclaim_scratch(&plan.dir, &intermediate, &context.runtime, cancel)
            .await;
        inject
    }

    /// Best-effort sudo-free reclaim of a signed cell's interim scratch (staging dir + raw
    /// intermediate). Cleanup never masks the primary result; the crash sweep (§3.5) catches leftovers.
    async fn reclaim_scratch(
        &self,
        staging: &Path,
        intermediate: &Path,
        runtime: &RuntimeConfig,
        cancel: CancellationToken,
    ) {
        let paths = [staging.to_path_buf(), intermediate.to_path_buf()];
        if let Err(err) = janitor::remove_paths(&self.runtime, &paths, runtime, cancel).await {
            warn!(error = %err, "failed to reclaim signed build scratch (will be swept next run)");
        }
    }

    /// Reclaim a janitor-managed scratch path, subordinating the cleanup to a failed IC run: when
    /// `ic_failed`, a cleanup error (e.g. `EBUSY` from real `proc`/`sys`/`dev` mounts IC left behind
    /// when it crashed mid-chroot) is logged and swallowed so the real IC failure stays the headline
    /// (ACL shakeout #2); the leftover is swept on the next run. When IC succeeded, a cleanup failure
    /// is a genuine error and propagates — the janitor's own self-bind `EBUSY` no longer occurs on
    /// the success path (see `janitor::remove_paths`).
    async fn reclaim_subordinate(
        &self,
        paths: &[PathBuf],
        runtime: &RuntimeConfig,
        cancel: CancellationToken,
        ic_failed: bool,
    ) -> Result<(), ExecError> {
        match janitor::remove_paths(&self.runtime, paths, runtime, cancel).await {
            Ok(()) => Ok(()),
            Err(err) if ic_failed => {
                warn!(error = %err, "cleanup after a failed Image Customizer run did not complete; leftovers are swept next run");
                Ok(())
            }
            Err(err) => Err(err),
        }
    }
}

/// Whether the IC run failed — a transport error, or a non-zero container exit. Cleanup failures are
/// subordinated to this so a janitor error never buries the real IC failure (ACL shakeout #2).
fn ic_run_failed(result: &Result<ContainerResult, ExecError>) -> bool {
    !matches!(result, Ok(outcome) if outcome.exit_code == 0)
}

#[derive(Debug, Default)]
struct PreparedToolsDir {
    rw_copy: Option<PathBuf>,
}

async fn prepare_tools_dir<R: ContainerRuntime>(
    runtime: &R,
    context: &ExecutionContext,
    cancel: CancellationToken,
) -> Result<PreparedToolsDir, ExecError> {
    let Some(plan) = &context.tools_dir else {
        return Ok(PreparedToolsDir::default());
    };
    ensure_tools_dir_cache(runtime, context, plan, cancel.clone()).await?;
    // The tools-dir is always bound writable (IC rewrites resolv.conf in the tools chroot), so it is
    // a per-cell disposable copy on the isolated build filesystem. Guard the copy target before the
    // janitor `rm`s it, then refresh the copy from the shared cache.
    guard::ensure_safe_build_dir(&plan.mount_dir)?;
    if plan.mount_dir.exists() {
        janitor::remove_paths(
            runtime,
            slice::from_ref(&plan.mount_dir),
            &context.runtime,
            cancel.clone(),
        )
        .await?;
    }
    copy_dir_all(&plan.cache_dir, &plan.mount_dir).map_err(|source| ExecError::Io {
        context: format!(
            "failed to copy tools-dir cache `{}` to `{}`",
            plan.cache_dir.display(),
            plan.mount_dir.display()
        ),
        source,
    })?;
    Ok(PreparedToolsDir {
        rw_copy: Some(plan.mount_dir.clone()),
    })
}

/// Populate the shared, digest-keyed tools-dir cache once (export the source container's flattened
/// filesystem), if it is not already present. Idempotent across cells/runs.
async fn ensure_tools_dir_cache<R: ContainerRuntime>(
    runtime: &R,
    context: &ExecutionContext,
    plan: &ToolsDirPlan,
    cancel: CancellationToken,
) -> Result<(), ExecError> {
    if plan.cache_dir == Path::new("/") || plan.mount_dir == Path::new("/") {
        return Err(ExecError::UnsafeDir {
            path: plan.mount_dir.clone(),
            reason: "tools-dir must not be filesystem root".to_owned(),
        });
    }
    if dir_has_entries(&plan.cache_dir)? {
        return Ok(());
    }
    fs::create_dir_all(&plan.cache_dir).map_err(|source| ExecError::Io {
        context: format!(
            "failed to create tools-dir cache `{}`",
            plan.cache_dir.display()
        ),
        source,
    })?;
    if let Err(err) = runtime
        .export_container(
            &plan.image_ref,
            &context.platform,
            plan.pull,
            &plan.cache_dir,
            cancel,
        )
        .await
    {
        let _ = fs::remove_dir_all(&plan.cache_dir);
        return Err(err);
    }
    Ok(())
}

fn dir_has_entries(path: &Path) -> Result<bool, ExecError> {
    match fs::read_dir(path) {
        Ok(mut entries) => entries
            .next()
            .transpose()
            .map(|entry| entry.is_some())
            .map_err(|source| ExecError::Io {
                context: format!("failed to read tools-dir cache `{}`", path.display()),
                source,
            }),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(ExecError::Io {
            context: format!("failed to read tools-dir cache `{}`", path.display()),
            source,
        }),
    }
}

fn copy_dir_all(source: &Path, dest: &Path) -> Result<(), std::io::Error> {
    fs::create_dir_all(dest)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        let metadata = fs::symlink_metadata(&source_path)?;
        if metadata.file_type().is_symlink() {
            let target = fs::read_link(&source_path)?;
            std::os::unix::fs::symlink(target, dest_path)?;
        } else if metadata.is_dir() {
            copy_dir_all(&source_path, &dest_path)?;
        } else {
            fs::copy(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RpmFarm {
    arg_path: PathBuf,
    /// A tailor-built **directory** farm (reflink/hardlink of a directory source): IC runs
    /// `createrepo_c` in place here, so it must be bind-mounted read-write and reclaimed afterward. A
    /// `.repo`-file passthrough is `false` — createrepo does not run and the file stays read-only
    /// (`meta/docs/2026-06-22-design.md` §7.8).
    writable: bool,
}

fn prepare_rpm_farms(cell: &Cell, context: &ExecutionContext) -> Result<Vec<RpmFarm>, ExecError> {
    let mut farms = Vec::new();
    for (index, source) in cell.rpm_sources.iter().enumerate() {
        // RPM sources are config-relative; absolutize against the image directory so the farm path
        // (and its `repodata`, which the janitor binds by absolute path) is never relative — a
        // relative bind spec is rejected by the container engine (`invalid mount path: must be
        // absolute`).
        let source = tailor_config::absolutize(source, &cell.target.dir);
        if source.is_dir() {
            let parent = source.parent().ok_or_else(|| {
                ExecError::Other(format!(
                    "RPM source `{}` has no parent directory",
                    source.display()
                ))
            })?;
            let dest = parent.join(farm_name(cell, context.clone_index, index));
            rpm_farm::build_rpm_farm(&source, &dest).map_err(|source_err| ExecError::Io {
                context: format!("failed to build RPM farm `{}`", dest.display()),
                source: source_err,
            })?;
            farms.push(RpmFarm {
                arg_path: dest,
                writable: true,
            });
        } else {
            farms.push(RpmFarm {
                arg_path: source,
                writable: false,
            });
        }
    }
    Ok(farms)
}

fn farm_name(cell: &Cell, clone_index: Option<u32>, index: usize) -> String {
    match clone_index {
        Some(clone) => format!(
            "{RPM_FARM_PREFIX}-{}_clone{clone}-{index}",
            cell.slug.as_ref()
        ),
        None => format!("{RPM_FARM_PREFIX}-{}-{index}", cell.slug.as_ref()),
    }
}

fn container_name(cell: &Cell, context: &ExecutionContext) -> String {
    match context.clone_index {
        Some(clone) => format!(
            "{CONTAINER_NAME_PREFIX}-{}_clone{clone}-{}",
            cell.slug.as_ref(),
            std::process::id()
        ),
        None => format!(
            "{CONTAINER_NAME_PREFIX}-{}-{}",
            cell.slug.as_ref(),
            std::process::id()
        ),
    }
}

/// A per-cell (and per-clone) identifier for the signing leaf key, so parallel/clone signs never
/// share a leaf (`meta/docs/2026-06-29-signing.md` §7).
fn leaf_id(cell: &Cell, context: &ExecutionContext) -> String {
    output_slug(cell.slug.as_ref(), context.clone_index)
}

fn verify_artifact(path: &PathBuf, format: OutputFormat) -> Result<(), ExecError> {
    let metadata = fs::metadata(path).map_err(|source| ExecError::Io {
        context: format!("Image Customizer did not produce `{}`", path.display()),
        source,
    })?;
    if format == OutputFormat::PxeDir && !metadata.is_dir() {
        return Err(ExecError::Other(format!(
            "Image Customizer output `{}` is not a directory",
            path.display()
        )));
    }
    if format != OutputFormat::PxeDir && !metadata.is_file() {
        return Err(ExecError::Other(format!(
            "Image Customizer output `{}` is not a file",
            path.display()
        )));
    }
    Ok(())
}

/// The zstd level tailor compresses artifacts at. Level 3 is the library default — a fast,
/// well-balanced ratio suitable for multi-GB disk images; a per-output level knob can come later.
const ZSTD_LEVEL: i32 = 3;

/// Stream `src` through the codec into `dst`, then remove `src`. Streamed (never buffered whole) so a
/// multi-GB image compresses in bounded memory. The compressed output is staged through a temp file
/// and renamed into place, so an interrupted build never leaves a truncated published artifact.
fn compress_artifact(src: &Path, dst: &Path, compression: Compression) -> Result<(), ExecError> {
    let mut reader = BufReader::new(fs::File::open(src).map_err(|source| ExecError::Io {
        context: format!("failed to open `{}` for compression", src.display()),
        source,
    })?);
    let temp = atomic::temp_sibling(dst);
    atomic::finish(&temp, dst, |output| match compression {
        Compression::Zstd => {
            let mut encoder = zstd::stream::Encoder::new(&mut *output, ZSTD_LEVEL)?;
            io::copy(&mut reader, &mut encoder)?;
            encoder.finish()?;
            Ok(())
        }
    })
    .map_err(|source| ExecError::Io {
        context: format!("failed to write compressed artifact `{}`", dst.display()),
        source,
    })?;
    fs::remove_file(src).map_err(|source| ExecError::Io {
        context: format!(
            "failed to remove uncompressed artifact `{}` after compression",
            src.display()
        ),
        source,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        collections::BTreeMap,
        path::PathBuf,
        sync::{Arc, Mutex},
    };

    use serde_yaml_ng::{Mapping, Value};
    use tailor_config::{
        Arch, BaseImageCatalogue, BaseSource, ImageDefinition, OutputArtifactsPolicy, OutputSpec,
    };
    use tailor_core::{CellSlug, Target};
    use tempfile::TempDir;

    use crate::container::runtime::NoopRuntime;

    fn dry_run_cell() -> Cell {
        let definition: ImageDefinition =
            serde_yaml_ng::from_str("name: sample\noperation: customize\n").unwrap();
        Cell {
            target: Arc::new(Target {
                definition,
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
                cosi_compression_level: None,
                compression: None,
                name: None,
            },
            slug: CellSlug("sample_cosi".to_owned()),
            ic_config: Value::Mapping(Mapping::default()),
            base: BaseSource::Path {
                path: PathBuf::from("/images/base.img"),
                arch: None,
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

    fn dry_run_context() -> ExecutionContext {
        ExecutionContext {
            output_dir: PathBuf::from("/out"),
            ic_image_ref: "ic@sha256:abc".to_owned(),
            base_ref: None,
            tools_dir: None,
            platform: "linux/amd64".to_owned(),
            clone_index: None,
            dry_run: true,
            pull: true,
            signer: None,
            runtime: RuntimeConfig::default(),
        }
    }

    /// Regression (acl-iso): a **relative** directory `rpmSources` path (`./rpms`) must yield an
    /// **absolute** farm path — a relative farm path produces a relative bind spec, which the
    /// container engine rejects (`invalid mount path: must be absolute`). Earlier tests only used
    /// absolute source paths, so this case slipped through.
    #[test]
    fn compress_artifact_zstd_round_trips_and_removes_the_original() {
        let tmp = TempDir::new().unwrap();
        let src = tmp.path().join("img.raw");
        let dst = tmp.path().join("img.raw.zst");
        // Repetitive but non-trivial content, larger than one buffer, so streaming is exercised.
        let data: Vec<u8> = (0..(1u32 << 18)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&src, &data).unwrap();

        compress_artifact(&src, &dst, Compression::Zstd).unwrap();

        assert!(!src.exists(), "uncompressed original must be removed");
        assert!(dst.exists(), "compressed artifact must exist");
        // Real zstd magic number.
        let head = std::fs::read(&dst).unwrap();
        assert_eq!(&head[..4], &[0x28, 0xB5, 0x2F, 0xFD], "not a zstd stream");
        // And it decodes back to the exact input.
        let decoded = zstd::stream::decode_all(std::io::Cursor::new(head)).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn a_relative_rpm_source_yields_an_absolute_writable_farm() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("rpms")).unwrap();
        std::fs::write(tmp.path().join("rpms/pkg.rpm"), b"rpm").unwrap();

        let mut cell = dry_run_cell();
        // The image directory is the tempdir; the source is config-relative.
        let target = Target {
            definition: cell.target.definition.clone(),
            dir: tmp.path().to_path_buf(),
            default_outputs: Vec::new(),
            output_artifacts: OutputArtifactsPolicy::default(),
            root: tmp.path().to_path_buf(),
            base_images: BaseImageCatalogue::default(),
            tools_dir_sources: Vec::new(),
        };
        cell.target = Arc::new(target);
        cell.rpm_sources = vec![PathBuf::from("./rpms")];

        let farms = prepare_rpm_farms(&cell, &dry_run_context()).unwrap();
        assert_eq!(farms.len(), 1);
        assert!(farms[0].writable, "a directory source is a writable farm");
        assert!(
            farms[0].arg_path.is_absolute(),
            "the farm path must be absolute, got `{}`",
            farms[0].arg_path.display()
        );
        assert!(
            farms[0].arg_path.starts_with(tmp.path()),
            "the farm must live beside the source under the image dir"
        );
    }

    /// A `.repo`-file `rpmSources` entry is a passthrough (createrepo does not run): not writable, and
    /// still absolutised for a valid bind/arg.
    #[test]
    fn a_repo_file_rpm_source_is_a_non_writable_absolute_passthrough() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("local.repo"), b"[local]\n").unwrap();

        let mut cell = dry_run_cell();
        let target = Target {
            definition: cell.target.definition.clone(),
            dir: tmp.path().to_path_buf(),
            default_outputs: Vec::new(),
            output_artifacts: OutputArtifactsPolicy::default(),
            root: tmp.path().to_path_buf(),
            base_images: BaseImageCatalogue::default(),
            tools_dir_sources: Vec::new(),
        };
        cell.target = Arc::new(target);
        cell.rpm_sources = vec![PathBuf::from("./local.repo")];

        let farms = prepare_rpm_farms(&cell, &dry_run_context()).unwrap();
        assert_eq!(farms.len(), 1);
        assert!(
            !farms[0].writable,
            "a .repo file is a passthrough, not a farm"
        );
        assert!(farms[0].arg_path.is_absolute());
    }

    #[derive(Debug, Default, Clone)]
    struct ExportRuntime {
        exports: Arc<Mutex<Vec<PathBuf>>>,
        pulls: Arc<Mutex<Vec<String>>>,
        runs: Arc<Mutex<usize>>,
    }

    impl ContainerRuntime for ExportRuntime {
        async fn pull_image(&self, reference: &str) -> Result<(), ExecError> {
            self.pulls.lock().unwrap().push(reference.to_owned());
            Ok(())
        }

        async fn inspect_image(
            &self,
            _reference: &str,
        ) -> Result<Option<tailor_core::LocalImage>, ExecError> {
            Ok(None)
        }

        async fn create_and_run(
            &self,
            _config: ContainerConfig,
            _cancel: CancellationToken,
        ) -> Result<ContainerResult, ExecError> {
            *self.runs.lock().unwrap() += 1;
            Ok(ContainerResult {
                exit_code: 0,
                logs: String::new(),
                failure_dump: None,
            })
        }

        async fn daemon_info(&self) -> Result<tailor_core::DaemonInfo, ExecError> {
            Ok(tailor_core::DaemonInfo::default())
        }

        async fn export_container(
            &self,
            _image_ref: &str,
            _platform: &str,
            _pull: bool,
            dest_dir: &Path,
            _cancel: CancellationToken,
        ) -> Result<(), ExecError> {
            std::fs::write(dest_dir.join("tdnf"), "mock").map_err(|source| ExecError::Io {
                context: "failed to write mock tools-dir export".to_owned(),
                source,
            })?;
            self.exports.lock().unwrap().push(dest_dir.to_path_buf());
            Ok(())
        }
    }

    #[tokio::test]
    async fn ensure_tools_dir_cache_exports_missing_cache_once() {
        let root = tempfile::Builder::new()
            .prefix("tailor-tools-dir-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        let runtime = ExportRuntime::default();
        let mut context = dry_run_context();
        context.dry_run = false;
        let plan = tailor_core::ToolsDirPlan {
            image_ref: "registry.example/tools@sha256:abc".to_owned(),
            digest: "sha256:abc".to_owned(),
            pull: true,
            cache_dir: root.path().join("cache/tools-dirs/sha256_abc"),
            mount_dir: root.path().join("build/gizmo/tools-dir"),
        };
        context.tools_dir = Some(plan.clone());

        ensure_tools_dir_cache(&runtime, &context, &plan, CancellationToken::new())
            .await
            .unwrap();
        ensure_tools_dir_cache(&runtime, &context, &plan, CancellationToken::new())
            .await
            .unwrap();

        assert_eq!(runtime.exports.lock().unwrap().len(), 1);
        assert!(
            root.path()
                .join("cache/tools-dirs/sha256_abc/tdnf")
                .exists()
        );
    }

    #[test]
    fn ic_run_failed_classifies_transport_and_exit_codes() {
        assert!(!ic_run_failed(&Ok(ContainerResult {
            exit_code: 0,
            logs: String::new(),
            failure_dump: None,
        })));
        assert!(ic_run_failed(&Ok(ContainerResult {
            exit_code: 1,
            logs: String::new(),
            failure_dump: None,
        })));
        assert!(ic_run_failed(&Err(ExecError::Runtime("boom".to_owned()))));
    }

    #[derive(Clone, Default)]
    struct FailingJanitor;

    impl ContainerRuntime for FailingJanitor {
        async fn pull_image(&self, _reference: &str) -> Result<(), ExecError> {
            Ok(())
        }

        async fn inspect_image(
            &self,
            _reference: &str,
        ) -> Result<Option<tailor_core::LocalImage>, ExecError> {
            Ok(None)
        }

        async fn create_and_run(
            &self,
            _config: ContainerConfig,
            _cancel: CancellationToken,
        ) -> Result<ContainerResult, ExecError> {
            Ok(ContainerResult {
                exit_code: 1,
                logs: "/bin/rm: cannot remove: Device or resource busy".to_owned(),
                failure_dump: None,
            })
        }

        async fn daemon_info(&self) -> Result<tailor_core::DaemonInfo, ExecError> {
            Ok(tailor_core::DaemonInfo::default())
        }

        async fn export_container(
            &self,
            _image_ref: &str,
            _platform: &str,
            _pull: bool,
            _dest_dir: &Path,
            _cancel: CancellationToken,
        ) -> Result<(), ExecError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn reclaim_subordinate_swallows_cleanup_error_when_ic_failed_but_propagates_otherwise() {
        // A tempdir outside the workspace, with a child to remove — so the janitor binds the safe
        // parent (the tempdir) and the FailingJanitor's non-zero exit (not a guard rejection) is what
        // the subordination logic sees.
        let dir = tempfile::Builder::new()
            .prefix("tailor-reclaim-")
            .tempdir()
            .unwrap();
        let target = dir.path().join("scratch");
        std::fs::create_dir(&target).unwrap();
        let executor = IcExecutor::new(FailingJanitor);
        let config = RuntimeConfig {
            janitor_image: "janitor@sha256:abc".to_owned(),
            ..RuntimeConfig::default()
        };
        let paths = [target];

        // IC already failed → the janitor EBUSY is swallowed so the IC error stays the headline.
        executor
            .reclaim_subordinate(&paths, &config, CancellationToken::new(), true)
            .await
            .unwrap();

        // IC succeeded → a cleanup failure is a genuine error and propagates.
        let err = executor
            .reclaim_subordinate(&paths, &config, CancellationToken::new(), false)
            .await
            .unwrap_err();
        assert!(matches!(err, ExecError::Runtime(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn run_ic_skips_pull_when_context_pull_is_false() {
        let runtime = ExportRuntime::default();
        let executor = IcExecutor::new(runtime.clone());
        let mut context = dry_run_context();
        context.dry_run = false;
        context.pull = false;

        executor
            .run_ic(
                &dry_run_cell(),
                &context,
                vec!["--version".to_owned()],
                &[],
                None,
                CancellationToken::new(),
            )
            .await
            .unwrap();

        assert!(runtime.pulls.lock().unwrap().is_empty());
        assert_eq!(*runtime.runs.lock().unwrap(), 1);
    }

    // A dry-run renders the invocation without ever calling the runtime; `NoopRuntime` (whose
    // methods all error) proves no daemon is contacted — so `build --dry-run` is engine-free.
    #[tokio::test]
    async fn dry_run_executes_without_contacting_the_runtime() {
        let executor = IcExecutor::new(NoopRuntime);
        let result = executor
            .execute(
                &dry_run_cell(),
                &dry_run_context(),
                CancellationToken::new(),
            )
            .await
            .expect("dry-run must not require a container engine");
        assert_eq!(result.exit_code, 0);
        assert!(result.logs.contains("sample_cosi"));
    }

    #[derive(Debug)]
    struct MockSigner;

    impl Signer for MockSigner {
        fn preflight(&self) -> Result<(), tailor_core::SignError> {
            Ok(())
        }
        fn sign(
            &self,
            _plan: &SigningPlan,
        ) -> Result<tailor_core::SigningResult, tailor_core::SignError> {
            Ok(tailor_core::SigningResult::default())
        }
    }

    // A signed `--dry-run` renders the real three-pass (customize → raw intermediate, sign, then
    // inject-files → final) without contacting the runtime (`NoopRuntime` errors on any call).
    #[tokio::test]
    async fn signed_dry_run_renders_the_three_pass_without_a_runtime() {
        let executor = IcExecutor::new(NoopRuntime);
        let mut context = dry_run_context();
        context.signer = Some(Arc::new(MockSigner));
        let result = executor
            .execute(&dry_run_cell(), &context, CancellationToken::new())
            .await
            .expect("signed dry-run must not require a container engine");
        assert_eq!(result.exit_code, 0);
        assert!(
            result.logs.contains("--output-image-format raw"),
            "pass 1 customizes to a raw intermediate:\n{}",
            result.logs
        );
        assert!(result.logs.contains("sample_cosi.intermediate.raw"));
        assert!(result.logs.contains("inject-files"), "pass 3 injects files");
        assert!(result.logs.contains("sign"), "pass 2 signs");
        assert!(result.logs.contains("ca_cert.pem"));
    }
}
