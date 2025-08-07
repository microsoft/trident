use std::{fs, io::Write, path::Path};

use anyhow::{bail, Context, Error};
use log::{debug, info, trace};
use tempfile::NamedTempFile;
use uuid::Uuid;

use osutils::{
    blkid,
    grub::GrubConfig,
    grub_mkconfig::GrubMkConfigScript,
    osmodifier::{self, BootConfig, IdentifiedPartition, Overlay, Verity},
    osrelease::{AzureLinuxRelease, Distro, OsRelease},
};
use trident_api::{
    config::Selinux,
    constants::{
        BOOT_MOUNT_POINT_PATH, ESP_EFI_DIRECTORY, ESP_MOUNT_POINT_PATH, GRUB2_CONFIG_FILENAME,
        GRUB2_CONFIG_RELATIVE_PATH, ROOT_MOUNT_POINT_PATH, TRIDENT_OVERLAY_LOWER_RELATIVE_PATH,
        TRIDENT_OVERLAY_UPPER_RELATIVE_PATH, TRIDENT_OVERLAY_WORK_RELATIVE_PATH,
    },
};

use crate::engine::{constants::TRIDENT_OVERLAY_PATH, storage::verity, EngineContext};

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

pub(super) fn update_configs(ctx: &EngineContext, os_modifier_path: &Path) -> Result<(), Error> {
    // Get the root block device path
    let root_device_path = ctx
        .get_root_block_device_path()
        .context("Cannot find the root block device path")?;
    if root_device_path.as_os_str().is_empty() {
        bail!("Root device path is none");
    }

    // Find the block device which holds /boot
    let boot_filesystem = ctx
        .spec
        .storage
        .path_to_filesystem(BOOT_MOUNT_POINT_PATH)
        .context("Failed to find filesystem for boot block device")?;

    let boot_block_device_id = &boot_filesystem
        .device_id
        .clone()
        .context("Failed to get device_id for boot block device")?;
    let boot_block_device_path = ctx
        .get_block_device_path(boot_block_device_id)
        .context("Failed to find boot block device")?;

    let boot_uuid = blkid::get_filesystem_uuid(boot_block_device_path)?;
    let boot_grub_config_path = Path::new(ROOT_MOUNT_POINT_PATH).join(GRUB2_CONFIG_RELATIVE_PATH);

    // Update GRUB config on the boot device (volume holding /boot)
    match OsRelease::read()
        .context("Failed to read OS release")?
        .get_distro()
    {
        Distro::AzureLinux(AzureLinuxRelease::AzL3) => {
            update_grub_config_azl3(
                ctx,
                os_modifier_path,
                &root_device_path,
                &boot_grub_config_path,
            )?;
        }

        d => bail!("Unsupported distro for GRUB config update: {d:?}"),
    }

    // Update GRUB config on the ESP
    let bootentry_config_path = Path::new(ESP_MOUNT_POINT_PATH)
        .join(ESP_EFI_DIRECTORY)
        .join(super::get_update_esp_dir_name(ctx).context("Failed to get update install ID")?)
        .join(GRUB2_CONFIG_FILENAME);

    update_grub_config_esp(&bootentry_config_path, &boot_uuid).context(format!(
        "Failed to update GRUB config at path '{}'",
        bootentry_config_path.display()
    ))
}

