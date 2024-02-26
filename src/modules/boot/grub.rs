use std::path::Path;

use anyhow::{bail, Context, Error};
use osutils::{blkid, grub::GrubConfig};
use trident_api::{
    constants::{
        BOOT_MOUNT_POINT_PATH, ESP_MOUNT_POINT_PATH, GRUB2_CONFIG_RELATIVE_PATH,
        ROOT_MOUNT_POINT_PATH,
    },
    status::HostStatus,
};
use uuid::Uuid;

use crate::modules;

/// Updates the boot filesystem UUID on the search command inside the GRUB
/// config.
fn update_grub_config_esp(grub_config_path: &Path, boot_fs_uuid: &Uuid) -> Result<(), Error> {
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
) -> Result<(), Error> {
    let mut grub_config = GrubConfig::read(grub_config_path)?;

    // TODO(6775): re-enable selinux
    grub_config.disable_selinux();

    grub_config.update_search(boot_fs_uuid)?;

    grub_config.update_rootdevice(root_device_path)?;

    grub_config.write()
}

pub(super) fn update_configs(host_status: &HostStatus) -> Result<(), Error> {
    // Get the root block device path
    let root_device_path = modules::get_root_block_device_path(host_status)
        .context("Cannot find the root block device path")?;
    if root_device_path.as_os_str().is_empty() {
        bail!("Root device path is none");
    }

    // Find the block device which holds /boot
    let boot_mount_point = host_status
        .storage
        .path_to_mount_point(Path::new(BOOT_MOUNT_POINT_PATH))
        .context("Failed to find mount point for boot block device")?;
    // get_filesystem_uuid expects a filesystem that uses UUIDs, so limiting to
    // ext4 for now
    // TODO: improve supported filesystems validation in API: https://dev.azure.com/mariner-org/ECF/_workitems/edit/6853
    if boot_mount_point.filesystem != "ext4" {
        bail!(
            "Unsupported filesystem type for block device '{}': {}",
            boot_mount_point.target_id,
            boot_mount_point.filesystem
        );
    }
    let boot_block_device_id = &boot_mount_point.target_id;
    let boot_block_device_info =
        modules::get_block_device(host_status, boot_block_device_id, false)
            .context("Failed to find boot block device")?;

    let boot_uuid = blkid::get_filesystem_uuid(boot_block_device_info.path.as_path())?;
    let boot_grub_config_path = Path::new(ROOT_MOUNT_POINT_PATH).join(GRUB2_CONFIG_RELATIVE_PATH);

    // Update GRUB config on the boot device (volume holding /boot)
    update_grub_config_boot(
        boot_grub_config_path.as_path(),
        &boot_uuid,
        &root_device_path,
    )
    .context(format!(
        "Failed to update GRUB config at path '{}'",
        boot_grub_config_path.display()
    ))?;

    let esp_grub_config_path = Path::new(ESP_MOUNT_POINT_PATH).join(GRUB2_CONFIG_RELATIVE_PATH);

    // Update GRUB config on the ESP device (also under /boot)
    update_grub_config_esp(esp_grub_config_path.as_path(), &boot_uuid).context(format!(
        "Failed to update GRUB config at path {}",
        esp_grub_config_path.display()
    ))?;

    Ok(())
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

        update_grub_config_boot(temp_file_path_grub, &random_uuid_grub_boot, root_path).unwrap();

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
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use maplit::btreemap;
    use osutils::{
        lsblk::{self, BlockDevice},
        mkfs,
        partition_types::DiscoverablePartitionType,
        repart::{RepartMode, RepartPartitionEntry, SystemdRepartInvoker},
        udevadm,
    };
    use std::path::PathBuf;
    use trident_api::{
        config::PartitionType,
        status::{
            AbUpdate, AbVolumePair, BlockDeviceContents, Disk, MountPoint, Partition,
            ReconcileState, Storage,
        },
    };
    use uuid::Uuid;

    const DISK_SIZE: u64 = 16 * 1024 * 1024 * 1024; // 16 GiB
    const PART1_SIZE: u64 = 50 * 1024 * 1024; // 50 MiB
    const DISK_BUS_PATH: &str = "/dev/sdb";
    const PART2_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB disk - 1 MiB prefix - 50 MiB ESP - 20 KiB (rounding?)

    fn generate_partition_definition() -> Vec<RepartPartitionEntry> {
        vec![
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::Esp,
                label: None,
                size_min_bytes: Some(PART1_SIZE),
                size_max_bytes: Some(PART1_SIZE),
            },
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::Root,
                label: None,
                size_min_bytes: Some(PART2_SIZE),
                size_max_bytes: Some(PART2_SIZE),
            },
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::LinuxGeneric,
                label: None,
                // When min==max==None, it's a grow partition
                size_min_bytes: None,
                size_max_bytes: None,
            },
        ]
    }

    pub fn test_execute_and_resulting_layout() {
        let partition_definition = generate_partition_definition();

        let disk_bus_path = PathBuf::from(DISK_BUS_PATH);

        let repart = SystemdRepartInvoker::new(&disk_bus_path, RepartMode::Force)
            .with_partition_entries(partition_definition.clone());

        let partitions = repart.execute().unwrap();

        assert_eq!(partitions.len(), 3);

        let part1 = &partitions[0];
        let part1_start = 1024 * 1024;
        assert_eq!(part1.start, part1_start);
        assert_eq!(part1.size, PART1_SIZE);

        let part2 = &partitions[1];
        let part2_start = part1_start + PART1_SIZE;
        assert_eq!(part2.start, part2_start);
        assert_eq!(part2.size, PART2_SIZE);

        let part3 = &partitions[2];
        assert_eq!(part3.start, part2_start + PART2_SIZE);
        assert_eq!(
            part3.size,
            16 * 1024 * 1024 * 1024 - part1_start - PART1_SIZE - PART2_SIZE - 20 * 1024 // 16 GiB disk - 1 MiB prefix - 50 MiB ESP - 20 KiB (rounding?)
        );

        udevadm::settle().unwrap();

        let expected_block_device_list = vec![BlockDevice {
            name: "/dev/sdb".into(),
            fstype: None,
            fssize: None,
            part_uuid: None,
            size: DISK_SIZE,
            parent_kernel_name: None,
            children: Some(vec![
                BlockDevice {
                    name: "/dev/sdb1".into(),
                    fstype: None,
                    fssize: None,
                    part_uuid: Some(part1.uuid),
                    size: part1.size,
                    parent_kernel_name: Some(PathBuf::from("/dev/sdb")),
                    children: None,
                },
                BlockDevice {
                    name: "/dev/sdb2".into(),
                    fstype: None,
                    fssize: None,
                    part_uuid: Some(part2.uuid),
                    size: part2.size,
                    parent_kernel_name: Some(PathBuf::from("/dev/sdb")),
                    children: None,
                },
                BlockDevice {
                    name: "/dev/sdb3".into(),
                    fstype: None,
                    fssize: None,
                    part_uuid: Some(part3.uuid),
                    size: part3.size,
                    parent_kernel_name: Some(PathBuf::from("/dev/sdb")),
                    children: None,
                },
            ]),
        }];

        let block_device_list = lsblk::run(&disk_bus_path).unwrap();
        assert_eq!(expected_block_device_list, block_device_list);
    }

    // Disabled as it breaks other FTs (depends on /dev/sda), task to fix: https://dev.azure.com/mariner-org/ECF/_workitems/edit/6828
    // #[functional_test(feature = "helpers")]
    // /// This functions tests update_grub by setting up root on a raid array.
    // fn test_update_grub_root_raided() {
    //     test_execute_and_resulting_layout();
    //     let mut host_status = HostStatus {
    //         storage: Storage {
    //             disks: btreemap! {
    //                 "foo".into() => Disk {
    //                     uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000000u128),
    //                     path: PathBuf::from("/dev/sda"),
    //                     capacity: 10,
    //                     contents: BlockDeviceContents::Initialized,
    //                     partitions: vec![
    //                         Partition {
    //                             uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000001u128),
    //                             path: PathBuf::from("/dev/sda1"),
    //                             id: "boot1".into(),
    //                             start: 1,
    //                             end: 3,
    //                             ty: PartitionType::Esp,
    //                             contents: BlockDeviceContents::Initialized,
    //                         },
    //                         Partition {
    //                             uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000002u128),
    //                             path: PathBuf::from("/dev/sda3"),
    //                             id: "root1".into(),
    //                             start: 4,
    //                             end: 10,
    //                             ty: PartitionType::Root,
    //                             contents: BlockDeviceContents::Initialized,
    //                         },
    //                     ],
    //                 },
    //                 "foo1".into() => Disk {
    //                     uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000003u128),
    //                     path: PathBuf::from("/dev/sdb"),
    //                     capacity: 10,
    //                     contents: BlockDeviceContents::Initialized,
    //                     partitions: vec![
    //                         Partition {
    //                             uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000004u128),
    //                             path: PathBuf::from("/dev/sdb1"),
    //                             id: "boot2".into(),
    //                             start: 1,
    //                             end: 3,
    //                             ty: PartitionType::Esp,
    //                             contents: BlockDeviceContents::Initialized,
    //                         },
    //                         Partition {
    //                             uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000005u128),
    //                             path: PathBuf::from("/dev/sdb2"),
    //                             id: "root2".into(),
    //                             start: 4,
    //                             end: 10,
    //                             ty: PartitionType::Root,
    //                             contents: BlockDeviceContents::Initialized,
    //                         },
    //                     ],
    //                 },

    //             },
    //             ..Default::default()
    //         },
    //         ..Default::default()
    //     };

    //     // Create a raid array
    //     let raid_array = SoftwareRaidArray {
    //         id: "raid_array".into(),
    //         name: "md0".into(),
    //         devices: vec!["root1".to_string(), "root2".to_string()],
    //         level: RaidLevel::Raid1,
    //         metadata_version: "1.2".into(),
    //     };
    //     raid::create_sw_raid_array(&mut host_status, &raid_array).unwrap();
    //     let root_device_path = PathBuf::from(format!("/dev/md/{}", &raid_array.name));
    //     let result = test_update_grub_root_raided_internal(
    //         &mut host_status,
    //         &raid_array,
    //         root_device_path.as_path(),
    //     );
    //     // Unmount and stop the raid array
    //     raid::unmount_and_stop(&root_device_path).unwrap();
    //     result.unwrap();
    // }

    // fn test_update_grub_root_raided_internal(
    //     host_status: &mut HostStatus,
    //     raid_array: &SoftwareRaidArray,
    //     root_device_path: &Path,
    // ) -> Result<(), Error> {
    //     // Make this as Root device
    //     host_status.storage.root_device_path = Some(root_device_path.to_owned());

    //     // Add mount points
    //     host_status.storage.mount_points = btreemap! {
    //         PathBuf::from("/boot") => MountPoint {
    //             target_id: "boot1".to_owned(),
    //             filesystem: "fat32".to_owned(),
    //             options: vec![],
    //         },
    //         PathBuf::from(ROOT_MOUNT_POINT_PATH) => MountPoint {
    //             target_id: raid_array.id.clone(),
    //             filesystem: "ext4".to_owned(),
    //             options: vec![],
    //         },
    //     };
    //     mkfs(root_device_path);

    //     update_grub_configs(host_status)
    // }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root on a standalone partition.
    fn test_update_grub_root_standalone_partition() {
        test_execute_and_resulting_layout();
        let mut host_status = HostStatus {
            storage: Storage {
                disks: btreemap! {
                    "foo".into() => Disk {
                        uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000003u128),
                        path: PathBuf::from("/dev/sdb"),
                        capacity: 10,
                        contents: BlockDeviceContents::Initialized,
                        partitions: vec![
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000004u128),
                                path: PathBuf::from("/dev/sdb1"),
                                id: "boot".into(),
                                start: 1,
                                end: 3,
                                ty: PartitionType::Esp,
                                contents: BlockDeviceContents::Initialized,
                            },
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000005u128),
                                path: PathBuf::from("/dev/sdb2"),
                                id: "root".into(),
                                start: 4,
                                end: 10,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Initialized,
                            },
                        ],
                    },

                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Add mount points
        host_status.storage.mount_points = btreemap! {
            PathBuf::from("/boot") => MountPoint {
                target_id: "boot".to_owned(),
                filesystem: "fat32".to_owned(),
                options: vec![],
            },
            PathBuf::from(ROOT_MOUNT_POINT_PATH) => MountPoint {
                target_id: "root".to_string(),
                filesystem: "ext4".to_string(),
                options: vec![],
            },
        };

        let root_device_path = PathBuf::from("/dev/sdb2");
        mkfs::run(&root_device_path, "ext4").unwrap();

        // fail on unsupported filesystem
        assert_eq!(
            update_configs(&host_status).unwrap_err().to_string(),
            "Unsupported filesystem type for block device 'boot': fat32"
        );

        // original test
        host_status
            .storage
            .mount_points
            .remove(&PathBuf::from("/boot"));
        host_status.storage.mount_points.insert(
            PathBuf::from("/esp"),
            MountPoint {
                target_id: "boot".to_owned(),
                filesystem: "fat32".to_owned(),
                options: vec![],
            },
        );

        update_configs(&host_status).unwrap();
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root as an ab volume partition.
    fn test_update_grub_root_abvolume() {
        test_execute_and_resulting_layout();
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/sdb"),
                        uuid: Uuid::nil(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "efi".to_string(),
                                path: PathBuf::from("/dev/sdb1"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root-a".to_string(),
                                path: PathBuf::from("/dev/sdb2"),
                                contents: BlockDeviceContents::Unknown,
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                path: PathBuf::from("/dev/sdb3"),
                                contents: BlockDeviceContents::Unknown,
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                        ],
                    },
                },
                ab_update: Some(AbUpdate {
                    volume_pairs: btreemap! {
                        "root".to_string() => AbVolumePair {
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        },
                    },
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        // Add mount points
        host_status.storage.mount_points = btreemap! {
            PathBuf::from("/efi") => MountPoint {
                target_id: "boot".to_owned(),
                filesystem: "fat32".to_owned(),
                options: vec![],
            },
            PathBuf::from(ROOT_MOUNT_POINT_PATH) => MountPoint {
                target_id: "root".to_string(),
                filesystem: "ext4".to_string(),
                options: vec![],
            },
        };

        let root_device_path = PathBuf::from("/dev/sdb2");
        mkfs::run(&root_device_path, "ext4").unwrap();
        update_configs(&host_status).unwrap();
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root on a standalone partition and setting root uuid empty so that the function bails on root_uuid being empty.
    fn test_update_grub_root_uuid_empty() {
        test_execute_and_resulting_layout();
        let mut host_status = HostStatus {
            storage: Storage {
                disks: btreemap! {
                    "foo".into() => Disk {
                        uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000003u128),
                        path: PathBuf::from("/dev/sdb"),
                        capacity: 10,
                        contents: BlockDeviceContents::Initialized,
                        partitions: vec![
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000004u128),
                                path: PathBuf::from("/dev/sdb1"),
                                id: "boot".into(),
                                start: 1,
                                end: 3,
                                ty: PartitionType::Esp,
                                contents: BlockDeviceContents::Initialized,
                            },
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000005u128),
                                path: PathBuf::from("/dev/sdb2"),
                                id: "root".into(),
                                start: 4,
                                end: 10,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Initialized,
                            },
                        ],
                    },

                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Add root mount point
        host_status.storage.mount_points = btreemap! {
                   PathBuf::from(ROOT_MOUNT_POINT_PATH) => MountPoint {
                    target_id: "root".to_string(),
                    filesystem: "ext4".to_string(),
                    options: vec![],
                },
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
        test_execute_and_resulting_layout();
        let mut host_status = HostStatus {
            storage: Storage {
                disks: btreemap! {
                    "foo".into() => Disk {
                        uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000003u128),
                        path: PathBuf::from("/dev/sdb"),
                        capacity: 10,
                        contents: BlockDeviceContents::Initialized,
                        partitions: vec![
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000004u128),
                                path: PathBuf::from("/dev/sdb1"),
                                id: "boot".into(),
                                start: 1,
                                end: 3,
                                ty: PartitionType::Esp,
                                contents: BlockDeviceContents::Initialized,
                            },
                            Partition {
                                uuid: Uuid::from_u128(0x00000000_0000_0000_0000_000000000005u128),
                                path: PathBuf::from(""),
                                id: "root".into(),
                                start: 4,
                                end: 10,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Initialized,
                            },
                        ],
                    },

                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Add root mount point
        host_status.storage.mount_points = btreemap! {
                   PathBuf::from(ROOT_MOUNT_POINT_PATH) => MountPoint {
                    target_id: "root".to_string(),
                    filesystem: "ext4".to_string(),
                    options: vec![],
                },
        };

        let result = update_configs(&host_status);

        assert_eq!(result.unwrap_err().to_string(), "Root device path is none");
    }
}
