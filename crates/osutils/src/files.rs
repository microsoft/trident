use std::{
    fs::{self, File, Permissions},
    io::{Read, Write},
    os::{linux::fs::MetadataExt, unix::fs::PermissionsExt},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};

/// Creates a file and all parent directories if they don't exist
pub fn create_file<S>(path: S) -> Result<File, Error>
where
    S: AsRef<Path>,
{
    if let Some(parent) = path.as_ref().parent() {
        create_dirs(parent)?;
    }

    File::create(path.as_ref()).context(format!(
        "Could not create file: {}",
        path.as_ref().display()
    ))
}

/// Creates a file and all parent directories if they don't exist, and sets the file mode
pub fn create_file_mode<S>(path: S, mode: u32) -> Result<File, Error>
where
    S: AsRef<Path>,
{
    let file = create_file(path.as_ref())?;
    fs::set_permissions(path.as_ref(), Permissions::from_mode(mode)).context(format!(
        "Could not set permissions {:#o} for file {}",
        mode,
        path.as_ref().display()
    ))?;
    Ok(file)
}

/// Creates all directories in a path if they don't exist
pub fn create_dirs<S>(path: S) -> Result<(), Error>
where
    S: AsRef<Path>,
{
    fs::create_dir_all(path.as_ref()).context(format!(
        "Could not create path: {}",
        path.as_ref().display()
    ))
}

/// Creates a file with a random name in the specified location
/// It creates all parent directories if they don't exist
pub fn create_random_file<S>(location: S) -> Result<(File, PathBuf), Error>
where
    S: AsRef<Path>,
{
    create_dirs(location.as_ref())?;
    tempfile::NamedTempFile::new_in(location)
        .context("Failed to create temporary file")?
        .keep()
        .context("Failed to persist file")
}

/// Reads the content of a file and trims it
pub fn read_file_trim<S>(file_path: &S) -> Result<String, Error>
where
    S: AsRef<Path>,
{
    let content = fs::read_to_string(file_path.as_ref()).context(format!(
        "Could not read file contents: {:?}",
        file_path.as_ref()
    ))?;
    Ok(content.trim().to_string())
}

/// Prepends to a file
pub fn prepend_file<S>(path: S, must_exist: bool, new_contents: &[u8]) -> Result<(), Error>
where
    S: AsRef<Path>,
{
    let mut mode = 0o600;
    let mut content = new_contents.to_owned();
    if path.as_ref().exists() {
        if !path.as_ref().is_file() {
            bail!("Path exists but is not a file: {}", path.as_ref().display());
        }

        mode = fs::metadata(path.as_ref())
            .context(format!(
                "Could not get metadata for {}",
                path.as_ref().display()
            ))?
            .permissions()
            .mode();

        File::open(path.as_ref())
            .context(format!("Could not open file: {}", path.as_ref().display()))?
            .read_to_end(&mut content)
            .context(format!(
                "Failed to read existing contents of {}",
                path.as_ref().display(),
            ))?;
    } else if must_exist {
        bail!("Path does not exist: {}", path.as_ref().display());
    }

    let mut file = create_file_mode(path.as_ref(), mode).context(format!(
        "Could not create file: {}",
        path.as_ref().display()
    ))?;

    file.write_all(&content).context(format!(
        "Could not write to file: {}",
        path.as_ref().display()
    ))?;

    Ok(())
}

/// Writes to a file
pub fn write_file<S>(path: S, mode: u32, contents: &[u8]) -> Result<(), Error>
where
    S: AsRef<Path>,
{
    let mut file = create_file_mode(path.as_ref(), mode).context(format!(
        "Could not create file: {}",
        path.as_ref().display()
    ))?;

    file.write_all(contents).context(format!(
        "Could not write to file: {}",
        path.as_ref().display()
    ))?;

    Ok(())
}

