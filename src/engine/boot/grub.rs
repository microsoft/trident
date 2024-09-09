use std::path::Path;

use anyhow::{bail, Context, Error};
use log::debug;
use uuid::Uuid;

use osutils::{
    blkid, exe::RunAndCheck, grub::GrubConfig, grub_mkconfig::GrubMkConfigScript, osrelease,
};
use trident_api::{
    config::{FileSystemType, SelinuxMode},
    constants::{
        BOOT_MOUNT_POINT_PATH, ESP_EFI_DIRECTORY, ESP_MOUNT_POINT_PATH, GRUB2_CONFIG_FILENAME,
        GRUB2_CONFIG_RELATIVE_PATH, ROOT_MOUNT_POINT_PATH,
    },
    status::HostStatus,
};

use crate::engine;

/// Updates the boot filesystem UUID on the search command inside the GRUB
/// config.
fn update_grub_config_esp(grub_config_path: &Path, boot_fs_uuid: &Uuid) -> Result<(), Error> {
    debug!(
        "Updating ESP GRUB config at path '{}' with UUID '{}'",
        grub_config_path.display(),
        boot_fs_uuid
    );
    let mut grub_config = GrubConfig::read(grub_config_path)?;
    grub_config.update_search(boot_fs_uuid)?;
    grub_config.write()
}

/// Updates the boot filesystem UUID on the search command and the rootdevice
/// inside the GRUB config.
fn update_grub_config_boot(
    grub_config_path: &Path,
    boot_fs_uuid: &Uuid,
    root_device_path: &Path,
    selinux_mode: Option<SelinuxMode>,
) -> Result<(), Error> {
    debug!(
        "Updating GRUB config at path '{}' with UUID '{}' and root device '{}'",
        grub_config_path.display(),
        boot_fs_uuid,
        root_device_path.display()
    );

    let mut grub_config = GrubConfig::read(grub_config_path)?;

    if let Some(mode) = selinux_mode {
        grub_config.set_selinux_mode(mode);
    }

    grub_config.update_search(boot_fs_uuid)?;

    grub_config.update_rootdevice(root_device_path)?;

    grub_config.write()
}

