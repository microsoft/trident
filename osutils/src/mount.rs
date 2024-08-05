use std::fs;
use std::{path::Path, process::Command};

use anyhow::{bail, Context, Error};
use log::{error, info};

use crate::{exe::RunAndCheck, files, filesystems::MountFileSystemType, lsof};

/// Mounts file or block device in path to a dir mount_dir.
pub fn mount(
    path: impl AsRef<Path>,
    mount_dir: impl AsRef<Path>,
    filesystem: MountFileSystemType,
    options: &[String],
) -> Result<(), Error> {
    let mut options = options.to_owned();
    let mut command = Command::new("mount");

    // Check if file_path is a regular file and not a block device
    if path.as_ref().is_file() {
        // Use -o loop for mounting files
        options.push("loop".into());
    }

    if !options.is_empty() {
        command.arg("-o").arg(options.join(","));
    }

    // Execute the mount command
    command
        .arg("-t")
        .arg(filesystem.name())
        .arg(path.as_ref())
        .arg(mount_dir.as_ref())
        .run_and_check()
        .context(format!(
            "Failed to mount {} to path {}",
            path.as_ref().display(),
            mount_dir.as_ref().display(),
        ))?;

    Ok(())
}

/// Create a recursive bind mount for mount_dir as an alias of path, including
/// all sub-mounts. The mount is private, confining mount/unmount events to this
/// point.
pub fn private_rbind_mount(
    path: impl AsRef<Path>,
    mount_dir: impl AsRef<Path>,
) -> Result<(), Error> {
    Command::new("mount")
        .arg("--rbind")
        .arg("--make-rprivate")
        .arg(path.as_ref())
        .arg(mount_dir.as_ref())
        .run_and_check()
        .context(format!(
            "Failed to mount {} as a bind mount for {}",
            path.as_ref().display(),
            mount_dir.as_ref().display(),
        ))
}

/// Recursively remounts a given directory as private.
pub fn remount_rprivate(mount_dir: impl AsRef<Path>) -> Result<(), Error> {
    Command::new("mount")
        .arg("--make-rprivate")
        .arg(mount_dir.as_ref())
        .run_and_check()
        .context(format!(
            "Failed to remount {} as private",
            mount_dir.as_ref().display(),
        ))
}

/// Unmounts given directory mount_dir.
pub fn umount(mount_dir: impl AsRef<Path>, recursive: bool) -> Result<(), Error> {
    let mut cmd = Command::new("umount");
    if recursive {
        cmd.arg("-R");
    }

    // Try to unmount the directory
    if let Err(e) = cmd.arg(mount_dir.as_ref()).run_and_check() {
        // If umount returns an error, do best effort to log open files while ignoring failures,
        // such as missing external dependency
        let opened_process_files = lsof::run(mount_dir.as_ref());

        if let Ok(opened_process_files) = opened_process_files {
            if !opened_process_files.is_empty() {
                error!("Open files: {:?}", opened_process_files);
            }
        }

        // Propagate the original unmount error
        return Err(e.context(format!(
            "Failed to unmount directory {}",
            mount_dir.as_ref().display()
        )));
    }

    Ok(())
}

// MountGuard is a helper struct that automatically unmounts a directory when it goes out of scope.
// It is used to ensure that the ESP image is unmounted even if the function returns early.
pub struct MountGuard<'a> {
    pub mount_dir: &'a Path,
}

impl<'a> Drop for MountGuard<'a> {
    fn drop(&mut self) {
        if let Err(e) = umount(self.mount_dir, false) {
            info!(
                "Failed to unmount directory {}: {}",
                self.mount_dir.display(),
                e
            );
        }
    }
}

