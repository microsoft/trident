use std::path::{Path, PathBuf};

use nix::unistd::{Gid, Uid};
use tokio_util::sync::CancellationToken;

use tailor_core::{ContainerConfig, ContainerRuntime, ExecError, LogSource, RuntimeConfig};

use crate::guard;

const CHOWN_BINARY: &str = "/bin/chown";
const RM_BINARY: &str = "/bin/rm";
const NAME_PREFIX: &str = "tailor-janitor";

/// The container platform the janitor runs as: the **host's native architecture**.
///
/// The janitor only runs `chown`/`rm` against bind-mounted host files, so it must run as a
/// container the host can execute natively — its architecture is irrelevant to the file operation
/// but decides whether the container can run at all. This must NOT be hardcoded: a fixed
/// `linux/amd64` fails with a 404 ("image ... does not provide the specified platform
/// (linux/amd64)") on an arm64 host, aborting an otherwise-successful build during cleanup.
///
/// Host arch (not the cell arch) is correct because the janitor operates on the host: it also fixes
/// `tailor clean` (no single cell arch) and a cross-arch build (an arm64 cell on an amd64 host still
/// cleans up natively as amd64, not under emulation). The janitor image is multi-arch and pinned by
/// its manifest-list tag/digest, so `--platform` selects the matching sub-manifest.
fn host_platform() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "linux/arm64",
        // tailor targets amd64/arm64; treat x86_64 and any other host as amd64.
        _ => "linux/amd64",
    }
}

pub(crate) async fn chown_paths<R: ContainerRuntime>(
    runtime: &R,
    paths: &[PathBuf],
    config: &RuntimeConfig,
    cancel: CancellationToken,
) -> Result<(), ExecError> {
    let existing = existing_paths(paths);
    if existing.is_empty() {
        return Ok(());
    }
    let uid = Uid::current().as_raw();
    let gid = Gid::current().as_raw();
    let mut args = vec![
        CHOWN_BINARY.to_owned(),
        "-h".to_owned(),
        "-R".to_owned(),
        format!("{uid}:{gid}"),
        "--".to_owned(),
    ];
    args.extend(existing.iter().map(|path| path_arg(path)));
    // chown never unlinks, so identity-binding each target is harmless (the target need not be
    // removable, only writable in place).
    let binds = existing.iter().map(|path| identity_bind(path)).collect();
    run_janitor(runtime, config, args, binds, cancel).await
}

pub(crate) async fn remove_paths<R: ContainerRuntime>(
    runtime: &R,
    paths: &[PathBuf],
    config: &RuntimeConfig,
    cancel: CancellationToken,
) -> Result<(), ExecError> {
    let existing = existing_paths(paths);
    if existing.is_empty() {
        return Ok(());
    }
    let mut args = vec![RM_BINARY.to_owned(), "-rf".to_owned(), "--".to_owned()];
    args.extend(existing.iter().map(|path| path_arg(path)));
    // Bind each target's PARENT, not the target itself. If we identity-bound the target, it would be
    // an active mountpoint inside the janitor container: `rm -rf` clears its contents but cannot
    // `rmdir` a busy mountpoint, so the top directory survives and rm exits non-zero (EBUSY) — the
    // recurring "cannot remove '<dir>': Device or resource busy" that failed every successful build.
    // With the parent bound, the target is an ordinary child directory that rm removes top-and-all.
    let binds = removal_binds(&existing)?;
    run_janitor(runtime, config, args, binds, cancel).await
}

/// Bind sources for a removal: each distinct target **parent**, guarded so the janitor never binds
/// `/`, an ancestor of the working directory, or another unsafe directory. The target itself is then
/// an ordinary child under its bound parent (see [`remove_paths`]).
fn removal_binds(paths: &[PathBuf]) -> Result<Vec<String>, ExecError> {
    let mut parents = std::collections::BTreeSet::new();
    for path in paths {
        let parent = path.parent().ok_or_else(|| ExecError::UnsafeDir {
            path: path.clone(),
            reason: "path has no parent directory to bind for removal".to_owned(),
        })?;
        guard::ensure_safe_removal_parent(parent)?;
        parents.insert(parent.to_path_buf());
    }
    Ok(parents.iter().map(|parent| identity_bind(parent)).collect())
}

async fn run_janitor<R: ContainerRuntime>(
    runtime: &R,
    runtime_config: &RuntimeConfig,
    args: Vec<String>,
    binds: Vec<String>,
    cancel: CancellationToken,
) -> Result<(), ExecError> {
    if runtime_config.janitor_image.is_empty() {
        return Err(ExecError::Runtime(
            "no janitor image configured: set `runtime.janitorImage` in tailor.yaml to normalize \
             ownership of Image Customizer's root-owned outputs"
                .to_owned(),
        ));
    }
    runtime.pull_image(&runtime_config.janitor_image).await?;
    let result = runtime
        .create_and_run(
            ContainerConfig {
                image_ref: runtime_config.janitor_image.clone(),
                platform: host_platform().to_owned(),
                name: format!("{NAME_PREFIX}-{}", std::process::id()),
                args,
                binds,
                privileged: false,
                cell_slug: String::new(),
                log_source: LogSource::Janitor,
                log_file: None,
            },
            cancel,
        )
        .await?;
    if result.exit_code == 0 {
        Ok(())
    } else {
        Err(ExecError::Runtime(format!(
            "janitor exited with code {}: {}",
            result.exit_code, result.logs
        )))
    }
}

fn existing_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths.iter().filter(|path| path.exists()).cloned().collect()
}

fn identity_bind(path: &Path) -> String {
    let path = path.to_string_lossy();
    format!("{path}:{path}")
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_platform_matches_the_running_arch() {
        let platform = host_platform();
        assert!(
            platform == "linux/amd64" || platform == "linux/arm64",
            "got {platform}"
        );
        #[cfg(target_arch = "aarch64")]
        assert_eq!(platform, "linux/arm64");
        #[cfg(target_arch = "x86_64")]
        assert_eq!(platform, "linux/amd64");
    }

    #[test]
    fn removal_binds_bind_the_parent_not_the_target() {
        // Regression: a removal must bind the target's PARENT, so the target is an ordinary child
        // (not a mountpoint) that `rm -rf` can remove top-and-all. Binding the target itself made it
        // a busy mountpoint whose `rmdir` returned EBUSY on every successful build.
        let target = PathBuf::from("/data/tbuild/myimage_arm64_vhd-fixed/tools-dir");
        let parent = "/data/tbuild/myimage_arm64_vhd-fixed";

        let binds = removal_binds(std::slice::from_ref(&target)).unwrap();

        assert_eq!(binds, vec![format!("{parent}:{parent}")]);
        assert!(
            !binds.iter().any(|bind| bind.contains("tools-dir")),
            "must not bind the target itself, got {binds:?}"
        );
    }

    #[test]
    fn removal_binds_dedup_a_shared_parent() {
        let a = PathBuf::from("/out/gizmo_amd64_cosi.cosi");
        let b = PathBuf::from("/out/gizmo_amd64_cosi.ca_cert.pem");

        let binds = removal_binds(&[a, b]).unwrap();

        assert_eq!(binds, vec!["/out:/out".to_owned()]);
    }

    #[test]
    fn removal_binds_reject_a_target_whose_parent_is_root() {
        // Parent of `/foo` is `/`; the janitor must never bind the filesystem root.
        let err = removal_binds(&[PathBuf::from("/foo")]).unwrap_err();
        assert!(matches!(err, ExecError::UnsafeDir { .. }), "got {err:?}");
    }
}
