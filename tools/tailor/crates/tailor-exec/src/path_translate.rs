use std::path::{Path, PathBuf};

const DEFAULT_HOST_ROOT: &str = "/host";

pub fn to_container_path(host_path: &Path, host_root: &Path) -> String {
    let root = if host_root.as_os_str().is_empty() {
        Path::new(DEFAULT_HOST_ROOT)
    } else {
        host_root
    };
    let suffix = host_path.strip_prefix(Path::new("/")).unwrap_or(host_path);
    let mut translated = PathBuf::from(root);
    translated.push(suffix);
    translated.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::Path;

    #[test]
    fn translates_absolute_path_under_host_root() {
        assert_eq!(
            to_container_path(Path::new("/home/me/image.vhdx"), Path::new("/host")),
            "/host/home/me/image.vhdx"
        );
    }

    #[test]
    fn empty_host_root_uses_default() {
        assert_eq!(
            to_container_path(Path::new("/var/lib/x"), Path::new("")),
            "/host/var/lib/x"
        );
    }

    #[test]
    fn custom_host_root_is_respected() {
        assert_eq!(
            to_container_path(Path::new("/work/out.raw"), Path::new("/mnt/host")),
            "/mnt/host/work/out.raw"
        );
    }
}