pub fn get_owner_uid<S>(path: S) -> Result<u32, Error>
where
    S: AsRef<Path>,
{
    Ok(path
        .as_ref()
        .metadata()
        .context(format!(
            "Failed to get metadata for {}",
            path.as_ref().display()
        ))?
        .st_uid())
}

pub fn get_owner_gid<S>(path: S) -> Result<u32, Error>
where
    S: AsRef<Path>,
{
    Ok(path
        .as_ref()
        .metadata()
        .context(format!(
            "Failed to get metadata for {}",
            path.as_ref().display()
        ))?
        .st_gid())
}

pub fn clean_directory<S>(path: S) -> Result<(), Error>
where
    S: AsRef<Path>,
{
    let path = path.as_ref();
    if !path.exists() {
        return Ok(());
    }

    if !path.is_dir() {
        bail!("Path exists but is not a directory: {}", path.display());
    }

    fs::read_dir(path)
        .context(format!(
            "Failed to read contents of directory {}",
            path.display()
        ))?
        .try_for_each(|entry| {
            let path = entry.context("Failed to read entry")?.path();
            if path.is_dir() {
                fs::remove_dir_all(&path)
                    .with_context(|| format!("Failed to remove directory: {}", path.display()))
            } else {
                fs::remove_file(&path)
                    .with_context(|| format!("Failed to remove file: {}", path.display()))
            }
        })
}

/// Log detailed diagnostics when an atomic rename fails.
///
/// Captures: errno, device IDs (same-device check), filesystem type,
/// mount options, SELinux labels, and directory permissions. This gives
/// enough information to understand *why* the rename failed and whether
/// the root cause is fixable.
fn log_rename_failure_diagnostics(tmp_path: &Path, target_path: &Path, error: &std::io::Error) {
    use std::os::unix::fs::MetadataExt;

    let errno_info = error
        .raw_os_error()
        .map(|e| format!("errno={e} ({})", errno_name(e)))
        .unwrap_or_else(|| format!("{error}"));

    log::warn!(
        "Atomic rename failed: '{}' -> '{}': {errno_info}",
        tmp_path.display(),
        target_path.display(),
    );

    // Compare st_dev to check if source and target are on the same device.
    // Different devices means rename() will always fail with EXDEV.
    let tmp_dev = fs::metadata(tmp_path).map(|m| m.dev()).ok();
    let tgt_dev = target_path
        .parent()
        .and_then(|p| fs::metadata(p).map(|m| m.dev()).ok());

    match (tmp_dev, tgt_dev) {
        (Some(td), Some(pd)) if td != pd => {
            log::warn!(
                "  Device mismatch: tmp dev={td:#x}, target parent dev={pd:#x} (cross-device rename will always fail)"
            );
        }
        (Some(td), Some(pd)) => {
            log::warn!("  Same device: dev={td:#x} (target parent dev={pd:#x})");
        }
        _ => {
            log::warn!(
                "  Could not compare devices: tmp={tmp_dev:?}, target_parent={tgt_dev:?}"
            );
        }
    }

    // Log mount info from /proc/self/mountinfo for both paths.
    if let Ok(mountinfo) = fs::read_to_string("/proc/self/mountinfo") {
        for (label, p) in [("tmp", tmp_path), ("target", target_path)] {
            // For mount matching, canonicalize the parent and join the filename.
            // This avoids following symlinks on the target itself, which would
            // report the wrong mount if target_path is a symlink.
            let canonical = p
                .parent()
                .and_then(|pp| {
                    fs::canonicalize(pp)
                        .ok()
                        .map(|cp| cp.join(p.file_name().unwrap_or_default()))
                })
                .or_else(|| fs::canonicalize(p).ok());

            if let Some(canonical) = canonical {
                // Find the mount entry with the longest matching prefix
                // using Path::starts_with for component-aware comparison.
                let best_mount = mountinfo
                    .lines()
                    .filter_map(|line| parse_mountinfo_mount_point(line))
                    .filter(|mp| canonical.starts_with(mp))
                    .max_by_key(|mp| mp.as_os_str().len());

                if let Some(mount_point) = &best_mount {
                    // Find the full line for this mount point to extract fs type and options.
                    if let Some(full_line) = mountinfo.lines().find(|line| {
                        parse_mountinfo_mount_point(line).as_ref() == Some(mount_point)
                    }) {
                        log::warn!("  {label} mount ({}): {full_line}", canonical.display());
                    } else {
                        log::warn!(
                            "  {label} mount ({}): mount_point={}",
                            canonical.display(),
                            mount_point.display()
                        );
                    }
                } else {
                    log::warn!(
                        "  {label} mount ({}): no matching mount found",
                        canonical.display()
                    );
                }
            }
        }
    }

    // Log SELinux labels if available (via /proc/self/attr/current and file xattrs).
    for (label, p) in [("tmp", tmp_path), ("target", target_path)] {
        match xattr_security_selinux(p) {
            Some(ctx) => log::warn!("  {label} SELinux label: {ctx}"),
            None => log::warn!("  {label} SELinux label: (not available)"),
        }
    }

    // Log directory permissions for the parent.
    if let Some(parent) = target_path.parent() {
        if let Ok(meta) = fs::metadata(parent) {
            log::warn!(
                "  target parent dir permissions: {:#o}, uid={}, gid={}",
                meta.permissions().mode(),
                meta.uid(),
                meta.gid(),
            );
        }
    }
}

