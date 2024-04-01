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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_host_relative() {
        assert_eq!(host_relative("/etc"), Path::new("/host/etc"));
        assert_eq!(host_relative("/host/etc"), Path::new("/host/host/etc"));
        assert_eq!(host_relative("etc"), Path::new("/host/etc"));
    }
}
