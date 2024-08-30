use anyhow::{Context, Error};
use std::{path::Path, process::Command};

use crate::exe::RunAndCheck;

/// Runs e2fsck on the file system on the block device.
pub fn run(block_device_path: &Path) -> Result<(), Error> {
    // Run e2fsck to check the file system on the block device
    Command::new("e2fsck")
        .arg("-f")
        .arg("-y")
        .arg(block_device_path)
        .run_and_check()
        .context("Failed to execute e2fsck")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use crate::{
        filesystems::MkfsFileSystemType,
        testutils::repart::{self, TEST_DISK_DEVICE_PATH},
    };

    /// Validates that run() correctly checks the file system on the block device.
    #[functional_test(feature = "helpers")]
    fn test_e2fsck_run() {
        let block_device_path = Path::new(TEST_DISK_DEVICE_PATH);
        // Create a new ext4 filesystem on /dev/sdb
        crate::mkfs::run(block_device_path, MkfsFileSystemType::Ext4).unwrap();

        // Run e2fsck to check the filesystem
        run(block_device_path).unwrap();
    }

    /// Validates that run() correctly handles negative cases.
    #[functional_test(feature = "helpers", negative = true)]
    fn test_e2fsck_run_negative() {
        // Test case 1: Run e2fsck on a non-existent file system
        let block_device_path_nonexistent = Path::new("/dev/nonexistent");
        let error_string = run(block_device_path_nonexistent)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert!(
            error_string.contains(
                "e2fsck: No such file or directory while trying to open /dev/nonexistent"
            ),
            "Unexpected output: {error_string}"
        );

        // Test case 2: Run e2fsck on a corrupted file system
        let block_device_path_corrupted = Path::new(TEST_DISK_DEVICE_PATH);
        // Create a new ext4 filesystem
        crate::mkfs::run(block_device_path_corrupted, MkfsFileSystemType::Ext4).unwrap();
        // Corrupt the filesystem
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();

        // Run e2fsck on the corrupted filesystem
        let error_string = run(block_device_path_corrupted)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert!(
            error_string.contains("ext2fs_open2: Bad magic number in super-block"),
            "Unexpected output: {error_string}"
        );
    }
}
