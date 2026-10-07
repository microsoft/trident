use std::{
    env,
    path::{Component, Path, PathBuf},
};

use tailor_core::ExecError;

const ROOT_PATH: &str = "/";

/// Well-known system directories a build/scratch dir must never *be*: IC recursively deletes the
/// build dir, so pointing it at one of these (or `$HOME`) would be catastrophic. This rejects the
/// directory itself, not paths beneath it — a build dir like `/home/user/scratch` is fine.
const PROTECTED_DIRS: &[&str] = &[
    "/bin", "/boot", "/dev", "/etc", "/home", "/lib", "/lib32", "/lib64", "/opt", "/proc", "/root",
    "/run", "/sbin", "/srv", "/sys", "/usr", "/var",
];

pub(crate) fn ensure_safe_build_dir(path: &Path) -> Result<(), ExecError> {
    ensure_safe_dir(path)
}

pub(crate) fn ensure_safe_rw_target(path: &Path) -> Result<(), ExecError> {
    ensure_safe_dir(path)
}

/// Guard a directory the janitor will bind read-write in order to remove a **named child** under it
/// (`janitor::remove_paths`). Unlike [`ensure_safe_rw_target`], this does not reject a parent that
/// contains the working directory: only the named child is deleted, so binding e.g. the workspace or
/// image directory is fine, and running `tailor` from inside such a directory must still clean up.
/// The one catastrophe is binding the filesystem root (re-exposing the whole host to a `rm -rf`), so
/// that is the only rejection.
pub(crate) fn ensure_safe_removal_parent(path: &Path) -> Result<(), ExecError> {
    let normalized = resolve_path(path)?;
    if normalized == Path::new(ROOT_PATH) {
        return Err(unsafe_dir(
            normalized,
            "refusing to bind the filesystem root to remove a child".to_owned(),
        ));
    }
    Ok(())
}

/// Guard a directory tailor will bind read-write and IC may recursively delete. Rejects the
/// filesystem root, a well-known system directory or `$HOME`, and any directory that *contains* the
/// current working directory (deleting it would take out the cwd). It deliberately does **not**
/// require a separate filesystem: IC keeps its overlays and mounts within `--build-dir`/`--tools-dir`
/// (see `docs/explanation/threat-model.md`), so a build dir on the same device as `/` is safe.
fn ensure_safe_dir(path: &Path) -> Result<(), ExecError> {
    let normalized = resolve_path(path)?;
    if normalized == Path::new(ROOT_PATH) {
        return Err(unsafe_dir(
            normalized,
            "must not be the filesystem root".to_owned(),
        ));
    }
    if is_protected_dir(&normalized) {
        return Err(unsafe_dir(
            normalized,
            "must not be a system directory or the home directory".to_owned(),
        ));
    }

    let cwd = resolve_path(&env::current_dir().map_err(|source| ExecError::Io {
        context: "failed to determine current directory".to_owned(),
        source,
    })?)?;
    if cwd.starts_with(&normalized) {
        return Err(unsafe_dir(
            normalized,
            format!(
                "must not contain the current working directory `{}`",
                cwd.display()
            ),
        ));
    }

    Ok(())
}

/// Whether `normalized` is a well-known system directory (see [`PROTECTED_DIRS`]) or `$HOME`.
fn is_protected_dir(normalized: &Path) -> bool {
    if PROTECTED_DIRS
        .iter()
        .any(|dir| normalized == Path::new(dir))
    {
        return true;
    }
    env::var_os("HOME").is_some_and(|home| {
        !home.is_empty() && resolve_path(Path::new(&home)).is_ok_and(|home| normalized == home)
    })
}

fn unsafe_dir(path: PathBuf, reason: String) -> ExecError {
    ExecError::UnsafeDir { path, reason }
}

/// Guard a per-cell scratch directory that the build-dir reap removes **recursively**: on top of the
/// generic [`ensure_safe_build_dir`] checks, it must be a *strict* descendant of its scratch `base`,
/// so a `.`/`..` slug cannot normalize to the base or its parent and delete unrelated files. Symlinks
/// are resolved throughout. Overlap with inputs/outputs is prevented at the base level by
/// [`ensure_build_dir_base_isolated`].
pub(crate) fn ensure_safe_scratch_dir(scratch: &Path, base: &Path) -> Result<(), ExecError> {
    ensure_safe_build_dir(scratch)?;
    let scratch = resolve_path(scratch)?;
    let base = resolve_path(base)?;
    if scratch == base || !scratch.starts_with(&base) {
        return Err(unsafe_dir(
            scratch,
            format!(
                "must be a strict subdirectory of the build-dir base `{}`",
                base.display()
            ),
        ));
    }
    Ok(())
}

