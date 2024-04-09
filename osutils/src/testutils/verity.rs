use std::path::Path;

use const_format::formatcp;

use crate::{
    filesystems::MountFileSystemType,
    grub::GrubConfig,
    mount::{self, MountGuard},
    repart::{RepartMode, SystemdRepartInvoker},
    udevadm, veritysetup,
};

use super::{
    image,
    repart::{self, TEST_DISK_DEVICE_PATH},
};

pub struct VerityGuard<'a> {
    pub device_name: &'a str,
}

impl<'a> Drop for VerityGuard<'a> {
    fn drop(&mut self) {
        veritysetup::close(self.device_name).unwrap();
    }
}

pub const VERITY_ROOT_DATA_IMAGE_PATH: &str = "/data/verity_root.rawzst";
pub const VERITY_ROOT_HASH_IMAGE_PATH: &str = "/data/verity_roothash.rawzst";
pub const VERITY_ROOT_BOOT_IMAGE_PATH: &str = "/data/verity_boot.rawzst";

pub fn check_verity_images() {
    assert!(Path::new(VERITY_ROOT_DATA_IMAGE_PATH).exists());
    assert!(Path::new(VERITY_ROOT_HASH_IMAGE_PATH).exists());
    assert!(Path::new(VERITY_ROOT_BOOT_IMAGE_PATH).exists());
}

pub fn setup_verity_volumes() -> String {
    check_verity_images();

    let block_device_path = Path::new(TEST_DISK_DEVICE_PATH);

    image::stream_zstd(Path::new(VERITY_ROOT_BOOT_IMAGE_PATH), block_device_path).unwrap();

    let expected_root_hash = {
        let boot_mount_dir = tempfile::tempdir().unwrap();
        // Mount image to temp dir
        mount::mount(
            block_device_path,
            boot_mount_dir.path(),
            MountFileSystemType::Ext4,
            &[],
        )
        .unwrap();

        // Create a mount guard that will automatically unmount when it goes out of scope
        let _mount_guard = MountGuard {
            mount_dir: boot_mount_dir.path(),
        };

        let mut grub_config =
            GrubConfig::read(boot_mount_dir.path().join("grub2/grub.cfg")).unwrap();
        grub_config
            .read_linux_command_line_argument("roothash")
            .unwrap()
    };

    let repart = SystemdRepartInvoker::new(block_device_path, RepartMode::Force)
        .with_partition_entries(repart::generate_partition_definition_boot_root_verity());

    repart.execute().unwrap();
    udevadm::settle().unwrap();

    let verity_data_path = Path::new(VERITY_ROOT_DATA_IMAGE_PATH);
    let verity_data_block_device_path = Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}3"));
    image::stream_zstd(verity_data_path, verity_data_block_device_path).unwrap();
    let verity_hash_path = Path::new(VERITY_ROOT_HASH_IMAGE_PATH);
    let verity_hash_block_device_path = Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}2"));
    image::stream_zstd(verity_hash_path, verity_hash_block_device_path).unwrap();
    let verity_boot_path = Path::new(VERITY_ROOT_BOOT_IMAGE_PATH);
    let verity_boot_block_device_path = Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}1"));
    image::stream_zstd(verity_boot_path, verity_boot_block_device_path).unwrap();

    expected_root_hash
}
