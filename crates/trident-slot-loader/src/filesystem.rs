//! Bounded input reads and no-symlink filesystem operations for root-owned loader state.
//!
//! Callers must exclude concurrent untrusted writers. These checks are not a mount namespace sandbox.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};

/// An absent ancestor is absence, but a dangling symlink or non-directory is an error.
pub(super) fn directory_exists(path: impl AsRef<Path>) -> Result<bool, Error> {
    let mut current = PathBuf::new();
    for component in path.as_ref().components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                metadata.is_dir(),
                "{} is not a real directory",
                current.display()
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", current.display()))
            }
        }
    }
    Ok(true)
}

pub(super) fn directory(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref();
    ensure!(
        directory_exists(path)?,
        "missing directory {}",
        path.display()
    );
    Ok(())
}

pub(super) fn regular_file(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref();
    directory(path.parent().context("file has no parent")?)?;
    ensure!(
        fs::symlink_metadata(path)
            .with_context(|| format!("inspecting {}", path.display()))?
            .is_file(),
        "{} is not a regular file",
        path.display()
    );
    Ok(())
}

/// The descriptor is checked after opening; a substituted FIFO cannot block the reader.
pub(super) fn read_bounded(path: impl AsRef<Path>, limit: u64) -> Result<String, Error> {
    let path = path.as_ref();
    directory(path.parent().context("input has no parent")?)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    ensure!(
        file.metadata()
            .with_context(|| format!("checking open input {}", path.display()))?
            .is_file(),
        "{} is not a regular file",
        path.display()
    );
    let mut data = String::new();
    file.take(limit + 1)
        .read_to_string(&mut data)
        .with_context(|| format!("reading {}", path.display()))?;
    ensure!(
        data.len() as u64 <= limit,
        "{} exceeds {limit} bytes",
        path.display()
    );
    Ok(data)
}

pub(super) fn create_directory(path: impl AsRef<Path>) -> Result<(), Error> {
    let mut current = PathBuf::new();
    for component in path.as_ref().components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                metadata.is_dir(),
                "{} is not a real directory",
                current.display()
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .with_context(|| format!("creating {}", current.display()))?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", current.display()))
            }
        }
    }
    Ok(())
}

/// Missing files are harmless; existing symlinks, directories and special files are not ours to unlink.
pub(super) fn remove_regular(path: impl AsRef<Path>) -> Result<bool, Error> {
    let path = path.as_ref();
    let parent = path.parent().context("file has no parent")?;
    if !directory_exists(parent)? {
        return Ok(false);
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            ensure!(
                metadata.is_file(),
                "refusing to remove non-regular file {}",
                path.display()
            );
            fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
    }
}

pub(super) fn sync_directory(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref();
    File::open(path)?
        .sync_all()
        .with_context(|| format!("synchronizing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::symlink;

    #[test]
    fn missing_directories_are_distinguished_from_symlinked_ancestors() {
        let root = tempfile::tempdir().unwrap();
        assert!(!directory_exists(root.path().join("absent")).unwrap());
        symlink("absent", root.path().join("alias")).unwrap();
        directory_exists(root.path().join("alias/nested")).unwrap_err();
        create_directory(root.path().join("alias/nested")).unwrap_err();
        assert!(!root.path().join("absent").exists());
    }

    #[test]
    fn reads_are_bounded_and_do_not_follow_symlinks_or_wait_on_fifos() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("input");
        fs::write(&file, "1234").unwrap();
        assert_eq!(read_bounded(&file, 4).unwrap(), "1234");
        read_bounded(&file, 3).unwrap_err();
        symlink(&file, root.path().join("alias")).unwrap();
        read_bounded(root.path().join("alias"), 4).unwrap_err();
        let fifo = std::ffi::CString::new(root.path().join("fifo").as_os_str().as_encoded_bytes())
            .unwrap();
        // SAFETY: the C string is valid for this call and mkfifo retains no pointer.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        read_bounded(root.path().join("fifo"), 4).unwrap_err();
    }
}
