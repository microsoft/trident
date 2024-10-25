use std::path::Path;

use anyhow::{Context, Error};

use crate::dependencies::Dependency;

/// Resize ext* filesystem on the specified block devices to fill the entire device.
pub fn run(block_device_path: &Path) -> Result<(), Error> {
    // Perform resize
    Dependency::Resize2fs
        .cmd()
        .arg(block_device_path)
        .run_and_check()
        .context("Failed to execute resize2fs")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use sys_mount::{MountFlags, UnmountFlags};

    use pytest_gen::functional_test;

    use crate::{
        filesystems::MkfsFileSystemType, lsblk, mkfs, testutils::repart::TEST_DISK_DEVICE_PATH,
    };

    fn create_and_resize_filesystem(
        block_device_path: &Path,
        filesystem: MkfsFileSystemType,
        before_blocks: &str,
        after_blocks: &str,
    ) {
        // Create a new filesystem on /dev/sdb
        crate::mkfs::run_blocks(block_device_path, filesystem, 10000).unwrap();

        {
            // Mount to get fs info
            let mount_point = tempfile::tempdir()
                .context("Failed to create temporary mount point")
                .unwrap();
            let _mount = sys_mount::Mount::builder()
                .flags(MountFlags::RDONLY)
                .mount_autodrop(block_device_path, mount_point.path(), UnmountFlags::DETACH);

            // Confirm initialize size
            let block_device = lsblk::get(block_device_path).unwrap();
            assert_eq!(block_device.fssize.unwrap().0.to_string(), before_blocks);
        }

        // Run resize2fs to resize the filesystem
        run(block_device_path).unwrap();

        {
            // Mount to get fs info
            let mount_point = tempfile::tempdir()
                .context("Failed to create temporary mount point")
                .unwrap();
            let _mount = sys_mount::Mount::builder()
                .flags(MountFlags::RDONLY)
                .mount_autodrop(block_device_path, mount_point.path(), UnmountFlags::DETACH);

            // Validate resize
            let block_device = lsblk::get(block_device_path).unwrap();
            assert_eq!(block_device.fssize.unwrap().0.to_string(), after_blocks);
        }
    }

    /// Validates that run() correctly resizes the filesystem.
    #[functional_test(feature = "helpers")]
    fn test_resize2fs_ext4_run() {
        create_and_resize_filesystem(
            Path::new(TEST_DISK_DEVICE_PATH),
            MkfsFileSystemType::Ext4,
            "8383488",
            "16518332416",
        );
    }

    /// Validates that run() correctly resizes the filesystem.
    #[functional_test(feature = "helpers")]
    fn test_resize2fs_ext3_run() {
        create_and_resize_filesystem(
            Path::new(TEST_DISK_DEVICE_PATH),
            MkfsFileSystemType::Ext3,
            "8463360",
            "16519315456",
        );
    }

    /// Validates that run() correctly resizes the filesystem.
    #[functional_test(feature = "helpers")]
    fn test_resize2fs_ext2_run() {
        create_and_resize_filesystem(
            Path::new(TEST_DISK_DEVICE_PATH),
            MkfsFileSystemType::Ext2,
            "9511936",
            "16520364032",
        );
    }

    /// Validates that run() correctly handles negative cases.
    #[functional_test(feature = "helpers", negative = true)]
    fn test_resize2fs_run_negative() {
        // Test case 1: Run resize2fs on a non-existent block device
        let block_device_path_nonexistent = Path::new("/dev/nonexistent");

        let error_string = run(block_device_path_nonexistent)
            .unwrap_err()
            .root_cause()
            .to_string();
        assert!(
            error_string
                .contains("\nopen: No such file or directory while opening /dev/nonexistent"),
            "Unexpected output: {error_string}"
        );

        // Test case 2: Run resize2fs on a valid block device that does not have a filesystem.
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

        let error_string = run(Path::new(&loop_device_path))
            .unwrap_err()
            .root_cause()
            .to_string();
        assert!(
            error_string.contains(
                "stdout:\nCouldn't find valid filesystem superblock.\n\n\nstderr:\nresize2fs "
            ),
            "Unexpected output: {error_string}"
        );
        assert!(
            error_string.contains(
                "resize2fs: Bad magic number in super-block while trying to open /dev/loop"
            ),
            "Unexpected output: {error_string}"
        );

        // Fail on unsupported FS
        mkfs::run(Path::new(&loop_device_path), MkfsFileSystemType::Vfat).unwrap();
        let error_string = run(Path::new(&loop_device_path))
            .unwrap_err()
            .root_cause()
            .to_string();
        assert!(
            error_string.contains(
                "stdout:\nCouldn't find valid filesystem superblock.\n\n\nstderr:\nresize2fs "
            ),
            "Unexpected output: {error_string}"
        );
        assert!(
            error_string.contains(
                "resize2fs: Bad magic number in super-block while trying to open /dev/loop"
            ),
            "Unexpected output: {error_string}"
        );
    }
}
