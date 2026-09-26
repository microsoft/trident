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
    let normalized = normalize_absolute_lexical(path)?;
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
    let normalized = normalize_absolute_lexical(path)?;
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

    let cwd = normalize_absolute_lexical(&env::current_dir().map_err(|source| ExecError::Io {
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
        !home.is_empty()
            && normalize_absolute_lexical(Path::new(&home)).is_ok_and(|home| normalized == home)
    })
}

fn unsafe_dir(path: PathBuf, reason: String) -> ExecError {
    ExecError::UnsafeDir { path, reason }
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
}
