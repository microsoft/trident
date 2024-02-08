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
    use pytest_gen::functional_test;

    use super::*;

    /// Validates that run() correctly checks the file system on the block device.
    #[functional_test(feature = "helpers")]
    fn test_e2fsck_run() {
        let block_device_path = Path::new("/dev/sdb");
        // Create a new ext4 filesystem on /dev/sdb
        crate::mkfs::run(block_device_path, "ext4").unwrap();

        // Run e2fsck to check the filesystem
        run(block_device_path).unwrap();
    }

    /// Validates that run() correctly handles negative cases.
    #[functional_test(feature = "helpers", negative = true)]
    fn test_e2fsck_run_negative() {
        // Test case 1: Run e2fsck on a non-existent file system
        let block_device_path_nonexistent = Path::new("/dev/nonexistent");
        let result = run(block_device_path_nonexistent);
        let error_message = result.unwrap_err().root_cause().to_string();

        assert_eq!(
            error_message,
            "Process output:\nstdout:\nPossibly non-existent device?\n\n\nstderr:\ne2fsck 1.46.5 (30-Dec-2021)\ne2fsck: No such file or directory while trying to open /dev/nonexistent\n\n",
            "Running tune2fs on a non-existent block device did not return the expected error message"
        );

        // Test case 2: Run e2fsck on a corrupted file system
        let block_device_path_corrupted = Path::new("/dev/sdb");
        // Create a new ext4 filesystem on /dev/sdc
        crate::mkfs::run(block_device_path_corrupted, "ext4").unwrap();
        // Corrupt the filesystem
        Command::new("dd")
            .arg("if=/dev/zero")
            .arg("of=/dev/sdb")
            .arg("bs=1M")
            .arg("count=1")
            .output()
            .unwrap();

        // Run e2fsck on the corrupted filesystem
        let result = run(block_device_path_corrupted);
        assert!(result.is_err());
    }
}
