use std::path::{Path, PathBuf};

fn strip_root(path: &Path) -> &Path {
    match path.strip_prefix("/") {
        Ok(relative) => relative,
        Err(_) => path,
    }
}

/// Prepend '/host' to the given path.
pub fn host_relative(path: impl AsRef<Path>) -> PathBuf {
    Path::new("/host").join(strip_root(path.as_ref()))
}

/// Prepend '/mnt/newroot' to the given path.
pub fn newroot_relative(path: impl AsRef<Path>) -> PathBuf {
    Path::new("/mnt/newroot").join(strip_root(path.as_ref()))
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
    fn test_newroot_relative() {
        assert_eq!(newroot_relative("/etc"), Path::new("/mnt/newroot/etc"));
        assert_eq!(
            newroot_relative("/mnt/newroot/etc"),
            Path::new("/mnt/newroot/mnt/newroot/etc")
        );
        assert_eq!(newroot_relative("etc"), Path::new("/mnt/newroot/etc"));
    }
}
