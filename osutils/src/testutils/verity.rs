use std::path::{Path, PathBuf};

use const_format::formatcp;

use crate::{
    files,
    grub::GrubConfig,
    mount::{self, MountGuard},
    mountpoint,
    repart::{RepartMode, SystemdRepartInvoker},
    testutils::repart::{CDROM_DEVICE_PATH, CDROM_MOUNT_PATH},
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

pub const VERITY_ROOT_DATA_IMAGE_PATH: &str = "data/verity_root.rawzst";
pub const VERITY_ROOT_HASH_IMAGE_PATH: &str = "data/verity_roothash.rawzst";
pub const VERITY_ROOT_BOOT_IMAGE_PATH: &str = "data/verity_boot.rawzst";

pub fn setup_verity_images() -> PathBuf {
    let cdrom_mount_path = Path::new(CDROM_MOUNT_PATH);
    if !cdrom_mount_path.exists() {
        files::create_dirs(cdrom_mount_path).unwrap();
    }
    if !mountpoint::check_is_mountpoint(cdrom_mount_path).unwrap() {
        mount::mount(CDROM_DEVICE_PATH, cdrom_mount_path, "iso9660", &[]).unwrap();
    }

    let verity_data_path = cdrom_mount_path.join(VERITY_ROOT_DATA_IMAGE_PATH);
    assert!(verity_data_path.exists());

    let verity_hash_path = cdrom_mount_path.join(VERITY_ROOT_HASH_IMAGE_PATH);
    assert!(verity_hash_path.exists());

    let boot_path = cdrom_mount_path.join(VERITY_ROOT_BOOT_IMAGE_PATH);
    assert!(boot_path.exists());

    cdrom_mount_path.to_owned()
}

pub fn setup_verity_volumes() -> String {
    let cdrom_mount_path = setup_verity_images();

    let block_device_path = Path::new(TEST_DISK_DEVICE_PATH);

    let boot_path = cdrom_mount_path.join(VERITY_ROOT_BOOT_IMAGE_PATH);
    image::stream_zstd(&boot_path, block_device_path).unwrap();

    let expected_root_hash = {
        let boot_mount_dir = tempfile::tempdir().unwrap();
        // Mount image to temp dir
        mount::mount(block_device_path, boot_mount_dir.path(), "ext4", &[]).unwrap();

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

    let verity_data_path = cdrom_mount_path.join(VERITY_ROOT_DATA_IMAGE_PATH);
    let verity_data_block_device_path = Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}3"));
    image::stream_zstd(&verity_data_path, verity_data_block_device_path).unwrap();
    let verity_hash_path = cdrom_mount_path.join(VERITY_ROOT_HASH_IMAGE_PATH);
    let verity_hash_block_device_path = Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}2"));
    image::stream_zstd(&verity_hash_path, verity_hash_block_device_path).unwrap();
    let verity_boot_path = cdrom_mount_path.join(VERITY_ROOT_BOOT_IMAGE_PATH);
    let verity_boot_block_device_path = Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}1"));
    image::stream_zstd(&verity_boot_path, verity_boot_block_device_path).unwrap();

    expected_root_hash
}