/// Map common errno values to symbolic names for readability.
fn errno_name(errno: i32) -> &'static str {
    match errno {
        libc::EXDEV => "EXDEV: cross-device link",
        libc::EACCES => "EACCES: permission denied",
        libc::EPERM => "EPERM: operation not permitted",
        libc::ENOENT => "ENOENT: no such file or directory",
        libc::ENOTDIR => "ENOTDIR: not a directory",
        libc::EISDIR => "EISDIR: is a directory",
        libc::ENOTEMPTY => "ENOTEMPTY: directory not empty",
        libc::EROFS => "EROFS: read-only file system",
        libc::EBUSY => "EBUSY: device or resource busy",
        _ => "unknown",
    }
}

/// Extract the mount point (field 5, 0-indexed) from a /proc/self/mountinfo line.
/// Unescapes octal sequences (e.g., `\040` for space) used by the kernel.
fn parse_mountinfo_mount_point(line: &str) -> Option<PathBuf> {
    // mountinfo format: id parent_id major:minor root mount_point options ...
    line.split_whitespace()
        .nth(4)
        .map(|s| PathBuf::from(unescape_mountinfo(s)))
}

/// Unescape octal sequences in mountinfo fields (e.g., `\040` -> ` `).
fn unescape_mountinfo(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            let oct: String = chars.by_ref().take(3).collect();
            if let Ok(val) = u8::from_str_radix(&oct, 8) {
                result.push(val as char);
            } else {
                result.push('\\');
                result.push_str(&oct);
            }
        } else {
            result.push(c);
        }
    }
    result
}

/// Read the security.selinux extended attribute from a path, if present.
fn xattr_security_selinux(path: &Path) -> Option<String> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let c_name = CString::new("security.selinux").ok()?;

    // First call with size 0 to get the required buffer size.
    let size =
        unsafe { libc::lgetxattr(c_path.as_ptr(), c_name.as_ptr(), std::ptr::null_mut(), 0) };

    if size < 0 {
        let err = std::io::Error::last_os_error();
        let detail = match err.raw_os_error() {
            Some(libc::ENODATA) => "no label set".to_string(),
            Some(libc::EOPNOTSUPP) => "xattrs not supported on this filesystem".to_string(),
            Some(libc::EACCES) => "permission denied reading xattr".to_string(),
            _ => format!("error: {err}"),
        };
        return Some(detail);
    }

    let mut buf = vec![0u8; size as usize];
    let read = unsafe {
        libc::lgetxattr(
            c_path.as_ptr(),
            c_name.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
        )
    };

    if read < 0 {
        return Some(format!("error on second read: {}", std::io::Error::last_os_error()));
    }

    buf.truncate(read as usize);
    // SELinux labels are null-terminated C strings.
    let s = String::from_utf8_lossy(&buf);
    Some(s.trim_end_matches('\0').to_string())
}

