use std::{path::Path, process::Command};

use anyhow::{Context, Error};
use log::error;

use crate::exe::RunAndCheck;
use crate::lsof;

/// Mounts file in file_path to a dir mount_dir.
pub fn mount(file_path: impl AsRef<Path>, mount_dir: impl AsRef<Path>) -> Result<(), Error> {
    // Mount the image
    Command::new("mount")
        // -o loop is required to mount a file instead of block device
        .arg("-o")
        .arg("loop")
        .arg(file_path.as_ref())
        .arg(mount_dir.as_ref())
        .run_and_check()
        .context(format!(
            "Failed to mount file {} to directory {}",
            file_path.as_ref().display(),
            mount_dir.as_ref().display(),
        ))?;

    Ok(())
}

/// Unmounts given directory mount_dir.
pub fn umount(mount_dir: &Path) -> Result<(), Error> {
    // Try to unmount the directory
    if let Err(e) = Command::new("umount").arg(mount_dir).run_and_check() {
        // If umount returns an error, do best effort to log open files while ignoring failures,
        // such as missing external dependency
        let opened_process_files = lsof::run(mount_dir);

        if let Ok(opened_process_files) = opened_process_files {
            error!("Open files: {:?}", opened_process_files);
        }

        // Propagate the original unmount error
        return Err(e.context(format!(
            "Failed to unmount directory {}",
            mount_dir.display()
        )));
    }

    Ok(())
}

#[cfg(feature = "functional-tests")]
mod functional_tests {
    #[cfg(test)]
    use super::*;
    #[cfg(test)]
    use std::{
        fs,
        path::{Path, PathBuf},
    };
    #[cfg(test)]
    use tempfile::NamedTempFile;
    #[cfg(test)]
    use tempfile::TempDir;

    use pytest_gen::pytest;

    #[pytest(feature = "helpers")]
    fn test_mount_and_umount() {
        // CDROM device to be mounted
        let device = Path::new("/dev/sr0");
        // Mount point
        let mount_point = Path::new("/mnt/cdrom");

        // Test mount_file function
        mount(device, mount_point).unwrap();

        // Fetch the name of loop device that was mounted at mount point
        let loop_device = find_loop_device(device).unwrap();
        // Validate that the device has been successfully mounted
        assert!(
            is_device_mounted_at(Path::new(&loop_device), mount_point),
            "Device not mounted at the expected mount point"
        );

        // Test unmount_dir function
        umount(mount_point).unwrap();

        // Validate that the device has been successfully unmounted
        assert!(
            !is_device_mounted_at(Path::new(&loop_device), mount_point),
            "Device not unmounted"
        );
    }

    /// Checks if a device is mounted at a given mount point
    #[cfg(test)]
    fn is_device_mounted_at(device: &Path, mount_point: &Path) -> bool {
        let mounts = fs::read_to_string("/proc/mounts").expect("Failed to read /proc/mounts");
        for line in mounts.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2
                && parts[0] == device.to_string_lossy()
                && parts[1] == mount_point.to_string_lossy()
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

    #[pytest(feature = "helpers", negative = true)]
    fn test_mount_failure() {
        // Test case 1: Create a valid temporary directory but use an invalid file path
        let temp_mount_dir = TempDir::new().unwrap();
        let invalid_file_path = PathBuf::from("/path/to/non/existent/file");

        // Attempt to mount a non-existent file and assert that it fails
        let mount_result_1 = mount(invalid_file_path, temp_mount_dir.path());
        assert_eq!(
            mount_result_1.unwrap_err().root_cause().to_string(),
            format!(
                "Process output:\nstderr:\nmount: {}: failed to setup loop device for /path/to/non/existent/file.\n\n",
                temp_mount_dir.path().display()
            ),
            "Unexpected error message for non-existent file"
        );

        // Test case 2: Create a valid temporary file but use an invalid directory path
        let temp_file = NamedTempFile::new().unwrap();
        let invalid_mount_dir = PathBuf::from("/path/to/non/existent/directory");

        // Attempt to mount a file to a non-existent directory and assert that it fails
        let mount_result_2 = mount(temp_file.path(), invalid_mount_dir);
        assert_eq!(
            mount_result_2.unwrap_err().root_cause().to_string(),
            "Process output:\nstderr:\nmount: /path/to/non/existent/directory: mount point does not exist.\n\n",
            "Mounting a file to a non-existent directory should fail"
        );
    }

    #[pytest(feature = "helpers", negative = true)]
    fn test_umount_failure() {
        // Create a valid temporary directory
        let temp_mount_dir = TempDir::new().unwrap();

        // Test case 1: Attempt to unmount an existing directory that isn't mounted and assert that
        // it fails
        let umount_result_1 = umount(temp_mount_dir.path());
        assert_eq!(
            umount_result_1.unwrap_err().root_cause().to_string(),
            format!(
                "Process output:\nstderr:\numount: {}: not mounted.\n\n",
                temp_mount_dir.path().display()
            ),
            "Unmounting a non-existent directory should fail"
        );

        // Test case 2: Attempt to unmount a directory that does not exist
        let invalid_mount_dir = PathBuf::from("/path/to/non/existent/directory");
        let umount_result_2 = umount(&invalid_mount_dir);
        assert_eq!(
            umount_result_2.unwrap_err().root_cause().to_string(),
            "Process output:\nstderr:\numount: /path/to/non/existent/directory: no mount point specified.\n\n",
            "Unmounting a non-existent directory should fail"
        );
    }
}
