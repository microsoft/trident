use anyhow::{bail, Error};
use std::collections::HashMap;
use uuid::Uuid;

use netplan_types::{
    CommonPropertiesAllDevices, CommonPropertiesPhysicalDeviceType, EthernetConfig, MatchConfig,
    NetworkConfig,
};

use crate::{
    config::{
        AbUpdate, AbVolumePair, AdditionalFile, AdoptedPartition, Disk, EncryptedVolume,
        Encryption, FileSystemType, HostConfiguration, Image, ImageFormat, ImageSha256, MountPoint,
        Os, Partition, PartitionSize, PartitionTableType, PartitionType, Raid, RaidLevel, Script,
        Scripts, ServicingType, SoftwareRaidArray, SshMode, Storage, User, VerityDevice,
    },
    constants,
};
use url::Url;

pub fn sample_host_configuration(name: &str) -> Result<HostConfiguration, Error> {
    let sample = match name {
        "basic" => HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::Fixed(0x4000000), // 64MiB
                        },
                        Partition {
                            id: "root".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::Fixed(0x200000000), // 8GiB
                        },
                    ],
                    adopted_partitions: vec![],
                }],
                mount_points: vec![
                    MountPoint {
                        path: constants::ESP_MOUNT_POINT_PATH.into(),
                        target_id: "esp".into(),
                        filesystem: FileSystemType::Vfat,
                        options: vec!["umask=0077".into()],
                    },
                    MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        target_id: "root".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                ],
                images: vec![
                    Image {
                        url: "file:///trident_cdrom/data/esp.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "esp".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/root.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "root".into(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        },
        "simple" => HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::Fixed(0x4000000), // 64MiB
                        },
                        Partition {
                            id: "root".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::Fixed(0x200000000), // 8GiB
                        },
                    ],
                    adopted_partitions: vec![],
                }],
                mount_points: vec![
                    MountPoint {
                        path: constants::ESP_MOUNT_POINT_PATH.into(),
                        target_id: "esp".into(),
                        filesystem: FileSystemType::Vfat,
                        options: vec!["umask=0077".into()],
                    },
                    MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        target_id: "root".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                ],
                images: vec![
                    Image {
                        url: "file:///trident_cdrom/data/esp.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "esp".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/root.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "root".into(),
                    },
                ],
                ..Default::default()
            },
            os: Os {
                users: vec![User {
                    name: "my-custom-user".into(),
                    ssh_public_keys: vec!["<MY_PUBLIC_SSH_KEY>".into()],
                    ssh_mode: SshMode::KeyOnly,
                    ..Default::default()
                }],
                network: Some(NetworkConfig {
                    version: 2,
                    ethernets: Some(HashMap::from([(
                        "eths".into(),
                        EthernetConfig {
                            common_all: Some(CommonPropertiesAllDevices {
                                dhcp4: Some(true),
                                ..Default::default()
                            }),
                            common_physical: Some(CommonPropertiesPhysicalDeviceType {
                                r#match: Some(MatchConfig {
                                    name: Some("enp*".into()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )])),
                    ..Default::default()
                }),
                additional_files: vec![AdditionalFile {
                    destination: "/var/config-script.sh".into(),
                    content: Some(
                        "echo 'Running from newly deployed chroot: $SAMPLE_VARIABLE'".into(),
                    ),
                    permissions: Some("0755".into()),
                    ..Default::default()
                }],
            },
            scripts: Scripts {
                post_provision: vec![Script {
                    name: "sample-provision-script".into(),
                    servicing_type: vec![ServicingType::CleanInstall, ServicingType::AbUpdate],
                    content: Some("ls /mnt/newroot".into()),
                    log_file_path: Some("/var/log/sample-provision-script.log".into()),
                    ..Default::default()
                }],
                post_configure: vec![Script {
                    name: "sample-configure-script".into(),
                    servicing_type: vec![ServicingType::All],
                    content: Some("/var/config-script.sh".into()),
                    environment_variables: HashMap::from([(
                        "SAMPLE_VARIABLE".into(),
                        "sample-variable-value".into(),
                    )]),
                    log_file_path: Some("/var/log/sample-configure-script.log".into()),
                    ..Default::default()
                }],
            },
            ..Default::default()
        },
        "base" => HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::Fixed(0x4000000), // 64MiB
                        },
                        Partition {
                            id: "root-a".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::Fixed(0x200000000), // 8GiB
                        },
                        Partition {
                            id: "root-b".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::Fixed(0x200000000), // 8GiB
                        },
                        Partition {
                            id: "swap".to_string(),
                            partition_type: PartitionType::Swap,
                            size: PartitionSize::Fixed(0x80000000), // 2GiB
                        },
                        Partition {
                            id: "trident".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Fixed(0x8000000), // 1GiB
                        },
                        Partition {
                            id: "enc-srv".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Fixed(0x40000000), // 128MiB
                        },
                        Partition {
                            id: "raid-a".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Fixed(0x40000000), // 1GiB
                        },
                        Partition {
                            id: "raid-b".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Fixed(0x40000000), // 1GiB
                        },
                    ],
                    adopted_partitions: vec![],
                }],
                encryption: Some(Encryption {
                    recovery_key_url: Some(
                        Url::parse("file:///trident_cdrom/data/recovery.key").unwrap(),
                    ),

                    volumes: vec![EncryptedVolume {
                        id: "srv".to_string(),
                        device_name: "luks-srv".to_string(),
                        target_id: "enc-srv".to_string(),
                    }],
                }),
                raid: Raid {
                    software: vec![SoftwareRaidArray {
                        id: "some_raid".to_string(),
                        name: "some_raid1".to_string(),
                        level: RaidLevel::Raid1,
                        devices: vec!["raid-a".to_string(), "raid-b".to_string()],
                        metadata_version: "1.0".into(),
                    }],
                },
                verity: vec![],
                mount_points: vec![
                    MountPoint {
                        path: constants::ESP_MOUNT_POINT_PATH.into(),
                        target_id: "esp".into(),
                        filesystem: FileSystemType::Vfat,
                        options: vec!["umask=0077".into()],
                    },
                    MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        target_id: "root".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                    MountPoint {
                        path: "/var/lib/trident".into(),
                        target_id: "trident".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                    MountPoint {
                        path: "none".into(),
                        target_id: "swap".into(),
                        filesystem: FileSystemType::Swap,
                        options: vec!["sw".into()],
                    },
                    MountPoint {
                        path: "/srv".into(),
                        target_id: "srv".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                    MountPoint {
                        path: "/mnt/raid".into(),
                        target_id: "some_raid".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                ],
                images: vec![
                    Image {
                        url: "file:///trident_cdrom/data/esp.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "esp".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/root.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "root".into(),
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![AbVolumePair {
                        id: "root".into(),
                        volume_a_id: "root-a".into(),
                        volume_b_id: "root-b".into(),
                    }],
                }),
            },
            os: Os {
                users: vec![User {
                    name: "my-custom-user".into(),
                    ssh_public_keys: vec!["<MY_PUBLIC_SSH_KEY>".into()],
                    ssh_mode: SshMode::KeyOnly,
                    ..Default::default()
                }],
                network: Some(NetworkConfig {
                    version: 2,
                    ethernets: Some(HashMap::from([(
                        "eths".into(),
                        EthernetConfig {
                            common_all: Some(CommonPropertiesAllDevices {
                                dhcp4: Some(true),
                                ..Default::default()
                            }),
                            common_physical: Some(CommonPropertiesPhysicalDeviceType {
                                r#match: Some(MatchConfig {
                                    name: Some("enp*".into()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )])),
                    ..Default::default()
                }),
                additional_files: vec![AdditionalFile {
                    destination: "/var/config-script.sh".into(),
                    content: Some(
                        "echo 'Running from newly deployed chroot: $SAMPLE_VARIABLE'".into(),
                    ),
                    permissions: Some("0755".into()),
                    ..Default::default()
                }],
            },
            scripts: Scripts {
                post_provision: vec![Script {
                    name: "sample-provision-script".into(),
                    servicing_type: vec![ServicingType::CleanInstall, ServicingType::AbUpdate],
                    content: Some("ls /mnt/newroot".into()),
                    log_file_path: Some("/var/log/sample-provision-script.log".into()),
                    ..Default::default()
                }],
                post_configure: vec![Script {
                    name: "sample-configure-script".into(),
                    servicing_type: vec![ServicingType::All],
                    content: Some("/var/config-script.sh".into()),
                    environment_variables: HashMap::from([(
                        "SAMPLE_VARIABLE".into(),
                        "sample-variable-value".into(),
                    )]),
                    log_file_path: Some("/var/log/sample-configure-script.log".into()),
                    ..Default::default()
                }],
            },
            ..Default::default()
        },
        "verity" => HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::Fixed(0x4000000), // 64MiB
                        },
                        Partition {
                            id: "boot".to_string(),
                            partition_type: PartitionType::Xbootldr,
                            size: PartitionSize::Fixed(0x20000000), // 512MiB
                        },
                        Partition {
                            id: "root".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::Fixed(0x200000000), // 8GiB
                        },
                        Partition {
                            id: "root-hash".to_string(),
                            partition_type: PartitionType::RootVerity,
                            size: PartitionSize::Fixed(0x19000000), // 400MiB
                        },
                        Partition {
                            id: "trident".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Fixed(0x8000000), // 128MiB
                        },
                        Partition {
                            id: "trident-overlay".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Fixed(0x8000000), // 128MiB
                        },
                    ],
                    adopted_partitions: vec![],
                }],
                images: vec![
                    Image {
                        url: "file:///trident_cdrom/data/esp.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "esp".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/boot.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "b8170f4c46eab33f641e7e102a573d7ef6d8b27dc912b5ecfb033e82f1fff52a"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "boot".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/root.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "root".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/roothash.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e875214b5ba8aac92203b72dbf0f78d673a16bd3c2a6f3577bcf4ed5d7c903af"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "root-hash".into(),
                    },
                ],
                verity: vec![VerityDevice {
                    id: "root-verity".into(),
                    device_name: "root".into(),
                    data_target_id: "root".into(),
                    hash_target_id: "root-hash".into(),
                }],
                mount_points: vec![
                    MountPoint {
                        path: constants::ESP_MOUNT_POINT_PATH.into(),
                        target_id: "esp".into(),
                        filesystem: FileSystemType::Vfat,
                        options: vec!["umask=0077".into()],
                    },
                    MountPoint {
                        path: constants::BOOT_MOUNT_POINT_PATH.into(),
                        target_id: "boot".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                    MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        target_id: "root-verity".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into(), "ro".into()],
                    },
                    MountPoint {
                        path: "/var/lib/trident".into(),
                        target_id: "trident".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                    MountPoint {
                        path: "/var/lib/trident-overlay".into(),
                        target_id: "trident-overlay".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                ],
                ..Default::default()
            },
            os: Os {
                users: vec![User {
                    name: "my-custom-user".into(),
                    ssh_public_keys: vec!["<MY_PUBLIC_SSH_KEY>".into()],
                    ssh_mode: SshMode::KeyOnly,
                    ..Default::default()
                }],
                network: Some(NetworkConfig {
                    version: 2,
                    ethernets: Some(HashMap::from([(
                        "eths".into(),
                        EthernetConfig {
                            common_all: Some(CommonPropertiesAllDevices {
                                dhcp4: Some(true),
                                ..Default::default()
                            }),
                            common_physical: Some(CommonPropertiesPhysicalDeviceType {
                                r#match: Some(MatchConfig {
                                    name: Some("enp*".into()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )])),
                    ..Default::default()
                }),
                additional_files: vec![AdditionalFile {
                    destination: "/var/config-script.sh".into(),
                    content: Some(
                        "echo 'Running from newly deployed chroot: $SAMPLE_VARIABLE'".into(),
                    ),
                    permissions: Some("0755".into()),
                    ..Default::default()
                }],
            },
            scripts: Scripts {
                post_provision: vec![Script {
                    name: "sample-provision-script".into(),
                    servicing_type: vec![ServicingType::CleanInstall, ServicingType::AbUpdate],
                    content: Some("ls /mnt/newroot".into()),
                    log_file_path: Some("/var/log/sample-provision-script.log".into()),
                    ..Default::default()
                }],
                post_configure: vec![Script {
                    name: "sample-configure-script".into(),
                    servicing_type: vec![ServicingType::All],
                    content: Some("/var/config-script.sh".into()),
                    environment_variables: HashMap::from([(
                        "SAMPLE_VARIABLE".into(),
                        "sample-variable-value".into(),
                    )]),
                    log_file_path: Some("/var/log/sample-configure-script.log".into()),
                    ..Default::default()
                }],
            },
            ..Default::default()
        },
        "advanced" => HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk1".to_string(),
                        device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2".into(),
                        partition_table_type: PartitionTableType::Gpt,
                        partitions: vec![
                            Partition {
                                id: "esp1".to_string(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::Fixed(0x4000000), // 64MiB
                            },
                            Partition {
                                id: "boot-a1".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: PartitionSize::Fixed(0x20000000), // 512MiB
                            },
                            Partition {
                                id: "boot-b1".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: PartitionSize::Fixed(0x20000000), // 512MiB
                            },
                            Partition {
                                id: "root-a1".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(0x100000000), // 4GiB
                            },
                            Partition {
                                id: "root-b1".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(0x100000000), // 4GiB
                            },
                            Partition {
                                id: "root-hash-a1".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::Fixed(0x19000000), // 400MiB
                            },
                            Partition {
                                id: "root-hash-b1".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::Fixed(0x19000000), // 400MiB
                            },
                            Partition {
                                id: "swap1".to_string(),
                                partition_type: PartitionType::Swap,
                                size: PartitionSize::Fixed(0x80000000), // 2GiB
                            },
                            Partition {
                                id: "trident1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x8000000), // 128MiB
                            },
                            Partition {
                                id: "trident-overlay-a1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x8000000), // 128MiB
                            },
                            Partition {
                                id: "trident-overlay-b1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x8000000), // 128MiB
                            },
                            Partition {
                                id: "enc-srv1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                        ],
                        adopted_partitions: vec![AdoptedPartition {
                            id: "bootstrap".to_string(),
                            uuid: Some(
                                Uuid::parse_str("a0a0a0a0-a0a0-a0a0-a0a0-a0a0a0a0a0a0").unwrap(),
                            ),
                            ..Default::default()
                        }],
                    },
                    Disk {
                        id: "disk2".to_string(),
                        device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-3".into(),
                        partition_table_type: PartitionTableType::Gpt,
                        partitions: vec![
                            Partition {
                                id: "esp2".to_string(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::Fixed(0x4000000), // 64MiB
                            },
                            Partition {
                                id: "boot-a2".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: PartitionSize::Fixed(0x20000000), // 512MiB
                            },
                            Partition {
                                id: "boot-b2".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: PartitionSize::Fixed(0x20000000), // 512MiB
                            },
                            Partition {
                                id: "root-a2".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(0x100000000), // 4GiB
                            },
                            Partition {
                                id: "root-b2".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(0x100000000), // 4GiB
                            },
                            Partition {
                                id: "root-hash-a2".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::Fixed(0x19000000), // 400MiB
                            },
                            Partition {
                                id: "root-hash-b2".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::Fixed(0x19000000), // 400MiB
                            },
                            Partition {
                                id: "swap2".to_string(),
                                partition_type: PartitionType::Swap,
                                size: PartitionSize::Fixed(0x80000000), // 2GiB
                            },
                            Partition {
                                id: "trident2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x8000000), // 128MiB
                            },
                            Partition {
                                id: "trident-overlay-a2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x8000000), // 128MiB
                            },
                            Partition {
                                id: "trident-overlay-b2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x8000000), // 128MiB
                            },
                            Partition {
                                id: "enc-srv2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                        ],
                        adopted_partitions: vec![],
                    },
                ],
                raid: Raid {
                    software: vec![
                        SoftwareRaidArray {
                            id: "boot-a".to_string(),
                            name: "boot-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["boot-a1".to_string(), "boot-a2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "boot-b".to_string(),
                            name: "boot-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["boot-b1".to_string(), "boot-b2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "root-a".to_string(),
                            name: "root-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root-a1".to_string(), "root-a2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "root-b".to_string(),
                            name: "root-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root-b1".to_string(), "root-b2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "root-hash-a".to_string(),
                            name: "root-hash-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root-hash-a1".to_string(), "root-hash-a2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "root-hash-b".to_string(),
                            name: "root-hash-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root-hash-b1".to_string(), "root-hash-b2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "trident".to_string(),
                            name: "trident".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["trident1".to_string(), "trident2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "trident-overlay-a".to_string(),
                            name: "trident-overlay-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec![
                                "trident-overlay-a1".to_string(),
                                "trident-overlay-a2".to_string(),
                            ],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "trident-overlay-b".to_string(),
                            name: "trident-overlay-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec![
                                "trident-overlay-b1".to_string(),
                                "trident-overlay-b2".to_string(),
                            ],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "enc-srv".to_string(),
                            name: "enc-srv".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["enc-srv1".to_string(), "enc-srv2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                    ],
                },
                encryption: Some(Encryption {
                    recovery_key_url: Some(Url::parse("file:///recovery.key").unwrap()),
                    volumes: vec![EncryptedVolume {
                        id: "srv".to_string(),
                        device_name: "luks-srv".to_string(),
                        target_id: "enc-srv".to_string(),
                    }],
                }),
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![
                        AbVolumePair {
                            id: "boot".into(),
                            volume_a_id: "boot-a".into(),
                            volume_b_id: "boot-b".into(),
                        },
                        AbVolumePair {
                            id: "root".into(),
                            volume_a_id: "root-a".into(),
                            volume_b_id: "root-b".into(),
                        },
                        AbVolumePair {
                            id: "root-hash".into(),
                            volume_a_id: "root-hash-a".into(),
                            volume_b_id: "root-hash-b".into(),
                        },
                        AbVolumePair {
                            id: "trident-overlay".into(),
                            volume_a_id: "trident-overlay-a".into(),
                            volume_b_id: "trident-overlay-b".into(),
                        },
                    ],
                }),
                images: vec![
                    Image {
                        url: "file:///trident_cdrom/data/esp.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "esp1".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/esp.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "esp2".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/boot.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "b8170f4c46eab33f641e7e102a573d7ef6d8b27dc912b5ecfb033e82f1fff52a"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "boot".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/root.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "root".into(),
                    },
                    Image {
                        url: "file:///trident_cdrom/data/roothash.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e875214b5ba8aac92203b72dbf0f78d673a16bd3c2a6f3577bcf4ed5d7c903af"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "root-hash".into(),
                    },
                ],
                verity: vec![VerityDevice {
                    id: "root-verity".into(),
                    device_name: "root".into(),
                    data_target_id: "root".into(),
                    hash_target_id: "root-hash".into(),
                }],
                mount_points: vec![
                    MountPoint {
                        path: constants::ESP_MOUNT_POINT_PATH.into(),
                        target_id: "esp1".into(),
                        filesystem: FileSystemType::Vfat,
                        options: vec!["umask=0077".into()],
                    },
                    MountPoint {
                        path: constants::BOOT_MOUNT_POINT_PATH.into(),
                        target_id: "boot".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                    MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        target_id: "root-verity".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into(), "ro".into()],
                    },
                    MountPoint {
                        path: "/var/lib/trident".into(),
                        target_id: "trident".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                    MountPoint {
                        path: "/var/lib/trident-overlay".into(),
                        target_id: "trident-overlay".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                    MountPoint {
                        path: "none".into(),
                        target_id: "swap1".into(),
                        filesystem: FileSystemType::Swap,
                        options: vec!["sw".into()],
                    },
                    MountPoint {
                        path: "none".into(),
                        target_id: "swap2".into(),
                        filesystem: FileSystemType::Swap,
                        options: vec!["sw".into()],
                    },
                    MountPoint {
                        path: "/srv".into(),
                        target_id: "srv".into(),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".into()],
                    },
                ],
            },
            os: Os {
                users: vec![User {
                    name: "my-custom-user".into(),
                    ssh_public_keys: vec!["<MY_PUBLIC_SSH_KEY>".into()],
                    ssh_mode: SshMode::KeyOnly,
                    ..Default::default()
                }],
                network: Some(NetworkConfig {
                    version: 2,
                    ethernets: Some(HashMap::from([(
                        "eths".into(),
                        EthernetConfig {
                            common_all: Some(CommonPropertiesAllDevices {
                                dhcp4: Some(true),
                                ..Default::default()
                            }),
                            common_physical: Some(CommonPropertiesPhysicalDeviceType {
                                r#match: Some(MatchConfig {
                                    name: Some("enp*".into()),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )])),
                    ..Default::default()
                }),
                additional_files: vec![
                    AdditionalFile {
                        destination: "/var/config-script.sh".into(),
                        content: Some("echo 'Hello, world!'".into()),
                        permissions: Some("0755".into()),
                        ..Default::default()
                    },
                    AdditionalFile {
                        destination: "/var/lib/cloud/instance/user-data".into(),
                        content: Some("#cloud-config".into()),
                        ..Default::default()
                    },
                ],
            },
            scripts: Scripts {
                post_provision: vec![Script {
                    name: "sample-provision-script".into(),
                    servicing_type: vec![ServicingType::CleanInstall, ServicingType::AbUpdate],
                    content: Some("echo 'Post provision!'".into()),
                    log_file_path: Some("/var/log/sample-provision-script.log".into()),
                    ..Default::default()
                }],
                post_configure: vec![Script {
                    name: "sample-configure-script".into(),
                    servicing_type: vec![ServicingType::All],
                    content: Some("/var/config-script.sh".into()),
                    environment_variables: HashMap::from([(
                        "SAMPLE_VARIABLE".into(),
                        "sample-variable-value".into(),
                    )]),
                    log_file_path: Some("/var/log/sample-configure-script.log".into()),
                    ..Default::default()
                }],
            },
            ..Default::default()
        },
        _ => bail!("Unsupported sample name"),
    };

    Ok(sample)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test is mostly to ensure that we try to build a host configuration
    /// and fail if the build fails to let us know that the sample is out of date.
    #[test]
    fn test_build_basic_host_configuration() {
        let host_configuration = sample_host_configuration("basic").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);
        assert!(&host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.mount_points.len(), 2);
        assert_eq!(host_configuration.storage.images.len(), 2);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.network.is_none());
        assert_eq!(host_configuration.os.users.len(), 0);
        assert_eq!(host_configuration.storage.verity.len(), 0);
    }

    #[test]
    fn test_build_simple_host_configuration() {
        let host_configuration = sample_host_configuration("simple").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);
        assert!(&host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.mount_points.len(), 2);
        assert_eq!(host_configuration.storage.images.len(), 2);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
        assert_eq!(host_configuration.storage.verity.len(), 0);
    }

    #[test]
    fn test_build_base_host_configuration() {
        let host_configuration = sample_host_configuration("base").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);

        assert!(host_configuration.storage.encryption.is_some());
        if let Some(encryption) = &host_configuration.storage.encryption {
            assert_eq!(encryption.volumes.len(), 1);
        }

        assert_eq!(host_configuration.storage.raid.software.len(), 1);
        assert_eq!(host_configuration.storage.mount_points.len(), 6);
        assert_eq!(host_configuration.storage.images.len(), 2);
        assert!(host_configuration.storage.ab_update.is_some());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
        assert_eq!(host_configuration.storage.verity.len(), 0);
    }

    #[test]
    fn test_build_verity_host_configuration() {
        let host_configuration = sample_host_configuration("verity").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);
        assert!(host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.mount_points.len(), 5);
        assert_eq!(host_configuration.storage.images.len(), 4);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
        assert_eq!(host_configuration.storage.verity.len(), 1);
    }

    #[test]
    fn test_build_advanced_host_configuration() {
        let host_configuration = sample_host_configuration("advanced").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 2);

        assert!(host_configuration.storage.encryption.is_some());
        if let Some(encryption) = &host_configuration.storage.encryption {
            assert_eq!(encryption.volumes.len(), 1);
        }

        assert_eq!(host_configuration.storage.raid.software.len(), 10);
        assert_eq!(host_configuration.storage.mount_points.len(), 8);
        assert_eq!(host_configuration.storage.images.len(), 5);
        assert!(host_configuration.storage.ab_update.is_some());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
        assert_eq!(host_configuration.storage.verity.len(), 1);
    }
}