/// Atomically replace `path` with `content`.
///
/// Writes to a temp file in the same directory, fsyncs, preserves ownership
/// and permissions from the original file (if it exists), then renames. This
/// guarantees that readers never see a partial write.
pub fn atomic_write_file(path: &Path, content: &str) -> Result<(), Error> {
    use std::os::unix::fs::MetadataExt;

    let parent = path.parent().context("Cannot determine parent directory")?;

    let mut tmp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("Failed to create temp file in '{}'", parent.display()))?;

    tmp.write_all(content.as_bytes())
        .with_context(|| format!("Failed to write temp file for '{}'", path.display()))?;

    tmp.flush()
        .with_context(|| format!("Failed to flush temp file for '{}'", path.display()))?;

    // fsync the temp file before rename to ensure data is on disk. Without
    // this, a power loss between rename and dirty-page flush could leave the
    // file zero-length.
    tmp.as_file()
        .sync_all()
        .with_context(|| format!("Failed to fsync temp file for '{}'", path.display()))?;

    // Preserve ownership and permissions from the original file if it exists.
    // Ownership must be set before permissions because chown can clear
    // setuid/setgid bits.
    if let Ok(metadata) = fs::metadata(path) {
        use std::os::fd::AsFd;
        nix::unistd::fchown(
            tmp.as_file().as_fd(),
            Some(nix::unistd::Uid::from_raw(metadata.uid())),
            Some(nix::unistd::Gid::from_raw(metadata.gid())),
        )
        .with_context(|| {
            format!(
                "Failed to set ownership on temp file for '{}'",
                path.display()
            )
        })?;

        fs::set_permissions(tmp.path(), metadata.permissions()).with_context(|| {
            format!(
                "Failed to set permissions on temp file for '{}'",
                path.display()
            )
        })?;
    }

    match tmp.persist(path) {
        Ok(_) => {
            // Sync parent directory to ensure the rename (directory entry
            // update) is durable. Without this, the old file could reappear
            // after power loss.
            if let Some(parent) = path.parent() {
                if let Ok(dir) = fs::File::open(parent) {
                    let _ = dir.sync_all();
                }
            }
        }
        Err(e) => {
            // Rename can fail across mount boundaries (EXDEV), in overlay/
            // bind-mount configurations (EACCES/EPERM), or under SELinux
            // restrictions. Log detailed diagnostics so we can identify the
            // root cause rather than silently relying on the fallback.
            log_rename_failure_diagnostics(e.file.path(), path, &e.error);

            // TODO: Restore fallback once root cause is understood.
            // Temporarily fail hard so pipeline logs capture diagnostics.
            bail!(
                "Atomic rename failed for '{}': {} — see diagnostic logs above for details",
                path.display(),
                e.error
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn test_create_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("some/path").join("test.txt");
        create_file(&path).unwrap();
        assert!(path.exists());
        assert!(path.is_file());
    }

    #[test]
    fn test_create_file_mode() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let file = create_file_mode(&path, 0o600).unwrap();
        assert!(path.exists());
        assert!(path.is_file());
        // Bitwise AND to ignore the file type
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn test_create_dirs() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test").join("test2");
        create_dirs(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_prepend_file() {
        let dir = tempdir().unwrap();
        let (mut file, path) = create_random_file(&dir).unwrap();

        let heading = "hello world\n";
        let contents = indoc::indoc! {r#"
            line 1
            line 2
            line 3
        "#};
        let expected = format!("{heading}{contents}");

        file.write_all(contents.as_bytes()).unwrap();
        file.flush().unwrap();
        // Close file
        drop(file);

        // Check that the contents are correct
        assert_eq!(fs::read(&path).unwrap(), contents.as_bytes());
        // Prepend the heading
        prepend_file(&path, false, heading.as_bytes()).unwrap();
        // Check that the new contents are correct
        assert_eq!(fs::read(&path).unwrap(), expected.as_bytes());

        // Assert that file is created when requested
        let nonexistent_path = dir.path().join("nonexistent");
        assert!(!nonexistent_path.exists());
        prepend_file(&nonexistent_path, false, heading.as_bytes()).unwrap();
        assert!(nonexistent_path.exists());

        // Assert error when file does not exist and must_exist=true
        let nonexistent_path = dir.path().join("nonexistent2");
        assert!(!nonexistent_path.exists());
        assert!(prepend_file(&nonexistent_path, true, heading.as_bytes()).is_err());
    }

    #[test]
    fn test_read_file_trim() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        let contents = indoc::indoc! {r#"

                 line 1    


        "#};
        fs::write(&path, contents).unwrap();
        assert_eq!(read_file_trim(&path).unwrap(), "line 1");
    }

    #[test]
    fn test_get_owner_uid() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        // Create a file, the owner should be the same as the current process
        let _ = create_file(&path).unwrap();

        // Yeah, this is silly, but it's the only way to get the current
        // process' UID without using an external crate
        assert_eq!(
            get_owner_uid(path).unwrap(),
            get_owner_uid("/proc/self").unwrap()
        );
    }

    #[test]
    fn test_get_owner_gid() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.txt");
        // Create a file, the owner should be the same as the current process
        let _ = create_file(&path).unwrap();

        // Yeah, this is silly, but it's the only way to get the current
        // process' GID without using an external crate
        assert_eq!(
            get_owner_gid(&path).unwrap(),
            get_owner_gid("/proc/self").unwrap()
        );
    }

    #[test]
    fn test_clean_directory() {
        let test_dir = tempdir().unwrap();

        // Create a bunch of files in the tempdir
        let files = (0..10)
            .map(|i| test_dir.path().join(format!("test_file_{i}")))
            .collect::<Vec<PathBuf>>();
        files.iter().for_each(|file| {
            create_file(file).unwrap();
        });

        // Create a bunch of directories in the tempdir
        let dirs = (0..10)
            .map(|i| test_dir.path().join(format!("test_dir_{i}")))
            .collect::<Vec<PathBuf>>();
        dirs.iter().for_each(|dir| {
            create_dirs(dir).unwrap();
            create_file(dir.join("test_file")).unwrap();
        });

        clean_directory(&test_dir).unwrap();

        // Assert that the directory still exists
        assert!(test_dir.path().exists(), "Directory should still exist");

        // Assert that all files and directories are gone
        files.iter().for_each(|file| {
            assert!(!file.exists(), "File should not exist: {}", file.display());
        });

        dirs.iter().for_each(|dir| {
            assert!(
                !dir.exists(),
                "Directory should not exist: {}",
                dir.display()
            );
        });
    }

    #[test]
    fn test_atomic_write_creates_new_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("new_file.conf");

        atomic_write_file(&path, "hello\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "hello\n");
    }

    #[test]
    fn test_atomic_write_replaces_existing_file() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("existing.conf");
        fs::write(&path, "old content\n").unwrap();

        atomic_write_file(&path, "new content\n").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new content\n");
    }

    #[test]
    fn test_atomic_write_preserves_permissions() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("perms.conf");
        fs::write(&path, "original\n").unwrap();
        fs::set_permissions(&path, Permissions::from_mode(0o640)).unwrap();

        atomic_write_file(&path, "updated\n").unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "Expected mode 0640, got {mode:04o}");
    }

    #[test]
    fn test_atomic_write_empty_content() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("empty.conf");

        atomic_write_file(&path, "").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "");
    }
}
