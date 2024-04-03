use std::{path::Path, process::Command};

use anyhow::{Context, Error};

use crate::exe::RunAndCheck;

pub fn run(device_path: &Path) -> Result<(), Error> {
    Command::new("mkswap")
        .arg("--verbose")
        .arg(device_path)
        .run_and_check()
        .context("Failed to execute mkswap")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use crate::{
        filesystems::MkfsFileSystemType,
        mkfs,
        testutils::repart::{self, TEST_DISK_DEVICE_PATH},
    };

    /// This function wipes the /dev/sdb device and ensures the /mnt
    /// directory exists.
    fn setup_test() {
        // Just zero-out the metadata so this is a fast operation.
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
        if !Path::new("/mnt").exists() {
            Command::new("mkdir").arg("/mnt").run_and_check().unwrap();
        }
    }

    #[functional_test(feature = "helpers")]
    fn test_run_pass() {
        setup_test();

        // run() on a zeroed block device should prepare it as a swap volume. It
        // should be mountable and writable.
        super::run(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
        assert_eq!(
            Command::new("lsblk")
                .arg("-no")
                .arg("FSTYPE")
                .arg(TEST_DISK_DEVICE_PATH)
                .output_and_check()
                .unwrap(),
            "swap\n"
        );
        Command::new("swapon")
            .arg(TEST_DISK_DEVICE_PATH)
            .run_and_check()
            .unwrap();
        Command::new("swapoff")
            .arg(TEST_DISK_DEVICE_PATH)
            .run_and_check()
            .unwrap();

        assert_eq!(
            Command::new("swapoff")
                .arg(TEST_DISK_DEVICE_PATH)
                .run_and_check()
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Process output:\nstderr:\nswapoff: /dev/sdb: swapoff failed: Invalid argument\n\n"
        );

        // run() on a formatted block device with a different filesystem
        // should reformat it as a swap.
        mkfs::run(Path::new(TEST_DISK_DEVICE_PATH), MkfsFileSystemType::Ext3).unwrap();
        assert_eq!(
            Command::new("lsblk")
                .arg("-no")
                .arg("FSTYPE")
                .arg(TEST_DISK_DEVICE_PATH)
                .output_and_check()
                .unwrap(),
            "ext3\n"
        );
        super::run(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
        assert_eq!(
            Command::new("lsblk")
                .arg("-no")
                .arg("FSTYPE")
                .arg(TEST_DISK_DEVICE_PATH)
                .output_and_check()
                .unwrap(),
            "swap\n"
        );
        Command::new("swapon")
            .arg(TEST_DISK_DEVICE_PATH)
            .run_and_check()
            .unwrap();
        Command::new("swapoff")
            .arg(TEST_DISK_DEVICE_PATH)
            .run_and_check()
            .unwrap();
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_run_fail() {
        setup_test();

        // run() using device '/dev/foo' that doesn't exist should also
        // fail and again not clear the device contents.
        assert_eq!(
            super::run(Path::new("/dev/foo")).unwrap_err().to_string(),
            "Failed to execute mkswap"
        );

        // run() using a non-block device path should also fail.
        assert_eq!(
            super::run(Path::new("/etc/passwd"))
                .unwrap_err()
                .to_string(),
            "Failed to execute mkswap"
        );
    }
}