pub(super) fn update_configs(host_status: &HostStatus) -> Result<(), Error> {
    // Get the root block device path
    let root_device_path = engine::get_root_block_device_path(host_status)
        .context("Cannot find the root block device path")?;
    if root_device_path.as_os_str().is_empty() {
        bail!("Root device path is none");
    }

    // Find the block device which holds /boot
    let boot_mount_point = host_status
        .spec
        .storage
        .path_to_mount_point(Path::new(BOOT_MOUNT_POINT_PATH))
        .context("Failed to find mount point for boot block device")?;
    // get_filesystem_uuid expects a filesystem that uses UUIDs, so limiting to
    // ext4 for now
    // TODO: improve supported filesystems validation in API: https://dev.azure.com/mariner-org/ECF/_workitems/edit/6853
    if boot_mount_point.filesystem != FileSystemType::Ext4 {
        bail!(
            "Unsupported filesystem type for block device '{}': {}",
            boot_mount_point.target_id,
            boot_mount_point.filesystem
        );
    }

    let boot_block_device_id = &boot_mount_point.target_id;
    let boot_block_device_path = engine::get_block_device_path(host_status, boot_block_device_id)
        .context("Failed to find boot block device")?;

    let boot_uuid = blkid::get_filesystem_uuid(boot_block_device_path)?;
    let boot_grub_config_path = Path::new(ROOT_MOUNT_POINT_PATH).join(GRUB2_CONFIG_RELATIVE_PATH);
    //Get selinux mode from host status
    let selinux_mode = host_status.spec.os.selinux.mode;

    // Update GRUB config on the boot device (volume holding /boot)
    if osrelease::is_azl2().unwrap_or(false) {
        update_grub_config_boot(
            &boot_grub_config_path,
            &boot_uuid,
            &root_device_path,
            selinux_mode,
        )
        .context(format!(
            "Failed to update GRUB config at path '{}'",
            boot_grub_config_path.display()
        ))?;
    } else {
        // For AzL 3.0 we need to drop a grub-mkconfig script to manipulate the SELinux policy.
        if let Some(mode) = host_status.spec.os.selinux.mode {
            let mut script = GrubMkConfigScript::new("70_selinux_policy");
            for (key, value) in match mode {
                SelinuxMode::Disabled => vec![("selinux", "0")],
                SelinuxMode::Permissive => vec![("selinux", "1"), ("enforcing", "0")],
                SelinuxMode::Enforcing => vec![("selinux", "1"), ("enforcing", "1")],
            } {
                script.add_kv_param(key, value);
            }

            script.write().context("Failed to set SELinux policy")?;
        }

        std::process::Command::new("bash")
            .arg("-c")
            .arg(format!("grub2-mkconfig > /{GRUB2_CONFIG_RELATIVE_PATH}"))
            .run_and_check()
            .context(format!("Failed to update GRUB config at path '/{GRUB2_CONFIG_RELATIVE_PATH}' with mkconfig"))?;
    }

    // Update GRUB config on the ESP
    let bootentry_config_path = Path::new(ESP_MOUNT_POINT_PATH)
        .join(ESP_EFI_DIRECTORY)
        .join(
            super::get_update_esp_dir_name(host_status)
                .context("Failed to get update install ID")?,
        )
        .join(GRUB2_CONFIG_FILENAME);

    update_grub_config_esp(&bootentry_config_path, &boot_uuid).context(format!(
        "Failed to update GRUB config at path '{}'",
        bootentry_config_path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::fs;
    use uuid::Uuid;

    fn get_original_grub_content() -> (&'static str, &'static str) {
        // Define original GRUB config contents on target machine
        let original_content_grub_boot = indoc! {r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s

            load_env -f $bootprefix/mariner.cfg
            if [ -f  $bootprefix/systemd.cfg ]; then
                    load_env -f $bootprefix/systemd.cfg
            else
                    set systemd_cmdline=net.ifnames=0
            fi
            if [ -f $bootprefix/grub2/grubenv ]; then
                    load_env -f $bootprefix/grub2/grubenv
            fi

            set rootdevice=PARTUUID=29f8eed2-3c85-4da0-b32e-480e54379766

            menuentry "CBL-Mariner" {
                    linux $bootprefix/$mariner_linux   rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline console=tty0 console=ttyS0 $kernelopts
                    if [ -f $bootprefix/$mariner_initrd ]; then
                            initrd $bootprefix/$mariner_initrd
                    fi
            }"#};

        let original_content_grub_esp = indoc! {r#"search -n -u febfaaaa-fec4-4682-aee2-54f2d46b39ae -s

            # If '/boot' is a seperate partition, BootUUID will point directly to '/boot'.
            # In this case we should omit the '/boot' prefix from all paths.
            set bootprefix=/boot
            configfile $bootprefix/grub2/grub.cfg"#};

        (original_content_grub_boot, original_content_grub_esp)
    }

    fn get_expected_grub_content(
        random_uuid_grub_boot: String,
        root_path: Option<&Path>,
        random_uuid_grub_esp: String,
    ) -> (String, String) {
        // Define expected GRUB config contents after updating the rootfs UUID
        let (original_content_grub_boot, original_content_grub_esp) = get_original_grub_content();
        // Build the expected content with the new UUID
        let expected_content_grub_boot = original_content_grub_boot
            .replace(
                "PARTUUID=29f8eed2-3c85-4da0-b32e-480e54379766",
                root_path.unwrap().to_str().unwrap(),
            )
            .replace(
                "9e6a9d2c-b7fe-4359-ac45-18b505e29d8b",
                &random_uuid_grub_boot,
            );

        // Build the expected content with the new UUID
        let expected_content_grub_esp = original_content_grub_esp.replace(
            "febfaaaa-fec4-4682-aee2-54f2d46b39ae",
            &random_uuid_grub_esp,
        );

        (expected_content_grub_boot, expected_content_grub_esp)
    }

    #[test]
    fn test_update_grub_config_random_rootuuid() {
        let (original_content_grub_boot, original_content_grub_esp) = get_original_grub_content();

        // Create a temporary file and write the original content to it
        let temp_file_grub = tempfile::NamedTempFile::new().unwrap();
        let temp_file_path_grub = temp_file_grub.path();
        fs::write(temp_file_path_grub, original_content_grub_boot).unwrap();

        // Generate random FS UUID and root path for the partition
        let random_uuid_grub_boot = Uuid::new_v4();
        let random_uuid_grub_esp = Uuid::new_v4();
        let root_path = Path::new("/dev/sda1");
        update_grub_config_boot(temp_file_path_grub, &random_uuid_grub_boot, root_path, None)
            .unwrap();

        // Read back the content of the file
        let updated_content_grub = fs::read_to_string(temp_file_path_grub).unwrap();
        let (expected_content_grub_boot, expected_content_grub_esp) = get_expected_grub_content(
            random_uuid_grub_boot.to_string(),
            Some(root_path),
            random_uuid_grub_esp.clone().to_string(),
        );

        // Assert that the updated content matches the expected content
        assert_eq!(updated_content_grub, expected_content_grub_boot);

        let temp_file_grub2 = tempfile::NamedTempFile::new().unwrap();
        let temp_file_path_grub_esp = temp_file_grub2.path();
        fs::write(temp_file_path_grub_esp, original_content_grub_esp).unwrap();

        update_grub_config_esp(temp_file_path_grub_esp, &random_uuid_grub_esp).unwrap();

        // Read back the content of the file
        let updated_content_grub_esp = fs::read_to_string(temp_file_path_grub_esp).unwrap();

        // Assert that the updated content matches the expected content
        assert_eq!(updated_content_grub_esp, expected_content_grub_esp);
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
pub(crate) mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use std::path::PathBuf;

    use const_format::formatcp;
    use engine::storage::raid;
    use maplit::btreemap;
    use osutils::{
        filesystems::MkfsFileSystemType,
        lsblk::{self, BlockDevice, BlockDeviceType, PartitionTableType},
        mkfs,
        repart::{RepartEmptyMode, SystemdRepartInvoker},
        testutils::repart::{
            self, DISK_SIZE, PART1_SIZE, PART2_SIZE, PART3_SIZE, TEST_DISK_DEVICE_PATH,
        },
        udevadm,
    };
    use trident_api::{
        config::{
            self, AbUpdate, AbVolumePair, Disk, HostConfiguration, InternalMountPoint, Partition,
            PartitionType, RaidLevel, SoftwareRaidArray,
        },
        status::{ServicingState, ServicingType, Storage},
    };

    pub fn test_execute_and_resulting_layout(is_single_disk_raid: bool, unequal_partitions: bool) {
        let disk_bus_path = PathBuf::from(TEST_DISK_DEVICE_PATH);

        let mut partition_definition = repart::generate_partition_definition_esp_root_generic();
        let mut part3_size = PART2_SIZE;
        if is_single_disk_raid & !unequal_partitions {
            partition_definition =
                repart::generate_partition_definition_esp_root_raid_single_disk();
        } else if is_single_disk_raid & unequal_partitions {
            partition_definition =
                repart::generate_partition_definition_esp_root_raid_single_disk_unequal();
            part3_size = PART3_SIZE;
        }

        let repart = SystemdRepartInvoker::new(&disk_bus_path, RepartEmptyMode::Force)
            .with_partition_entries(partition_definition.clone());

        let partitions = repart.execute().unwrap();
        udevadm::settle().unwrap();

        if is_single_disk_raid {
            assert_eq!(partitions.len(), 4);
        } else {
            assert_eq!(partitions.len(), 3);
        }

        let part1 = &partitions[0];
        let part1_start = 1024 * 1024;
        assert_eq!(part1.start, part1_start);
        assert_eq!(part1.size, PART1_SIZE);

        let part2 = &partitions[1];
        let part2_start = part1_start + PART1_SIZE;
        assert_eq!(part2.start, part2_start);
        assert_eq!(part2.size, PART2_SIZE);

        if is_single_disk_raid {
            let part3 = &partitions[2];
            let part3_start = part2_start + PART2_SIZE;
            assert_eq!(part3.start, part3_start);
            assert_eq!(part3.size, part3_size);
            let part4 = &partitions[3];
            assert_eq!(part4.start, part3_start + part3_size);
            assert_eq!(
                part4.size,
                16 * 1024 * 1024 * 1024
                    - part1_start
                    - PART1_SIZE
                    - PART2_SIZE
                    - part3_size
                    - 20 * 1024 // 16 GiB disk - 1 MiB prefix - 50 MiB ESP - 20 KiB (rounding?)
            );

            let block_device = lsblk::run(&disk_bus_path).unwrap();
            let expected_block_device = BlockDevice {
                name: TEST_DISK_DEVICE_PATH.into(),
                ptuuid: block_device.ptuuid.clone(),
                size: DISK_SIZE,
                partition_table_type: Some(PartitionTableType::Gpt),
                readonly: false,
                blkdev_type: BlockDeviceType::Disk,
                children: vec![
                    BlockDevice {
                        name: formatcp!("{TEST_DISK_DEVICE_PATH}1").into(),
                        part_uuid: Some(part1.uuid.into()),
                        ptuuid: None,
                        size: part1.size,
                        parent_kernel_name: Some(PathBuf::from(TEST_DISK_DEVICE_PATH)),
                        partition_table_type: None,
                        readonly: false,
                        blkdev_type: BlockDeviceType::Partition,
                        ..Default::default()
                    },
                    BlockDevice {
                        name: formatcp!("{TEST_DISK_DEVICE_PATH}2").into(),
                        part_uuid: Some(part2.uuid.into()),
                        ptuuid: None,
                        size: part2.size,
                        parent_kernel_name: Some(PathBuf::from(TEST_DISK_DEVICE_PATH)),
                        partition_table_type: None,
                        readonly: false,
                        blkdev_type: BlockDeviceType::Partition,
                        ..Default::default()
                    },
                    BlockDevice {
                        name: formatcp!("{TEST_DISK_DEVICE_PATH}3").into(),
                        part_uuid: Some(part3.uuid.into()),
                        ptuuid: None,
                        size: part3.size,
                        parent_kernel_name: Some(PathBuf::from(TEST_DISK_DEVICE_PATH)),
                        partition_table_type: None,
                        readonly: false,
                        blkdev_type: BlockDeviceType::Partition,
                        ..Default::default()
                    },
                    BlockDevice {
                        name: formatcp!("{TEST_DISK_DEVICE_PATH}4").into(),
                        part_uuid: Some(part4.uuid.into()),
                        ptuuid: None,
                        size: part4.size,
                        parent_kernel_name: Some(PathBuf::from(TEST_DISK_DEVICE_PATH)),
                        partition_table_type: None,
                        readonly: false,
                        blkdev_type: BlockDeviceType::Partition,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };

            assert_eq!(expected_block_device, block_device);
        } else {
            let part3 = &partitions[2];
            assert_eq!(part3.start, part2_start + PART2_SIZE);
            assert_eq!(
                part3.size,
                16 * 1024 * 1024 * 1024 - part1_start - PART1_SIZE - PART2_SIZE - 20 * 1024 // 16 GiB disk - 1 MiB prefix - 50 MiB ESP - 20 KiB (rounding?)
            );

            udevadm::settle().unwrap();

            let block_device = lsblk::run(&disk_bus_path).unwrap();
            let expected_block_device = BlockDevice {
                name: TEST_DISK_DEVICE_PATH.into(),
                ptuuid: block_device.ptuuid.clone(),
                size: DISK_SIZE,
                partition_table_type: Some(PartitionTableType::Gpt),
                readonly: false,
                blkdev_type: BlockDeviceType::Disk,
                children: vec![
                    BlockDevice {
                        name: formatcp!("{TEST_DISK_DEVICE_PATH}1").into(),
                        part_uuid: Some(part1.uuid.into()),
                        ptuuid: None,
                        size: part1.size,
                        parent_kernel_name: Some(PathBuf::from(TEST_DISK_DEVICE_PATH)),
                        partition_table_type: None,
                        readonly: false,
                        blkdev_type: BlockDeviceType::Partition,
                        ..Default::default()
                    },
                    BlockDevice {
                        name: formatcp!("{TEST_DISK_DEVICE_PATH}2").into(),
                        part_uuid: Some(part2.uuid.into()),
                        ptuuid: None,
                        size: part2.size,
                        parent_kernel_name: Some(PathBuf::from(TEST_DISK_DEVICE_PATH)),
                        partition_table_type: None,
                        readonly: false,
                        blkdev_type: BlockDeviceType::Partition,
                        ..Default::default()
                    },
                    BlockDevice {
                        name: formatcp!("{TEST_DISK_DEVICE_PATH}3").into(),
                        part_uuid: Some(part3.uuid.into()),
                        ptuuid: None,
                        size: part3.size,
                        parent_kernel_name: Some(PathBuf::from(TEST_DISK_DEVICE_PATH)),
                        partition_table_type: None,
                        readonly: false,
                        blkdev_type: BlockDeviceType::Partition,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };

            assert_eq!(expected_block_device, block_device);
        }
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root on a raid array.
    fn test_update_grub_root_raided() {
        test_execute_and_resulting_layout(true, false);

        let mut host_status = HostStatus {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,

            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "foo".into(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "boot1".into(),
                                size: 2.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root1".into(),
                                size: 8.into(),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "root2".into(),
                                size: 8.into(),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_device_paths: btreemap! {
                        "foo".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                        "boot1".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                        "root1".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                        "root2".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Create a raid array
        let raid_array = SoftwareRaidArray {
            id: "raid_array".into(),
            name: "md0".into(),
            devices: vec!["root1".to_string(), "root2".to_string()],
            level: RaidLevel::Raid1,
        };
        raid::create_sw_raid_array(&mut host_status, &raid_array).unwrap();
        let root_device_path = raid_array.device_path();
        let result = test_update_grub_root_raided_internal(
            &mut host_status,
            &raid_array,
            root_device_path.as_path(),
        );
        // Unmount and stop the raid array
        raid::unmount_and_stop(&root_device_path).unwrap();
        result.unwrap();
    }

    fn test_update_grub_root_raided_internal(
        host_status: &mut HostStatus,
        raid_array: &SoftwareRaidArray,
        root_device_path: &Path,
    ) -> Result<(), Error> {
        host_status
            .spec
            .storage
            .internal_mount_points
            .push(InternalMountPoint {
                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                target_id: raid_array.id.clone(),
                filesystem: FileSystemType::Ext4,
                options: vec![],
            });

        host_status
            .storage
            .block_device_paths
            .insert(raid_array.id.clone(), root_device_path.to_owned());

        mkfs::run(root_device_path, MkfsFileSystemType::Ext4).unwrap();

        update_configs(host_status)
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root on a standalone partition.
    fn test_update_grub_root_standalone_partition() {
        test_execute_and_resulting_layout(false, false);
        let mut host_status = HostStatus {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,

            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "foo".into(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "boot".into(),
                                size: 2.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root".into(),
                                size: 8.into(),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![
                        InternalMountPoint {
                            path: PathBuf::from("/boot"),
                            target_id: "boot".to_owned(),
                            filesystem: FileSystemType::Vfat,
                            options: vec![],
                        },
                        InternalMountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            target_id: "root".to_string(),
                            filesystem: FileSystemType::Ext4,
                            options: vec![],
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_device_paths: btreemap! {
                        "foo".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                        "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                        "root".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let root_device_path = PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"));
        mkfs::run(&root_device_path, MkfsFileSystemType::Ext4).unwrap();

        // fail on unsupported filesystem
        assert_eq!(
            update_configs(&host_status).unwrap_err().to_string(),
            "Unsupported filesystem type for block device 'boot': vfat"
        );

        // original test
        host_status.spec.storage.internal_mount_points.remove(0);
        host_status
            .spec
            .storage
            .internal_mount_points
            .push(InternalMountPoint {
                path: PathBuf::from("/esp"),
                target_id: "boot".to_owned(),
                filesystem: FileSystemType::Vfat,
                options: vec![],
            });

        update_configs(&host_status).unwrap();
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root as an ab volume partition.
    fn test_update_grub_root_abvolume() {
        test_execute_and_resulting_layout(false, false);
        let host_status = HostStatus {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,

            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".into(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "efi".into(),
                                size: 1.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root-a".into(),
                                size: 9.into(),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "root-b".into(),
                                size: 9.into(),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![
                        InternalMountPoint {
                            path: PathBuf::from("/efi"),
                            target_id: "boot".to_owned(),
                            filesystem: FileSystemType::Vfat,
                            options: vec![],
                        },
                        InternalMountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            target_id: "root".to_string(),
                            filesystem: FileSystemType::Ext4,
                            options: vec![],
                        },
                    ],
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "root".to_string(),
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_device_paths: btreemap![
                    "os".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                    "efi".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                    "root-a".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                    "root-b".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let root_device_path = PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"));
        mkfs::run(&root_device_path, MkfsFileSystemType::Ext4).unwrap();
        update_configs(&host_status).unwrap();
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root on a standalone partition and setting root uuid empty so that the function bails on root_uuid being empty.
    fn test_update_grub_root_uuid_empty() {
        test_execute_and_resulting_layout(false, false);
        let host_status = HostStatus {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,

            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "foo".into(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "boot".into(),
                                size: 2.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root".into(),
                                size: 8.into(),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![InternalMountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        target_id: "root".to_string(),
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_device_paths: btreemap! {
                        "foo".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                        "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                        "root".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let result = update_configs(&host_status);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Failed to get UUID for path '/dev/sdb2', received ''"
        );
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root path empty so that the function bails on root path being None.
    fn test_update_grub_root_path_empty() {
        test_execute_and_resulting_layout(false, false);
        let host_status = HostStatus {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,

            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "foo".into(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "boot".into(),
                                size: 2.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root".into(),
                                size: 8.into(),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![InternalMountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        target_id: "root".to_string(),
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_device_paths: btreemap! {
                        "foo".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                        "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                        "root".into() => PathBuf::from(""),
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let result = update_configs(&host_status);

        assert_eq!(result.unwrap_err().to_string(), "Root device path is none");
    }
}
