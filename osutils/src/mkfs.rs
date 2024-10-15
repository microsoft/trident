use std::path::Path;

use anyhow::{Context, Error};

use crate::{dependencies::Dependency, filesystems::MkfsFileSystemType};

pub fn run(device_path: &Path, filesystem: MkfsFileSystemType) -> Result<(), Error> {
    Dependency::Mkfs
        .cmd()
        .arg("--type")
        .arg(filesystem.name())
        .arg(device_path)
        .run_and_check()
        .context("Failed to execute mkfs")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
/// Helper function to create a filesystem that is smaller than the full device size
pub(super) fn run_blocks(
    device_path: &Path,
    filesystem: MkfsFileSystemType,
    blocks: u64,
) -> Result<(), Error> {
    Dependency::Mkfs
        .cmd()
        .arg("--type")
        .arg(filesystem.name())
        .arg(device_path)
        .arg(format!("{blocks}"))
        .run_and_check()
        .context("Failed to execute mkfs")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use sys_mount::{MountFlags, UnmountFlags};

    use pytest_gen::functional_test;

    use crate::{
        partition_types::DiscoverablePartitionType,
        repart::{RepartEmptyMode, RepartPartitionEntry, SystemdRepartInvoker},
        testutils::repart::{self, TEST_DISK_DEVICE_PATH},
        udevadm,
    };

    /// This function wipes the /dev/sdb device and ensures the /mnt
    /// directory exists.
    fn setup_test() {
        // Just zero-out the metadata so this is a fast operation.
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
        if !Path::new("/mnt").exists() {
            Dependency::Mkdir.cmd().arg("/mnt").run_and_check().unwrap();
        }
    }

    fn test_filesystem(filesystem: MkfsFileSystemType) {
        let block_device_path = Path::new(TEST_DISK_DEVICE_PATH);

        super::run(block_device_path, filesystem).unwrap();

        let mount_point = tempfile::tempdir()
            .context("Failed to create temporary mount point")
            .unwrap();
        let _mount = sys_mount::Mount::builder()
            .flags(MountFlags::RDONLY)
            .mount_autodrop(block_device_path, mount_point.path(), UnmountFlags::DETACH);
    }

    #[functional_test(feature = "helpers")]
    fn test_supported_filesystems() {
        test_filesystem(MkfsFileSystemType::Ext2);
        test_filesystem(MkfsFileSystemType::Ext3);
        test_filesystem(MkfsFileSystemType::Ext4);
        test_filesystem(MkfsFileSystemType::Vfat);
    }

    #[functional_test(feature = "helpers")]
    fn test_run_pass() {
        setup_test();

        // run() on a zeroed block device should format it with the
        // specified filesystem. It should be mountable and writable.
        super::run(Path::new(TEST_DISK_DEVICE_PATH), MkfsFileSystemType::Ext4).unwrap();
        assert_eq!(
            Dependency::Lsblk
                .cmd()
                .arg("-no")
                .arg("FSTYPE")
                .arg(TEST_DISK_DEVICE_PATH)
                .output_and_check()
                .unwrap(),
            "ext4\n"
        );
        Dependency::Mount
            .cmd()
            .arg(TEST_DISK_DEVICE_PATH)
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        Dependency::Touch
            .cmd()
            .arg("/mnt/test")
            .run_and_check()
            .unwrap();
        Dependency::Umount
            .cmd()
            .arg("/mnt")
            .run_and_check()
            .unwrap();

        // run() on a formatted block device with a different filesystem
        // should format it with the new filesystem and clear the device
        // contents.
        super::run(Path::new(TEST_DISK_DEVICE_PATH), MkfsFileSystemType::Ext3).unwrap();
        assert_eq!(
            Dependency::Lsblk
                .cmd()
                .arg("-no")
                .arg("FSTYPE")
                .arg(TEST_DISK_DEVICE_PATH)
                .output_and_check()
                .unwrap(),
            "ext3\n"
        );
        Dependency::Mount
            .cmd()
            .arg(TEST_DISK_DEVICE_PATH)
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        assert!(!Path::new("/mnt/test").exists());
        Dependency::Touch
            .cmd()
            .arg("/mnt/test")
            .run_and_check()
            .unwrap();
        Dependency::Umount
            .cmd()
            .arg("/mnt")
            .run_and_check()
            .unwrap();

        // run() on a formatted block device with the same filesystem
        // should not change the filesystem but should again clear the
        // device contents.
        super::run(Path::new(TEST_DISK_DEVICE_PATH), MkfsFileSystemType::Ext3).unwrap();
        assert_eq!(
            Dependency::Lsblk
                .cmd()
                .arg("-no")
                .arg("FSTYPE")
                .arg(TEST_DISK_DEVICE_PATH)
                .output_and_check()
                .unwrap(),
            "ext3\n"
        );
        Dependency::Mount
            .cmd()
            .arg(TEST_DISK_DEVICE_PATH)
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        assert!(!Path::new("/mnt/test").exists());
        Dependency::Touch
            .cmd()
            .arg("/mnt/test")
            .run_and_check()
            .unwrap();
        Dependency::Umount
            .cmd()
            .arg("/mnt")
            .run_and_check()
            .unwrap();
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_run_fail() {
        setup_test();

        // Create a file on the block device to ensure it's not empty.
        super::run(Path::new(TEST_DISK_DEVICE_PATH), MkfsFileSystemType::Ext4).unwrap();
        Dependency::Mount
            .cmd()
            .arg(TEST_DISK_DEVICE_PATH)
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        Dependency::Touch
            .cmd()
            .arg("/mnt/test")
            .run_and_check()
            .unwrap();
        Dependency::Umount
            .cmd()
            .arg("/mnt")
            .run_and_check()
            .unwrap();

        // run() using device '/dev/foo' that doesn't exist should also
        // fail and again not clear the device contents.
        assert!(super::run(Path::new("/dev/foo"), MkfsFileSystemType::Ext3).is_err());
        Dependency::Mount
            .cmd()
            .arg(TEST_DISK_DEVICE_PATH)
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        assert!(Path::new("/mnt/test").exists());
        Dependency::Umount
            .cmd()
            .arg("/mnt")
            .run_and_check()
            .unwrap();
    }

    #[functional_test(feature = "helpers")]
    fn test_create_ntfs() {
        setup_test();

        // NTFS requires partitions
        // Create parititions on block device
        let repart = SystemdRepartInvoker::new(TEST_DISK_DEVICE_PATH, RepartEmptyMode::Force)
            .with_partition_entries(vec![RepartPartitionEntry {
                id: "1".to_string(),
                partition_type: DiscoverablePartitionType::Root,
                label: Some("1".to_string()),
                size_max_bytes: Some(10 * 1048576),
                size_min_bytes: Some(10 * 1048576),
            }]);
        let partition1 = &repart.execute().unwrap()[0];

        // Wait for udev to process pending events, so that the system recognizes the new partition
        udevadm::settle().unwrap();

        // Create a NTFS filesystem on the partition.
        super::run(&partition1.node, MkfsFileSystemType::Ntfs).unwrap();
        Dependency::Mount
            .cmd()
            .arg(&partition1.node)
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        Dependency::Touch
            .cmd()
            .arg("/mnt/test")
            .run_and_check()
            .unwrap();
        Dependency::Umount
            .cmd()
            .arg("/mnt")
            .run_and_check()
            .unwrap();
    }
}
