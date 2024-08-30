use std::path::{Path, PathBuf};

use crate::container;

fn strip_root(path: &Path) -> &Path {
    match path.strip_prefix("/") {
        Ok(relative) => relative,
        Err(_) => path,
    }
}

/// Returns the path obtained by prepending the path to the root of the host filesystem to the
/// given path.
pub fn host_relative(path: impl AsRef<Path>) -> PathBuf {
    Path::new(container::HOST_ROOT_PATH).join(strip_root(path.as_ref()))
}

/// Returns the path obtained by joining the given base path with the given relative path.
pub fn join_relative(base: impl AsRef<Path>, relative: impl AsRef<Path>) -> PathBuf {
    base.as_ref().join(strip_root(relative.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_relative() {
        assert_eq!(host_relative("/etc"), Path::new("/host/etc"));
        assert_eq!(host_relative("/host/etc"), Path::new("/host/host/etc"));
        assert_eq!(host_relative("etc"), Path::new("/host/etc"));
    }

    #[test]
    fn test_join_relative() {
        assert_eq!(join_relative("/etc", "passwd"), Path::new("/etc/passwd"));
        assert_eq!(join_relative("/etc", "/passwd"), Path::new("/etc/passwd"));
        assert_eq!(
            join_relative("/etc", "/etc/passwd"),
            Path::new("/etc/etc/passwd")
        );
    }
}