/// Validate a `--build-dir-base` against the obvious footgun: because the reap removes
/// `<base>/<slug>` recursively, the base must not *contain* a `protected` directory — the workspace
/// root (every in-workspace input) or the output directory (artifacts, and the cache/log dirs that
/// default under it). Rejecting a base that is an ancestor of (or equal to) either keeps the reap
/// from deleting inputs or outputs. `--build-dir-base` is an advanced flag: inputs supplied as
/// absolute paths *outside* the workspace that a user parks under the base are deliberately not
/// covered — the base is contracted to be a dedicated scratch directory. Symlinks are resolved.
pub fn ensure_build_dir_base_isolated(base: &Path, protected: &[&Path]) -> Result<(), ExecError> {
    let base = resolve_path(base)?;
    for path in protected {
        let path = resolve_path(path)?;
        if path.starts_with(&base) {
            return Err(unsafe_dir(
                base,
                format!("the build-dir base must not contain `{}`", path.display()),
            ));
        }
    }
    Ok(())
}

/// Absolutize and lexically collapse `.`/`..`, then **resolve symlinks** on the deepest existing
/// ancestor, re-attaching any not-yet-created leaf lexically. A purely lexical check treats a
/// symlinked build/scratch dir like `/tmp/tailor-build -> /` as safe; resolving the existing
/// components first makes the guard test the symlink's real target, so a symlink cannot smuggle the
/// build dir onto `/` or a system directory before `create_dir_all` and the read-write bind follow it.
fn resolve_path(path: &Path) -> Result<PathBuf, ExecError> {
    let lexical = normalize_absolute_lexical(path)?;
    // Walk up to the deepest existing ancestor (`exists()` follows symlinks; `/` always exists, so
    // this terminates), collecting the not-yet-created leaf components.
    let mut existing = lexical.as_path();
    let mut leaf: Vec<std::ffi::OsString> = Vec::new();
    while !existing.exists() {
        match (existing.file_name(), existing.parent()) {
            (Some(name), Some(parent)) => {
                leaf.push(name.to_os_string());
                existing = parent;
            }
            _ => break,
        }
    }
    let mut resolved = existing.canonicalize().map_err(|source| ExecError::Io {
        context: format!("failed to resolve `{}`", existing.display()),
        source,
    })?;
    for name in leaf.iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn normalize_absolute_lexical(path: &Path) -> Result<PathBuf, ExecError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|source| ExecError::Io {
                context: "failed to determine current directory".to_owned(),
                source,
            })?
            .join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new(ROOT_PATH)),
            Component::CurDir => {}
            Component::ParentDir => {
                if normalized != Path::new(ROOT_PATH) {
                    normalized.pop();
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    if normalized.as_os_str().is_empty() {
        Ok(PathBuf::from(ROOT_PATH))
    } else {
        Ok(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_filesystem_root() {
        let err = ensure_safe_build_dir(Path::new(ROOT_PATH)).unwrap_err();
        assert!(matches!(err, ExecError::UnsafeDir { .. }));
    }

    #[test]
    fn scratch_dir_enforces_strict_descent() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("scratch");
        let child = base.join("cell");
        std::fs::create_dir_all(&child).unwrap();

        // A strict child of the base is fine.
        ensure_safe_scratch_dir(&child, &base).unwrap();

        // The base itself (a `.` slug) and its parent (a `..` slug) are not strict descendants.
        assert!(matches!(
            ensure_safe_scratch_dir(&base, &base).unwrap_err(),
            ExecError::UnsafeDir { .. }
        ));
        assert!(matches!(
            ensure_safe_scratch_dir(tmp.path(), &base).unwrap_err(),
            ExecError::UnsafeDir { .. }
        ));
    }

    #[test]
    fn build_dir_base_rejects_containing_protected_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("scratch");
        std::fs::create_dir_all(&base).unwrap();

        // A base that contains the output dir (output nested under the base) is rejected.
        let output = base.join("out");
        std::fs::create_dir_all(&output).unwrap();
        assert!(matches!(
            ensure_build_dir_base_isolated(&base, &[output.as_path()]).unwrap_err(),
            ExecError::UnsafeDir { .. }
        ));
        // The base itself as a protected dir (base == output) is rejected.
        assert!(matches!(
            ensure_build_dir_base_isolated(&base, &[base.as_path()]).unwrap_err(),
            ExecError::UnsafeDir { .. }
        ));

        // A base that is disjoint from (or nested under) the protected dirs is fine — this is the
        // default layout, where the scratch base sits *under* the output dir.
        let workspace = tmp.path().join("ws");
        let output = workspace.join("artifacts");
        let default_base = output.join(".tailor").join("build");
        std::fs::create_dir_all(&default_base).unwrap();
        ensure_build_dir_base_isolated(&default_base, &[workspace.as_path(), output.as_path()])
            .unwrap();
    }

    #[test]
    fn rejects_ancestor_of_current_working_dir() {
        let cwd = std::env::current_dir().unwrap();

        let err = ensure_safe_rw_target(&cwd).unwrap_err();

        assert!(matches!(err, ExecError::UnsafeDir { .. }));
    }

    #[test]
    fn removal_parent_rejects_root_but_allows_a_cwd_containing_dir() {
        // The removal-parent guard is narrower than `ensure_safe_rw_target`: only the filesystem
        // root is refused. A directory that contains the cwd (e.g. running `tailor` from inside an
        // image dir whose staging is reclaimed) must be allowed, since only a named child is deleted.
        let root_err = ensure_safe_removal_parent(Path::new(ROOT_PATH)).unwrap_err();
        assert!(matches!(root_err, ExecError::UnsafeDir { .. }));

        let cwd = std::env::current_dir().unwrap();
        ensure_safe_removal_parent(&cwd).unwrap();
    }

    #[test]
    fn same_device_build_dir_is_allowed() {
        // A build dir on the same filesystem as `/` is now accepted: IC keeps its overlays/mounts
        // within the build dir, so a separate device is no longer required.
        let temp = tempfile::Builder::new()
            .prefix("tailor-guard-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        let build_dir = temp.path().join("scratch");
        ensure_safe_build_dir(&build_dir).unwrap();
        ensure_safe_rw_target(&build_dir).unwrap();
    }

    #[test]
    fn rejects_protected_system_dirs() {
        for dir in ["/usr", "/etc", "/home", "/var", "/boot"] {
            let err = ensure_safe_build_dir(Path::new(dir)).unwrap_err();
            assert!(
                matches!(err, ExecError::UnsafeDir { .. }),
                "expected `{dir}` to be rejected"
            );
        }
    }

    #[test]
    fn normalizes_without_requiring_leaf_to_exist() {
        // A non-existent, lexically-normalized build dir under the cwd is accepted (tailor creates
        // it); normalization collapses the `..` without touching the filesystem.
        let base = std::env::current_dir().unwrap().join("does-not-exist");
        let candidate = base.join("..").join("does-not-exist").join("scratch");
        ensure_safe_build_dir(&candidate).unwrap();
    }

    #[test]
    fn rejects_a_symlink_to_the_filesystem_root() {
        // A lexical-only guard would accept `<tmp>/link` (it isn't literally `/`); resolving the
        // symlink reveals it points at `/`, which must be rejected — this is the exact bypass that
        // could re-expose the whole host to IC's recursive delete.
        let temp = tempfile::Builder::new()
            .prefix("tailor-guard-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(Path::new(ROOT_PATH), &link).unwrap();
        let err = ensure_safe_build_dir(&link).unwrap_err();
        assert!(matches!(err, ExecError::UnsafeDir { .. }), "got {err:?}");
        // The removal-parent guard (root-only) must also see through the symlink.
        let err = ensure_safe_removal_parent(&link).unwrap_err();
        assert!(matches!(err, ExecError::UnsafeDir { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_a_symlink_to_a_system_directory() {
        let temp = tempfile::Builder::new()
            .prefix("tailor-guard-")
            .tempdir_in(std::env::current_dir().unwrap())
            .unwrap();
        let link = temp.path().join("link");
        std::os::unix::fs::symlink(Path::new("/etc"), &link).unwrap();
        let err = ensure_safe_build_dir(&link).unwrap_err();
        assert!(matches!(err, ExecError::UnsafeDir { .. }), "got {err:?}");
    }
}
