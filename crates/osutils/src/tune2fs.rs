use anyhow::{Context, Error};
use std::path::Path;
use uuid::Uuid;

use crate::{dependencies::Dependency, e2fsck};

/// Assign filesystem UUID to the filesystem at block_device_path.
pub fn run(fs_uuid: &Uuid, block_device_path: &Path) -> Result<(), Error> {
    // Always need to first run e2fsck to check the file system on the block device
    e2fsck::fix(block_device_path)?;

    // Run tune2fs to assign a new randomized FS UUID to the updated volume
    Dependency::Tune2fs
        .cmd()
        .arg("-U")
        .arg(fs_uuid.to_string())
        .arg(block_device_path)
        .run_and_check()
        .context("Failed to execute tune2fs")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    use crate::{filesystems::MkfsFileSystemType, testutils::repart::TEST_DISK_DEVICE_PATH};

    /// Validates that run() correctly assigns a new UUID to the filesystem.
    #[functional_test(feature = "helpers")]
    fn test_tune2fs_run() {
        let block_device_path = Path::new(TEST_DISK_DEVICE_PATH);
        // Create a new ext4 filesystem on /dev/sdb
        crate::mkfs::run(block_device_path, MkfsFileSystemType::Ext4).unwrap();

        // Run tune2fs to assign a new UUID to the filesystem
        let new_uuid = Uuid::new_v4();
        run(&new_uuid, block_device_path).unwrap();

        // Validate that the UUID was assigned correctly by running blkid command to fetch block
        // devices
        let output = Dependency::Blkid
            .cmd()
            .arg("-o")
            .arg("value")
            .arg("-s")
            .arg("UUID")
            .arg(block_device_path)
            .output_and_check()
            .unwrap();

        let fs_uuid = Uuid::parse_str(output.trim()).unwrap();

        // Assert that the UUIDs match
        assert_eq!(fs_uuid, new_uuid);
    }

    /// Validates that run() correctly handles negative cases.
    #[functional_test(feature = "helpers", negative = true)]
    fn test_tune2fs_run_negative() {
        // Test case 1: Run tune2fs on a non-existent block device
        let block_device_path_nonexistent = Path::new("/dev/nonexistent");
        let new_uuid = Uuid::new_v4();

        let result_1 = run(&new_uuid, block_device_path_nonexistent);
        let error_message = result_1.unwrap_err().root_cause().to_string();

        // Check for key parts of the error message
        let expected_substrings = [
            "stdout:\nPossibly non-existent device?\n\n\nstderr:\ne2fsck ",
            "e2fsck: No such file or directory while trying to open /dev/nonexistent\n\n",
        ];

        for substring in &expected_substrings {
            assert!(
                error_message.contains(substring),
                "Error message does not contain expected substring '{substring}'"
            );
        }

        // Test case 2: Run tune2fs on a valid block device that does not have a filesystem.
        // Create a new loop device
        // Create a file to act as a loopback device
        Dependency::Dd
            .cmd()
            .arg("if=/dev/zero")
            .arg("of=/tmp/loopback.img")
            .arg("bs=1M")
            .arg("count=100")
            .output_and_check()
            .unwrap();
        // Set up a loop device
        let loop_device_output = Dependency::Losetup
            .cmd()
            .arg("--find")
            .arg("--show")
            .arg("/tmp/loopback.img")
            .output_and_check()
            .unwrap();
        // The output is already a string containing the loop device path
        let loop_device_path = loop_device_output.trim().to_string();
        // Zero out the metadata of the loop device
        Dependency::Wipefs
            .cmd()
            .arg("--all")
            .arg(&loop_device_path)
            .output_and_check()
            .unwrap();

        let result_2 = run(&Uuid::new_v4(), Path::new(&loop_device_path));
        let error_message_2 = result_2.unwrap_err().root_cause().to_string();

        println!("LOOK HERE:\n{error_message_2}");
        assert!(
                error_message_2.contains("stdout:\next2fs_open2: Bad magic number in super-block\n/usr/sbin/e2fsck: Superblock invalid, trying backup blocks...\n\nThe superblock could not be read or does not describe a valid ext2/ext3/ext4\nfilesystem.  If the device is valid and it really contains an ext2/ext3/ext4\nfilesystem (and not swap or ufs or something else), then the superblock\nis corrupt, and you might try running e2fsck with an alternate superblock:"),
                "Running tune2fs on a valid block device that does not have a filesystem did not return the expected error message"
            );
    }
}
