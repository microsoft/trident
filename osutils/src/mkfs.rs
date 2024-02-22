use std::{path::Path, process::Command};

use anyhow::{Context, Error};

use crate::exe::RunAndCheck;

pub fn run(device_path: &Path, filesystem: &str) -> Result<(), Error> {
    Command::new("mkfs")
        .arg("--type")
        .arg(filesystem)
        .arg(device_path)
        .run_and_check()
        .context("Failed to execute mkfs")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
/// Helper function to create a filesystem that is smaller than the full device size
pub(super) fn run_blocks(device_path: &Path, filesystem: &str, blocks: u64) -> Result<(), Error> {
    Command::new("mkfs")
        .arg("--type")
        .arg(filesystem)
        .arg(device_path)
        .arg(format!("{blocks}"))
        .run_and_check()
        .context("Failed to execute mkfs")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use pytest_gen::functional_test;
    use sys_mount::{MountFlags, UnmountFlags};

    use super::*;

    /// This function wipes the /dev/sdb device and ensures the /mnt
    /// directory exists.
    fn setup_test() {
        // Just zero-out the metadata so this is a fast operation.
        Command::new("dd")
            .arg("if=/dev/zero")
            .arg("of=/dev/sdb")
            .arg("bs=1M")
            .arg("count=1")
            .run_and_check()
            .unwrap();
        if !Path::new("/mnt").exists() {
            Command::new("mkdir").arg("/mnt").run_and_check().unwrap();
        }
    }

    fn test_filesystem(filesystem: &str) {
        let block_device_path = Path::new("/dev/sdb");

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
        test_filesystem("ext2");
        test_filesystem("ext3");
        test_filesystem("ext4");
        test_filesystem("vfat");
    }

    #[functional_test(feature = "helpers")]
    fn test_run_pass() {
        setup_test();

        // run() on a zeroed block device should format it with the
        // specified filesystem. It should be mountable and writable.
        super::run(Path::new("/dev/sdb"), &String::from("ext4")).unwrap();
        assert_eq!(
            Command::new("lsblk")
                .arg("-no")
                .arg("FSTYPE")
                .arg("/dev/sdb")
                .output_and_check()
                .unwrap(),
            "ext4\n"
        );
        Command::new("mount")
            .arg("/dev/sdb")
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        Command::new("touch")
            .arg("/mnt/test")
            .run_and_check()
            .unwrap();
        Command::new("umount").arg("/mnt").run_and_check().unwrap();

        // run() on a formatted block device with a different filesystem
        // should format it with the new filesystem and clear the device
        // contents.
        super::run(Path::new("/dev/sdb"), &String::from("ext3")).unwrap();
        assert_eq!(
            Command::new("lsblk")
                .arg("-no")
                .arg("FSTYPE")
                .arg("/dev/sdb")
                .output_and_check()
                .unwrap(),
            "ext3\n"
        );
        Command::new("mount")
            .arg("/dev/sdb")
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        assert!(!Path::new("/mnt/test").exists());
        Command::new("touch")
            .arg("/mnt/test")
            .run_and_check()
            .unwrap();
        Command::new("umount").arg("/mnt").run_and_check().unwrap();

        // run() on a formatted block device with the same filesystem
        // should not change the filesystem but should again clear the
        // device contents.
        super::run(Path::new("/dev/sdb"), &String::from("ext3")).unwrap();
        assert_eq!(
            Command::new("lsblk")
                .arg("-no")
                .arg("FSTYPE")
                .arg("/dev/sdb")
                .output_and_check()
                .unwrap(),
            "ext3\n"
        );
        Command::new("mount")
            .arg("/dev/sdb")
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        assert!(!Path::new("/mnt/test").exists());
        Command::new("touch")
            .arg("/mnt/test")
            .run_and_check()
            .unwrap();
        Command::new("umount").arg("/mnt").run_and_check().unwrap();
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_run_fail() {
        setup_test();

        // Create a file on the block device to ensure it's not empty.
        super::run(Path::new("/dev/sdb"), &String::from("ext4")).unwrap();
        Command::new("mount")
            .arg("/dev/sdb")
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        Command::new("touch")
            .arg("/mnt/test")
            .run_and_check()
            .unwrap();
        Command::new("umount").arg("/mnt").run_and_check().unwrap();

        // run() using filesystem 'foo' that mkfs doesn't recognize should
        // fail and not clear the device contents.
        assert!(super::run(Path::new("/dev/sdb"), &String::from("foo")).is_err());
        Command::new("mount")
            .arg("/dev/sdb")
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        assert!(Path::new("/mnt/test").exists());
        Command::new("umount").arg("/mnt").run_and_check().unwrap();

        // run() using device '/dev/foo' that doesn't exist should also
        // fail and again not clear the device contents.
        assert!(super::run(Path::new("/dev/foo"), &String::from("ext3")).is_err());
        Command::new("mount")
            .arg("/dev/sdb")
            .arg("/mnt")
            .run_and_check()
            .unwrap();
        assert!(Path::new("/mnt/test").exists());
        Command::new("umount").arg("/mnt").run_and_check().unwrap();
    }
}
