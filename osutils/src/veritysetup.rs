use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Error};
use log::error;
use trident_api::constants::DEV_MAPPER_PATH;

use crate::{exe::RunAndCheck, lsblk};

pub fn open(
    data_device_path: impl AsRef<Path>,
    device_name: &str,
    hash_device_name: impl AsRef<Path>,
    root_hash: &str,
) -> Result<(), Error> {
    Command::new("veritysetup")
        .arg("open")
        .arg(data_device_path.as_ref())
        .arg(device_name)
        .arg(hash_device_name.as_ref())
        .arg(root_hash)
        .arg("--verbose")
        .run_and_check()
        .context(format!("Failed to open verity device {}", device_name))?;
    let dm_verity_root_path = Path::new(DEV_MAPPER_PATH).join(device_name);
    if !dm_verity_root_path.exists() {
        bail!(
            "Verity device {} does not exist",
            dm_verity_root_path.display()
        );
    }

    Ok(())
}

pub fn is_present() -> Result<(), Error> {
    Command::new("veritysetup")
        .arg("--version")
        .run_and_check()
        .context("Failed to check veritysetup presence")
}

#[derive(Debug, PartialEq)]
pub struct VeritySetupStatus {
    pub type_: String,
    pub status: String,
    pub hash_type: u8,
    pub data_block_size: u64,
    pub hash_block_size: u64,
    pub hash_name: String,
    pub salt: String,
    pub data_device_path: PathBuf,
    pub size: String,
    pub mode: String,
    pub hash_device_path: PathBuf,
    pub hash_offset: String,
    pub root_hash: String,
    pub flags: Option<String>,
}