/// Updates the GRUB config for Azure Linux 3.0 using OS modifier.
fn update_grub_config_azl3(
    ctx: &EngineContext,
    os_modifier_path: &Path,
    root_device_path: &Path,
    boot_grub_config_path: &Path,
) -> Result<(), Error> {
    // For azl 3.0, we need to disable cloud-init's network configuration when Trident is
    // configuring the network. This is done by setting the 'network-config' kernel parameter
    // to 'disabled'.
    if ctx.spec.os.netplan.is_some() {
        info!("Disabling default cloud-init network config");
        let mut disable_default_cloud_init_network = GrubMkConfigScript::new("prefer-netplan");
        disable_default_cloud_init_network.add_kv_param("network-config", "disabled");
        disable_default_cloud_init_network
            .write()
            .context("Failed to disable default cloud-init network config")?;
    }

    debug!("Updating GRUB config for Azure Linux 3.0 with OS modifier");

    // OS modifier will read values of verity, selinux, root device, and overlay from original GRUB config
    // stamp them into /etc/default/grub and regenerate the GRUB config using grub-mkconfig.
    // Log the contents of the GRUB config first.
    let grub_config = fs::read_to_string(boot_grub_config_path)?;
    trace!(
        "Contents of GRUB config at path '{}':\n{}",
        boot_grub_config_path.display(),
        grub_config
    );

    osmodifier::update_grub(os_modifier_path)?;

    let updated_grub_config = fs::read_to_string(boot_grub_config_path)?;
    trace!(
        "Contents of GRUB config at path '{}' updated with OS modifier:\n{}",
        boot_grub_config_path.display(),
        updated_grub_config
    );

    // If selinux is provided in engine context, overwrite selinux in GRUB config
    let selinux_config = ctx
        .spec
        .os
        .selinux
        .mode
        .map(|mode| Selinux { mode: Some(mode) });

    // If root verity is provided in engine context, overwrite it in GRUB config
    let root_device_id = ctx
        .spec
        .storage
        .path_to_filesystem(ROOT_MOUNT_POINT_PATH)
        .and_then(|m| m.device_id.clone())
        .context("Failed to find mount point for root block device")?;

    let verity = ctx
        .spec
        .storage
        .verity
        .iter()
        .find(|device| device.id == *root_device_id)
        .map(|verity_device| {
            let (verity_data_path, verity_hash_path) =
                verity::get_verity_device_paths(ctx, verity_device)
                    .context("Failed to get verity-related device paths")?;

            let verity_data_path_str = verity_data_path
                .to_str()
                .context("Failed to convert verity_data_path to string")?;

            let verity_hash_path_str = verity_hash_path
                .to_str()
                .context("Failed to convert verity_hash_path to string")?;

            Ok::<Verity, anyhow::Error>(Verity {
                id: verity_device.id.clone(),
                name: verity_device.name.to_string(),
                data_device: verity_data_path_str.to_string(),
                hash_device: verity_hash_path_str.to_string(),
                corruption_option: None,
            })
        })
        .transpose()?;

    // If overlay is provided in engine context, overwrite overlay in GRUB config
    let overlays = ctx
        .spec
        .storage
        .mount_points_by_path()
        .get(Path::new(TRIDENT_OVERLAY_PATH))
        .map(|overlay_mount_point| {
            overlay_mount_point
                .device_id
                .as_ref()
                .map(|device_id| {
                    let overlay_device_path = ctx
                        .get_block_device_path(device_id)
                        .context(format!("Failed to find overlay device {device_id}"))?;

                    let volume_value = overlay_device_path.to_str().context(format!(
                        "Failed to convert mount device path '{}' to string",
                        overlay_device_path.display()
                    ))?;

                    let partition = IdentifiedPartition {
                        id: volume_value.to_string(),
                    };

                    Ok::<Overlay, anyhow::Error>(Overlay {
                        lower_dir: TRIDENT_OVERLAY_LOWER_RELATIVE_PATH.into(),
                        upper_dir: TRIDENT_OVERLAY_UPPER_RELATIVE_PATH.into(),
                        work_dir: TRIDENT_OVERLAY_WORK_RELATIVE_PATH.into(),
                        partition,
                    })
                })
                .transpose()
        })
        .transpose()?
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

    let root_device_str = root_device_path
        .to_str()
        .context("Failed to convert root device path to string")?;

    let config: BootConfig = BootConfig {
        selinux: selinux_config,
        overlays,
        verity,
        root_device: Some(root_device_str.to_string()),
    };

    let boot_config_yaml = serde_yaml::to_string(&config).context("Failed to serialize to YAML")?;

    // Create a temporary file and write the config to it
    let mut tmpfile = NamedTempFile::new().context("Failed to create a temporary file")?;
    tmpfile
        .write_all(boot_config_yaml.as_bytes())
        .context(format!(
            "Failed to write boot config to temporary file at {:?}",
            tmpfile.path()
        ))?;
    tmpfile.flush().context(format!(
        "Failed to flush temporary file at {:?}",
        tmpfile.path()
    ))?;

    osmodifier::run(os_modifier_path, tmpfile.path()).with_context(|| {
        format!(
            "Failed to run OS modifier to update GRUB config with temporary config file at {:?}",
            tmpfile.path()
        )
    })?;

    debug!("Finished updating GRUB config for Azure Linux 3.0 with OS modifier");

    Ok(())
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
pub(crate) mod functional_test {
    use super::*;

    use std::path::PathBuf;

    use const_format::formatcp;
    use maplit::btreemap;

    use crate::{
        engine::{boot::get_update_esp_dir_name, storage::raid},
        OS_MODIFIER_BINARY_PATH,
    };

    use osutils::{
        block_devices,
        filesystems::MkfsFileSystemType,
        lsblk::{self, BlockDevice, BlockDeviceType, PartitionTableType},
        mdadm, mkfs,
        repart::{RepartEmptyMode, SystemdRepartInvoker},
        testutils::repart::{
            self, DISK_SIZE, PART1_SIZE, PART2_SIZE, PART3_SIZE, TEST_DISK_DEVICE_PATH,
        },
        udevadm,
    };
    use pytest_gen::functional_test;
    use trident_api::{
        config::{
            self, AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
            MountOptions, MountPoint, Partition, PartitionType, RaidLevel, SoftwareRaidArray,
        },
        status::ServicingType,
    };

    struct DropFile(PathBuf);
    impl Drop for DropFile {
        fn drop(&mut self) {
            if let Err(e) = fs::remove_file(&self.0) {
                eprintln!("Failed to remove file '{}': {}", self.0.display(), e);
            }
        }
    }

    fn setup_mock_grub_configs(ctx: &EngineContext) -> (DropFile, DropFile) {
        let grub_esp = include_str!("test_files/grub_esp.cfg");
        let grub_boot = include_str!("test_files/grub_boot.cfg");

        let grub_esp_path = Path::new(ESP_MOUNT_POINT_PATH)
            .join(ESP_EFI_DIRECTORY)
            .join(get_update_esp_dir_name(ctx).expect("Failed to get update esp dir name"))
            .join(GRUB2_CONFIG_FILENAME);
        let grub_boot_path = Path::new(ROOT_MOUNT_POINT_PATH).join(GRUB2_CONFIG_RELATIVE_PATH);

        fs::create_dir_all(grub_esp_path.parent().unwrap())
            .expect("Failed to create directory for grub esp config");
        fs::create_dir_all(grub_boot_path.parent().unwrap())
            .expect("Failed to create directory for grub boot config");

        fs::write(&grub_esp_path, grub_esp).expect("Failed to write grub esp config");
        let drop_file_esp = DropFile(grub_esp_path.clone());
        fs::write(&grub_boot_path, grub_boot).expect("Failed to write grub boot config");
        let drop_file_boot = DropFile(grub_boot_path.clone());

        (drop_file_esp, drop_file_boot)
    }

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

            let block_device = lsblk::get(&disk_bus_path).unwrap();
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
                        partn: Some(1),
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
                        partn: Some(2),
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
                        partn: Some(3),
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
                        partn: Some(4),
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

            let block_device = lsblk::get(&disk_bus_path).unwrap();
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
                        partn: Some(1),
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
                        partn: Some(2),
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
                        partn: Some(3),
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
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .try_init()
            .ok();
        test_execute_and_resulting_layout(true, false);

        let mut ctx = EngineContext {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,

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
            partition_paths: btreemap! {
                "foo".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot1".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root1".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                "root2".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
            },
            is_uki: Some(false),
            ..Default::default()
        };

        // Create a raid array
        let raid_array = SoftwareRaidArray {
            id: "raid_array".into(),
            name: "md0".into(),
            devices: vec!["root1".to_string(), "root2".to_string()],
            level: RaidLevel::Raid1,
        };
        raid::create_sw_raid_array(&ctx, &raid_array).unwrap();
        let root_device_path = raid_array.device_path();
        let result = test_update_grub_root_raided_internal(
            &mut ctx,
            &raid_array,
            root_device_path.as_path(),
        );
        // Unmount and stop the raid array
        block_devices::unmount_all_mount_points(&root_device_path).unwrap();
        mdadm::stop(&root_device_path).unwrap();

        repart::clear_disk(TEST_DISK_DEVICE_PATH).unwrap();
        result.unwrap();
    }

    fn test_update_grub_root_raided_internal(
        ctx: &mut EngineContext,
        raid_array: &SoftwareRaidArray,
        root_device_path: &Path,
    ) -> Result<(), Error> {
        ctx.spec.storage.filesystems.push(FileSystem {
            mount_point: Some(MountPoint {
                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                options: MountOptions::empty(),
            }),
            device_id: Some(raid_array.id.clone()),
            source: FileSystemSource::Image,
        });

        ctx.partition_paths
            .insert(raid_array.id.clone(), root_device_path.to_owned());

        mkfs::run(root_device_path, MkfsFileSystemType::Ext4).unwrap();

        let _a = setup_mock_grub_configs(ctx);

        update_configs(ctx, Path::new(OS_MODIFIER_BINARY_PATH))
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root on a standalone partition.
    fn test_update_grub_root_standalone_partition() {
        test_execute_and_resulting_layout(false, false);
        let ctx = EngineContext {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,

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
                    filesystems: vec![
                        FileSystem {
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/esp"),
                                options: MountOptions::empty(),
                            }),
                            device_id: Some("boot".to_owned()),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            mount_point: Some(MountPoint {
                                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                                options: MountOptions::empty(),
                            }),
                            device_id: Some("root".to_owned()),
                            source: FileSystemSource::Image,
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "foo".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
            },
            ..Default::default()
        };

        let root_device_path = PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"));
        mkfs::run(&root_device_path, MkfsFileSystemType::Ext4).unwrap();

        let _a = setup_mock_grub_configs(&ctx);

        update_configs(&ctx, Path::new(OS_MODIFIER_BINARY_PATH)).unwrap();
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root as an ab volume partition.
    fn test_update_grub_root_abvolume() {
        test_execute_and_resulting_layout(false, false);
        let ctx = EngineContext {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,

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
                    filesystems: vec![
                        FileSystem {
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/efi"),
                                options: MountOptions::empty(),
                            }),
                            device_id: Some("boot".to_owned()),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            mount_point: Some(MountPoint {
                                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                                options: MountOptions::empty(),
                            }),
                            device_id: Some("root".to_owned()),
                            source: FileSystemSource::Image,
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
            partition_paths: btreemap![
                "os".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "efi".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root-a".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                "root-b".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
            ],
            ..Default::default()
        };

        let root_device_path = PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"));
        mkfs::run(&root_device_path, MkfsFileSystemType::Ext4).unwrap();

        let _a = setup_mock_grub_configs(&ctx);

        update_configs(&ctx, Path::new(OS_MODIFIER_BINARY_PATH)).unwrap();
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root on a standalone partition and setting root uuid empty so that the function bails on root_uuid being empty.
    fn test_update_grub_root_uuid_empty() {
        test_execute_and_resulting_layout(false, false);
        let ctx = EngineContext {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,

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
                    filesystems: vec![FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            options: MountOptions::empty(),
                        }),
                        device_id: Some("root".to_owned()),
                        source: FileSystemSource::Image,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "foo".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
            },
            ..Default::default()
        };

        let _a = setup_mock_grub_configs(&ctx);

        let result = update_configs(&ctx, Path::new(ROOT_MOUNT_POINT_PATH));
        assert_eq!(
            result.unwrap_err().to_string(),
            "Failed to get UUID for path '/dev/sdb2', received ''"
        );
    }

    #[functional_test(feature = "helpers")]
    /// This functions tests update_grub by setting up root path empty so that the function bails on root path being None.
    fn test_update_grub_root_path_empty() {
        test_execute_and_resulting_layout(false, false);
        let ctx = EngineContext {
            // These are required to get the update install ID
            servicing_type: ServicingType::CleanInstall,

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
                    filesystems: vec![FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            options: MountOptions::empty(),
                        }),
                        device_id: Some("root".to_owned()),
                        source: FileSystemSource::Image,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "foo".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root".into() => PathBuf::from(""),
            },
            ..Default::default()
        };

        let _a = setup_mock_grub_configs(&ctx);

        let result = update_configs(&ctx, Path::new(ROOT_MOUNT_POINT_PATH));

        assert_eq!(result.unwrap_err().to_string(), "Root device path is none");
    }
}