/// Ensure that the target_path is a suitable path for a mount point
pub fn ensure_mount_directory(target_path: &Path) -> Result<(), Error> {
    if target_path.exists() {
        if !target_path.is_dir() {
            bail!("Mount path '{}' is not a directory", target_path.display());
        }
        if let Ok(entries) = fs::read_dir(target_path) {
            if entries.count() > 0 {
                bail!("Mount path '{}' is not empty", target_path.display());
            }
        }
    } else {
        files::create_dirs(target_path).context(format!(
            "Failed to create mount path '{}'",
            target_path.display()
        ))?;
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    use std::fs::File;

    use tempfile::TempDir;

    #[test]
    fn test_ensure_mount_directory() {
        let temp_mount_dir = TempDir::new().unwrap();

        // Test case 1: Ensure a directory that exists and is empty
        ensure_mount_directory(temp_mount_dir.path()).unwrap();

        // Test case 2: Ensure a directory that does not exist
        let temp_mount_point_dir = temp_mount_dir.path().join("temp_dir");
        ensure_mount_directory(&temp_mount_point_dir).unwrap();
        assert!(temp_mount_point_dir.exists());

        // Test case 3: Ensure a directory that exists and is not empty
        assert_eq!(
            ensure_mount_directory(temp_mount_dir.path())
                .unwrap_err()
                .to_string(),
            format!(
                "Mount path '{}' is not empty",
                temp_mount_dir.path().display()
            )
        );

        // Test case 4: Ensure a file path does not work
        let temp_mount_point_file = temp_mount_dir.path().join("temp_file");
        File::create(&temp_mount_point_file).unwrap();
        assert_eq!(
            ensure_mount_directory(&temp_mount_point_file)
                .unwrap_err()
                .to_string(),
            format!(
                "Mount path '{}' is not a directory",
                temp_mount_point_file.display()
            )
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use crate::mountpoint;

    use super::*;
    use std::{fs, path::Path};
    use tempfile::NamedTempFile;
    use tempfile::TempDir;

    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_mount_and_umount() {
        // CDROM device to be mounted
        let device = Path::new("/dev/sr0");
        // Mount point
        let mount_point = Path::new("/mnt/cdrom");

        if mountpoint::check_is_mountpoint(mount_point).unwrap() {
            umount(mount_point, false).unwrap();
        }

        // Create the mount point directory if it doesn't exist yet
        fs::create_dir_all(mount_point).unwrap();

        // Test mount_file function
        mount(device, mount_point, MountFileSystemType::Iso9660, &[]).unwrap();

        // If device is a file, fetch the name of loop device that was mounted at mount point;
        // otherwise, use the device path itself
        let loop_device = if device.is_file() {
            find_loop_device(device).unwrap()
        } else {
            device.to_string_lossy().to_string()
        };

        // Validate that the device has been successfully mounted
        assert!(
            is_device_mounted_at(&loop_device, mount_point),
            "Device not mounted at the expected mount point"
        );

        // Test unmount_dir function
        umount(mount_point, false).unwrap();

        // Validate that the device has been successfully unmounted
        assert!(
            !is_device_mounted_at(&loop_device, mount_point),
            "Device not unmounted"
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_recursive_unmount() {
        let tmp_mount = Path::new("/mnt/tmpfs");
        fs::create_dir_all(tmp_mount).unwrap();
        mount(
            "tmpfs",
            tmp_mount,
            MountFileSystemType::Tmpfs,
            &["size=1M".into()],
        )
        .unwrap();

        let cdrom_mount = tmp_mount.join("cdrom");
        fs::create_dir_all(&cdrom_mount).unwrap();
        mount(
            Path::new("/dev/sr0"),
            &cdrom_mount,
            MountFileSystemType::Auto,
            &[],
        )
        .unwrap();

        umount(tmp_mount, true).unwrap();
        assert!(!cdrom_mount.exists());
    }

    #[functional_test(feature = "helpers")]
    fn test_readonly_mount() {
        let tmp_mount = Path::new("/mnt/tmpfs");
        fs::create_dir_all(tmp_mount).unwrap();
        mount(
            "tmpfs",
            tmp_mount,
            MountFileSystemType::Tmpfs,
            &["size=1M".into(), "ro".into()],
        )
        .unwrap();

        let cdrom_mount = tmp_mount.join("cdrom");
        assert_eq!(
            fs::create_dir_all(cdrom_mount).unwrap_err().to_string(),
            "Read-only file system (os error 30)"
        );

        umount(tmp_mount, true).unwrap();
    }

    /// Checks if a device is mounted at a given mount point
    #[cfg(test)]
    fn is_device_mounted_at(device: impl AsRef<Path>, mount_point: impl AsRef<Path>) -> bool {
        let mounts = fs::read_to_string("/proc/mounts").expect("Failed to read /proc/mounts");
        for line in mounts.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2
                && parts[0] == device.as_ref().to_string_lossy()
                && parts[1] == mount_point.as_ref().to_string_lossy()
            {
                return true;
            }
        }
        false
    }

    /// Identifies the loop device associated with a given file
    #[cfg(test)]
    fn find_loop_device(file_path: &Path) -> Result<String, Error> {
        let output = Command::new("losetup")
            .arg("-j")
            .arg(file_path)
            .output()
            .context("Failed to execute losetup command")?;

        let output_str =
            String::from_utf8(output.stdout.clone()).context("Failed to parse losetup output")?;

        // Extract the loop device name from the losetup output
        output_str
            .lines()
            .next()
            .and_then(|line| line.split(':').next())
            .map(String::from)
            .ok_or_else(|| Error::msg("Failed to find loop device in losetup output"))
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_mount_failure() {
        // Test case 1: Create a valid temporary directory but use an invalid file path
        let temp_mount_dir = TempDir::new().unwrap();

        // Attempt to mount a non-existent file and assert that it fails
        let mount_result_1 = mount(
            "/path/to/non/existent/file",
            temp_mount_dir.path(),
            MountFileSystemType::Auto,
            &[],
        );
        assert_eq!(
            mount_result_1.unwrap_err().root_cause().to_string(),
            format!(
                "Process output:\nstderr:\nmount: {}: special device /path/to/non/existent/file does not exist.\n\n",
                temp_mount_dir.path().display()
            ),
            "Unexpected error message for non-existent file"
        );

        // Test case 2: Create a valid temporary file but use an invalid directory path
        let temp_file = NamedTempFile::new().unwrap();

        // Attempt to mount a file to a non-existent directory and assert that it fails
        let mount_result_2 = mount(
            temp_file.path(),
            "/path/to/non/existent/directory",
            MountFileSystemType::Auto,
            &[],
        );
        assert_eq!(
            mount_result_2.unwrap_err().root_cause().to_string(),
            "Process output:\nstderr:\nmount: /path/to/non/existent/directory: mount point does not exist.\n\n",
            "Mounting a file to a non-existent directory should fail"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_umount_failure() {
        // Create a valid temporary directory
        let temp_mount_dir = TempDir::new().unwrap();

        // Test case 1: Attempt to unmount an existing directory that isn't mounted and assert that
        // it fails
        let umount_result_1 = umount(temp_mount_dir.path(), false);
        assert_eq!(
            umount_result_1.unwrap_err().root_cause().to_string(),
            format!(
                "Process output:\nstderr:\numount: {}: not mounted.\n\n",
                temp_mount_dir.path().display()
            ),
            "Unmounting a non-existent directory should fail"
        );

        // Test case 2: Attempt to unmount a directory that does not exist
        let umount_result_2 = umount("/path/to/non/existent/directory", false);
        assert_eq!(
            umount_result_2.unwrap_err().root_cause().to_string(),
            "Process output:\nstderr:\numount: /path/to/non/existent/directory: no mount point specified.\n\n",
            "Unmounting a non-existent directory should fail"
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_private_rbind_mount() {
        let temp_source_dir = TempDir::new().unwrap();
        let temp_intermediate_dir = TempDir::new().unwrap();
        let temp_work_dir = TempDir::new().unwrap();

        fs::write(temp_source_dir.path().join("test_file1"), "test1").unwrap();

        private_rbind_mount(temp_source_dir.path(), temp_intermediate_dir.path()).unwrap();
        private_rbind_mount(temp_intermediate_dir.path(), temp_work_dir.path()).unwrap();

        // Check that files in source are available from work directory
        assert_eq!(
            fs::read_to_string(temp_work_dir.path().join("test_file1")).unwrap(),
            "test1"
        );

        // Check that changes in work directory are reflected in mounted directories
        fs::write(temp_work_dir.path().join("test_file2"), "test2").unwrap();
        assert_eq!(
            fs::read_to_string(temp_intermediate_dir.path().join("test_file2")).unwrap(),
            "test2"
        );
        assert_eq!(
            fs::read_to_string(temp_source_dir.path().join("test_file2")).unwrap(),
            "test2"
        );

        // Check that files are no longer present after unmounting
        fs::write(temp_source_dir.path().join("test_file3"), "test3").unwrap();
        umount(temp_work_dir.path(), false).unwrap();
        assert!(!temp_work_dir.path().join("test_file1").exists());
        assert!(!temp_work_dir.path().join("test_file2").exists());
        assert!(!temp_work_dir.path().join("test_file3").exists());

        // Check that files are still available in source directory
        assert_eq!(
            fs::read_to_string(temp_source_dir.path().join("test_file1")).unwrap(),
            "test1"
        );
        assert_eq!(
            fs::read_to_string(temp_source_dir.path().join("test_file2")).unwrap(),
            "test2"
        );
        assert_eq!(
            fs::read_to_string(temp_source_dir.path().join("test_file3")).unwrap(),
            "test3"
        );
    }
}
