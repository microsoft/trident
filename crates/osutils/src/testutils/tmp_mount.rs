use std::path::{Path, PathBuf};

use crate::{filesystems::MountFileSystemType, mount};

struct TempGuard(PathBuf);

impl Drop for TempGuard {
    fn drop(&mut self) {
        mount::umount(&self.0, false).unwrap();
    }
}

/// Mounts a filesystem at a temporary directory and calls the provided function
/// with the path.
pub fn mount(
    path: impl AsRef<Path>,
    filesystem: MountFileSystemType,
    options: &[String],
    mut f: impl FnMut(&Path),
) {
    let mount_dir = tempfile::tempdir().unwrap();
    mount::mount(path, mount_dir.path(), filesystem, options).unwrap();
    let _guard = TempGuard(mount_dir.path().to_path_buf());
    f(mount_dir.path());
}