fn parse_veritysetup_status_output(output: &str) -> Result<VeritySetupStatus, Error> {
    // Skip the first line, which has human readable status
    let lines = output.lines().skip(1);

    let key_values: HashMap<&str, &str> = lines
        .map(|line| {
            let mut parts = line.splitn(2, ':');
            let key = parts
                .next()
                .context(format!(
                    "Missing key in the output of veritysetup status on line '{line}'"
                ))
                .map(|k| k.trim());
            let value = parts
                .next()
                .context(format!(
                    "Missing value in the output of veritysetup status on line '{line}'"
                ))
                .map(|v| v.trim());

            key.and_then(|key| value.map(|value| (key, value)))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;

    let verity_setup_status = VeritySetupStatus {
        type_: key_values
            .get("type")
            .context("Missing 'type' in the output of veritysetup status")?
            .to_string(),
        status: key_values
            .get("status")
            .context("Missing 'status' in the output of veritysetup status")?
            .to_string(),
        hash_type: key_values
            .get("hash type")
            .context("Missing 'hash type' in the output of veritysetup status")?
            .parse()
            .context(format!(
                "Unable to parse 'hash type' value '{}'",
                key_values
                    .get("hash type")
                    .context("Missing 'hash type' in the output of veritysetup status")?
            ))?,
        data_block_size: key_values
            .get("data block")
            .context("Missing 'data block' in the output of veritysetup status")?
            .parse()
            .context(format!(
                "Unable to parse 'data block' value '{}'",
                key_values
                    .get("data block")
                    .context("Missing 'data block' in the output of veritysetup status")?
            ))?,
        hash_block_size: key_values
            .get("hash block")
            .context("Missing 'hash block' in the output of veritysetup status")?
            .parse()
            .context(format!(
                "Unable to parse 'hash block' value '{}'",
                key_values
                    .get("hash block")
                    .context("Missing 'hash block' in the output of veritysetup status")?
            ))?,
        hash_name: key_values
            .get("hash name")
            .context("Missing 'hash name' in the output of veritysetup status")?
            .to_string(),
        salt: key_values
            .get("salt")
            .context("Missing 'salt' in the output of veritysetup status")?
            .to_string(),
        data_device_path: PathBuf::from(
            key_values
                .get("data device")
                .context("Missing 'data device' in the output of veritysetup status")?,
        ),
        size: key_values
            .get("size")
            .context("Missing 'size' in the output of veritysetup status")?
            .to_string(),
        mode: key_values
            .get("mode")
            .context("Missing 'mode' in the output of veritysetup status")?
            .to_string(),
        hash_device_path: PathBuf::from(
            key_values
                .get("hash device")
                .context("Missing 'hash device' in the output of veritysetup status")?,
        ),
        hash_offset: key_values
            .get("hash offset")
            .context("Missing 'hash offset' in the output of veritysetup status")?
            .to_string(),
        root_hash: key_values
            .get("root hash")
            .context("Missing 'root hash' in the output of veritysetup status")?
            .to_string(),
        flags: key_values.get("flags").map(|s| s.to_string()),
    };

    Ok(verity_setup_status)
}

pub fn status(device_name: &str) -> Result<VeritySetupStatus, Error> {
    let output = Command::new("veritysetup")
        .arg("status")
        .arg(device_name)
        .output_and_check()
        .context(format!(
            "Failed to get status of verity device {device_name}",
        ))?;

    parse_veritysetup_status_output(output.as_str())
}

pub fn close(device_name: &str) -> Result<(), Error> {
    let res = Command::new("veritysetup")
        .arg("close")
        .arg(device_name)
        .arg("--verbose")
        .run_and_check()
        .context(format!("Failed to close verity device {}", device_name));

    if let Err(e) = res {
        // If close returns an error, do best effort to log what is holding the
        // block device
        let block_device = lsblk::run(Path::new(DEV_MAPPER_PATH).join(device_name));
        if let Ok(block_device) = block_device {
            error!(
                "Failed to close {}: active children: {:?}, active mount points: {:?}",
                device_name, block_device.children, block_device.mountpoints
            );
        }

        // Propagate the original unmount error
        return Err(e.context(format!("Failed to close verity device {}", device_name)));
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parse_veritysetup_status_output() {
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        let status = parse_veritysetup_status_output(output).unwrap();

        assert_eq!(status.type_, "VERITY");
        assert_eq!(status.status, "verified");
        assert_eq!(status.hash_type, 1);
        assert_eq!(status.data_block_size, 4096);
        assert_eq!(status.hash_block_size, 4096);
        assert_eq!(status.hash_name, "sha256");
        assert_eq!(
            status.salt,
            "c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62"
        );
        assert_eq!(status.data_device_path, Path::new("/dev/sda3"));
        assert_eq!(status.size, "7567360 sectors");
        assert_eq!(status.mode, "readonly");
        assert_eq!(status.hash_device_path, Path::new("/dev/sda4"));
        assert_eq!(status.hash_offset, "8 sectors");
        assert_eq!(
            status.root_hash,
            "180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a"
        );
        assert_eq!(status.flags, Some("panic_on_corruption".to_string()));

        // fail on missing value
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Missing value in the output of veritysetup status on line '  type'"
        );

        // fail on missing key (though this also showcases as a missing value)
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.

            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Missing value in the output of veritysetup status on line ''"
        );

        // fail non-integer hash type
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1.5
                  data block:  4096
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "invalid digit found in string"
        );

        // fail non-integer data block
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096.5
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "invalid digit found in string"
        );

        // fail non-integer hash block
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096
                  hash block:  4096.5
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "invalid digit found in string"
        );

        // fail on a missing attribute (data device)
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Missing 'data device' in the output of veritysetup status"
        );

        // fail on a missing attribute (hash device)
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Missing 'hash device' in the output of veritysetup status"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use const_format::formatcp;
    use pytest_gen::functional_test;

    use std::fs;

    use crate::{
        files,
        grub::GrubConfig,
        mount::{self, MountGuard},
        partition_types::DiscoverablePartitionType,
        repart::{RepartMode, RepartPartitionEntry, SystemdRepartInvoker},
        testutils::{
            image,
            repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH},
            verity::{
                self, VerityGuard, VERITY_ROOT_BOOT_IMAGE_PATH, VERITY_ROOT_DATA_IMAGE_PATH,
                VERITY_ROOT_HASH_IMAGE_PATH,
            },
        },
        udevadm,
    };

    #[functional_test(feature = "helpers")]
    fn test_open_and_close() {
        let cdrom_mount_path = verity::setup_verity_images();

        let block_device_path = Path::new(TEST_DISK_DEVICE_PATH);

        let boot_path = cdrom_mount_path.join(VERITY_ROOT_BOOT_IMAGE_PATH);
        image::stream_zstd(&boot_path, block_device_path).unwrap();

        let root_hash = {
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
            .with_partition_entries(vec![
                RepartPartitionEntry {
                    partition_type: DiscoverablePartitionType::Root,
                    label: None,
                    size_min_bytes: Some(1024 * 1024 * 1024),
                    size_max_bytes: None,
                },
                RepartPartitionEntry {
                    partition_type: DiscoverablePartitionType::RootVerity,
                    label: None,
                    // When min==max==None, it's a grow partition
                    size_min_bytes: None,
                    size_max_bytes: None,
                },
            ]);

        repart.execute().unwrap();
        udevadm::settle().unwrap();

        let verity_data_path = cdrom_mount_path.join(VERITY_ROOT_DATA_IMAGE_PATH);
        let verity_data_block_device_path = Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}1"));
        image::stream_zstd(&verity_data_path, verity_data_block_device_path).unwrap();
        let verity_hash_path = cdrom_mount_path.join(VERITY_ROOT_HASH_IMAGE_PATH);
        let verity_hash_block_device_path = Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}2"));
        image::stream_zstd(&verity_hash_path, verity_hash_block_device_path).unwrap();

        // bad hash
        assert_eq!(
            open(
                verity_data_block_device_path,
                "verity-test",
                verity_hash_block_device_path,
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nInvalid root hash string specified.\n\n"
        );

        let mut expected_status = VeritySetupStatus {
            type_: "VERITY".to_string(),
            status: "verified".to_string(),
            hash_type: 1,
            data_block_size: 4096,
            hash_block_size: 4096,
            hash_name: "sha256".to_string(),
            salt: "".to_string(), // salt is not deterministic
            data_device_path: verity_data_block_device_path.to_owned(),
            size: "".to_string(), // size is not deterministic
            mode: "readonly".to_string(),
            hash_device_path: verity_hash_block_device_path.to_owned(),
            hash_offset: "8 sectors".to_string(),
            root_hash: root_hash.clone(),
            flags: None,
        };

        {
            // good hash
            open(
                verity_data_block_device_path,
                "verity-test",
                verity_hash_block_device_path,
                root_hash.as_str(),
            )
            .unwrap();
            let _verityguard = VerityGuard {
                device_name: "verity-test",
            };

            let mut status = status("verity-test").unwrap();
            status.salt = "".to_string(); // salt is not deterministic
            status.size = "".to_string(); // size is not deterministic
            assert_eq!(status, expected_status);

            {
                let verity_mount_dir = tempfile::tempdir().unwrap();
                mount::mount(
                    Path::new(DEV_MAPPER_PATH).join("verity-test"),
                    verity_mount_dir.path(),
                    "ext4",
                    &["ro".into()],
                )
                .unwrap();
                // Create a mount guard that will automatically unmount when it goes out of scope
                let _mount_guard = MountGuard {
                    mount_dir: verity_mount_dir.path(),
                };

                assert!(verity_mount_dir.path().join("etc").exists());
                fs::read_to_string(verity_mount_dir.path().join("etc/hostname")).unwrap();
            }
        }

        // verify verity checks

        {
            let root_mount_dir = tempfile::tempdir().unwrap();
            // Mount image to temp dir
            mount::mount(
                verity_data_block_device_path,
                root_mount_dir.path(),
                "ext4",
                &[],
            )
            .unwrap();

            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: root_mount_dir.path(),
            };

            files::write_file(
                root_mount_dir.path().join("etc/hostname"),
                0o644,
                "verity-test\n".as_bytes(),
            )
            .unwrap();
        };

        {
            open(
                verity_data_block_device_path,
                "verity-test",
                verity_hash_block_device_path,
                root_hash.as_str(),
            )
            .unwrap();
            let _verityguard = VerityGuard {
                device_name: "verity-test",
            };

            let mut status = status("verity-test").unwrap();
            status.salt = "".to_string(); // salt is not deterministic
            status.size = "".to_string(); // size is not deterministic
            expected_status.status = "corrupted".to_string();
            assert_eq!(status, expected_status);

            {
                let verity_mount_dir = tempfile::tempdir().unwrap();
                assert_eq!(mount::mount(
                    Path::new(DEV_MAPPER_PATH).join("verity-test"),
                    verity_mount_dir.path(),
                    "ext4",
                    &["ro".into()],
                )
                .unwrap_err().root_cause().to_string(), format!("Process output:\nstderr:\nmount: {}: can't read superblock on /dev/mapper/verity-test.\n\n", verity_mount_dir.path().display()));
            }
        }
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_fail_close_on_missing_devices() {
        assert_eq!(
            close("non-existent-device").unwrap_err().to_string(),
            "Failed to close verity device non-existent-device"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_fail_on_missing_or_malformed_devices() {
        // hash device does not contain verity hash tree
        assert_eq!(
            open(
                Path::new("/dev/sda1"),
                "foobar",
                Path::new(OS_DISK_DEVICE_PATH),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nDevice /dev/sda is not a valid VERITY device.\n\n"
        );

        // hash device is not a block device
        assert_eq!(
            open(
                Path::new("/dev/sda1"),
                "foobar",
                Path::new("/etc/passwd"),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nDevice /etc/passwd is not a valid VERITY device.\n\n"
        );

        let cdrom_mount_path = verity::setup_verity_images();
        image::stream_zstd(
            &cdrom_mount_path.join(VERITY_ROOT_HASH_IMAGE_PATH),
            Path::new(TEST_DISK_DEVICE_PATH),
        )
        .unwrap();

        // data device is not a block device
        assert_eq!(
            open(
                Path::new("/etc/passwd"),
                "foobar",
                Path::new(TEST_DISK_DEVICE_PATH),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nInvalid root hash string specified.\n\n"
        );

        // data device does not exist
        assert_eq!(
            open(
                Path::new("/dev/does-not-exist"),
                "foobar",
                Path::new("/etc/passwd"),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -4 (wrong device or file specified).\n\n\nstderr:\nDevice /dev/does-not-exist does not exist or access denied.\n\n"
        );

        // hash device does not exist
        assert_eq!(
            open(
                Path::new("/dev/sda1"),
                "foobar",
                Path::new("/etc/does-not-exist"),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -4 (wrong device or file specified).\n\n\nstderr:\nDevice /etc/does-not-exist does not exist or access denied.\n\n"
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_is_present() {
        is_present().unwrap();
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_status_fail_on_missing_device() {
        assert_eq!(
            status("non-existent-device").unwrap_err().to_string(),
            "Failed to get status of verity device non-existent-device"
        );
    }
}
