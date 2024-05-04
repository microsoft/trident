use std::{collections::HashMap, vec};

use anyhow::{bail, Error};
use url::Url;

use netplan_types::{
    CommonPropertiesAllDevices, CommonPropertiesPhysicalDeviceType, EthernetConfig, MatchConfig,
    NetworkConfig,
};

use crate::{
    config::{
        AbUpdate, AbVolumePair, AdditionalFile, Disk, EncryptedVolume, Encryption, FileSystem,
        FileSystemSource, FileSystemType, HostConfiguration, Image, ImageFormat, ImageSha256,
        MountOptions, MountPoint, Os, Partition, PartitionSize, PartitionTableType, PartitionType,
        Raid, RaidLevel, Script, Scripts, ServicingType, SoftwareRaidArray, SshMode, Storage, User,
        VerityFileSystem,
    },
    constants,
};

pub fn sample_host_configuration(name: &str) -> Result<(&'static str, HostConfiguration), Error> {
    let sample = match name {
        "basic" => (
            "Basic sample with a bootable deployment.",
            HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-1.0".into(),
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
                    filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        fs_type: FileSystemType::Vfat,
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/esp.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        fs_type: FileSystemType::Ext4,
                        mount_point: Some(MountPoint {
                            path: constants::ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/root.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                ],
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        "simple" => (
            "Simple sample showcasing OS config of networking, users, additional files and customer scripts.",
            HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-1.0".into(),
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
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        fs_type: FileSystemType::Vfat,
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/esp.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        fs_type: FileSystemType::Ext4,
                        mount_point: Some(MountPoint {
                            path: constants::ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/root.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
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
                    ..Default::default()
                }],
                ..Default::default()
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
            }
        ),
        "base" => (
            "Base sample config showcasing raid, encryption and A/B update.",
            HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-1.0".into(),
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
                    recovery_key_url: Some(Url::parse("file:///recovery.key").unwrap()),
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
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        fs_type: FileSystemType::Vfat,
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/esp.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        fs_type: FileSystemType::Ext4,
                        mount_point: Some(MountPoint {
                            path: constants::ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/root.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        fs_type: FileSystemType::Ext4,
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Create,
                    },
                    FileSystem {
                        device_id: Some("swap".into()),
                        fs_type: FileSystemType::Swap,
                        mount_point: None,
                        source: FileSystemSource::Create,
                    },
                    FileSystem {
                        device_id: Some("srv".into()),
                        fs_type: FileSystemType::Ext4,
                        mount_point: Some(MountPoint {
                            path: "/srv".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Create,
                    },
                    FileSystem {
                        device_id: Some("some_raid".into()),
                        fs_type: FileSystemType::Ext4,
                        mount_point: Some(MountPoint {
                            path: "/mnt/raid".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Create,
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![AbVolumePair {
                        id: "root".into(),
                        volume_a_id: "root-a".into(),
                        volume_b_id: "root-b".into(),
                    }],
                }),
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
                    ..Default::default()
                }],
                ..Default::default()
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
            }
        ),
        "verity" => (
            "Verity sample showcasing usage of dm-verity.",
            HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-1.0".into(),
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
                        Partition {
                            id: "var".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Fixed(0x40000000), // 1GiB
                        },
                        Partition {
                            id: "run".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Fixed(0x40000000), // 1GiB
                        },
                        Partition {
                            id: "home".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Fixed(0x40000000), // 1GiB
                        },
                    ],
                    adopted_partitions: vec![],
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        fs_type: FileSystemType::Vfat,
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/verity_esp.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                    FileSystem {
                        device_id: Some("boot".into()),
                        fs_type: FileSystemType::Ext4,
                        mount_point: Some(MountPoint {
                            path: constants::BOOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/verity_boot.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "b8170f4c46eab33f641e7e102a573d7ef6d8b27dc912b5ecfb033e82f1fff52a"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident-overlay".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident-overlay".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("var".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Image(Image{
                            url: "file:///trident_cdrom/data/verity_var.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "1876c8c3921570a22d48f0c30e6509ed594cf7c22a2c26a718aadcc901194585"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                        mount_point: Some(MountPoint {
                            path: "/var".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("run".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                        mount_point: Some(MountPoint {
                            path: "/run".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("home".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                        mount_point: Some(MountPoint {
                            path: "/home".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                ],
                verity_filesystems: vec![VerityFileSystem {
                    name: "root".into(),
                    data_device_id: "root".into(),
                    hash_device_id: "root-hash".into(),
                    data_image: Image {
                        url: "file:///trident_cdrom/data/verity_root.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                    },
                    hash_image: Image {
                        url: "file:///trident_cdrom/data/verity_roothash.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e875214b5ba8aac92203b72dbf0f78d673a16bd3c2a6f3577bcf4ed5d7c903af"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                    },
                    fs_type: FileSystemType::Ext4,
                    mount_point: MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        options: MountOptions::defaults(),
                    },
                }],
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
                    ..Default::default()
                }],
                ..Default::default()
            },
            scripts: Scripts {
                post_configure: vec![Script {
                    name: "rw-overlay".into(),
                    servicing_type: vec![ServicingType::All],
                    content: Some("mkdir -p /var/lib/trident-overlay/etc-rw/upper && mkdir -p /var/lib/trident-overlay/etc-rw/work".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
            }
        ),
        "advanced" => (
            "Advanced sample showcasing combination of RAID, encryption, dm-verity and A/B update.",
            HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk1".to_string(),
                        device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-1".into(),
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
                                id: "var-a1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                            Partition {
                                id: "run-a1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                            Partition {
                                id: "var-b1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                            Partition {
                                id: "run-b1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                            Partition {
                                id: "enc-home1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                        ],
                        ..Default::default()
                    },
                    Disk {
                        id: "disk2".to_string(),
                        device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2".into(),
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
                                id: "var-a2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                            Partition {
                                id: "run-a2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                            Partition {
                                id: "var-b2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                            Partition {
                                id: "run-b2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                            Partition {
                                id: "enc-home2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(0x40000000), // 1GiB
                            },
                        ],
                        ..Default::default()
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
                            id: "var-a".to_string(),
                            name: "var-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["var-a1".to_string(), "var-a2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "run-a".to_string(),
                            name: "run-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["run-a1".to_string(), "run-a2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "var-b".to_string(),
                            name: "var-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["var-b1".to_string(), "var-b2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "run-b".to_string(),
                            name: "run-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["run-b1".to_string(), "run-b2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                        SoftwareRaidArray {
                            id: "enc-home".to_string(),
                            name: "home".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["enc-home1".to_string(), "enc-home2".to_string()],
                            metadata_version: "1.0".into(),
                        },
                    ],
                },
                encryption: Some(Encryption {
                    recovery_key_url: Some(Url::parse("file:///recovery.key").unwrap()),
                    volumes: vec![EncryptedVolume {
                        id: "home".to_string(),
                        device_name: "home".to_string(),
                        target_id: "enc-home".to_string(),
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
                        AbVolumePair {
                            id: "var".into(),
                            volume_a_id: "var-a".into(),
                            volume_b_id: "var-b".into(),
                        },
                        AbVolumePair {
                            id: "run".into(),
                            volume_a_id: "run-a".into(),
                            volume_b_id: "run-b".into(),
                        },
                    ],
                }),
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp1".into()),
                        fs_type: FileSystemType::Vfat,
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/verity_esp.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                    FileSystem {
                        device_id: Some("esp2".into()),
                        fs_type: FileSystemType::Vfat,
                        mount_point: None,
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/verity_esp.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                    FileSystem {
                        device_id: Some("boot".into()),
                        fs_type: FileSystemType::Ext4,
                        mount_point: Some(MountPoint {
                            path: constants::BOOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/verity_boot.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "b8170f4c46eab33f641e7e102a573d7ef6d8b27dc912b5ecfb033e82f1fff52a"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident-overlay".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident-overlay".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("srv".into()),
                        fs_type: FileSystemType::Ext4,
                        mount_point: Some(MountPoint {
                            path: "/srv".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Create,
                    },
                    FileSystem {
                        device_id: Some("swap1".into()),
                        fs_type: FileSystemType::Swap,
                        source: FileSystemSource::Create,
                        mount_point: None,
                    },
                    FileSystem {
                        device_id: Some("swap2".into()),
                        fs_type: FileSystemType::Swap,
                        source: FileSystemSource::Create,
                        mount_point: None,
                    },
                    FileSystem {
                        device_id: Some("var".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Image(Image {
                            url: "file:///trident_cdrom/data/verity_var.rawzst".into(),
                            sha256: ImageSha256::Checksum(
                                "1876c8c3921570a22d48f0c30e6509ed594cf7c22a2c26a718aadcc901194585"
                                    .into(),
                            ),
                            format: ImageFormat::RawZst,
                        }),
                        mount_point: Some(MountPoint {
                            path: "/var".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("run".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                        mount_point: Some(MountPoint {
                            path: "/run".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("home".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                        mount_point: Some(MountPoint {
                            path: "/home".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                ],
                verity_filesystems: vec![VerityFileSystem {
                    name: "root".into(),
                    data_device_id: "root".into(),
                    hash_device_id: "root-hash".into(),
                    data_image: Image {
                        url: "file:///trident_cdrom/data/verity_root.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                    },
                    hash_image: Image {
                        url: "file:///trident_cdrom/data/verity_roothash.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e875214b5ba8aac92203b72dbf0f78d673a16bd3c2a6f3577bcf4ed5d7c903af"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                    },
                    fs_type: FileSystemType::Ext4,
                    mount_point: MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        options: MountOptions::defaults(),
                    },
                }],
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
                additional_files: vec![
                    AdditionalFile {
                        destination: "/var/config-script.sh".into(),
                        content: Some("echo 'Hello, world!'".into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            scripts: Scripts {
                post_configure: vec![Script {
                    name: "rw-overlay".into(),
                    servicing_type: vec![ServicingType::All],
                    content: Some("mkdir -p /var/lib/trident-overlay/etc-rw/upper && mkdir -p /var/lib/trident-overlay/etc-rw/work".into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
            }
        ),
        "raid" => (
            "RAID sample showcasing usage of RAID.",
            HostConfiguration {
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
                                    id: "root1".to_string(),
                                    partition_type: PartitionType::Root,
                                    size: PartitionSize::Fixed(0x100000000), // 4GiB
                                },
                                Partition {
                                    id: "swap1".to_string(),
                                    partition_type: PartitionType::Swap,
                                    size: PartitionSize::Fixed(0x80000000), // 2GiB
                                },
                            ],
                            adopted_partitions: vec![],
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
                                    id: "root2".to_string(),
                                    partition_type: PartitionType::Root,
                                    size: PartitionSize::Fixed(0x100000000), // 4GiB
                                },
                                Partition {
                                    id: "swap2".to_string(),
                                    partition_type: PartitionType::Swap,
                                    size: PartitionSize::Fixed(0x80000000), // 2GiB
                                },
                            ],
                            adopted_partitions: vec![],
                        },
                    ],
                    raid: Raid {
                        software: vec![SoftwareRaidArray {
                            id: "root".to_string(),
                            name: "root".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root1".to_string(), "root2".to_string()],
                            metadata_version: "1.0".into(),
                        }],
                    },
                    encryption: None,
                    ab_update: None,
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("esp1".into()),
                            fs_type: FileSystemType::Vfat,
                            mount_point: Some(MountPoint {
                                path: constants::ESP_MOUNT_POINT_PATH.into(),
                                options: MountOptions::new("umask=0077"),
                            }),
                            source: FileSystemSource::Image(Image {
                                url: "file:///trident_cdrom/data/verity_esp.rawzst".into(),
                                sha256: ImageSha256::Checksum(
                                    "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                        .into(),
                                ),
                                format: ImageFormat::RawZst,
                            }),
                        },
                        FileSystem {
                            device_id: Some("esp2".into()),
                            fs_type: FileSystemType::Vfat,
                            mount_point: None,
                            source: FileSystemSource::Image(Image {
                                url: "file:///trident_cdrom/data/verity_esp.rawzst".into(),
                                sha256: ImageSha256::Checksum(
                                    "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                        .into(),
                                ),
                                format: ImageFormat::RawZst,
                            }),
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            fs_type: FileSystemType::Ext4,
                            mount_point: Some(MountPoint {
                                path: constants::ROOT_MOUNT_POINT_PATH.into(),
                                options: MountOptions::defaults(),
                            }),
                            source: FileSystemSource::Image(Image {
                                url: "file:///trident_cdrom/data/verity_root.rawzst".into(),
                                sha256: ImageSha256::Checksum(
                                    "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                        .into(),
                                ),
                                format: ImageFormat::RawZst,
                            }),
                        },
                        FileSystem {
                            device_id: Some("swap1".into()),
                            fs_type: FileSystemType::Swap,
                            source: FileSystemSource::Create,
                            mount_point: None,
                        },
                        FileSystem {
                            device_id: Some("swap2".into()),
                            fs_type: FileSystemType::Swap,
                            source: FileSystemSource::Create,
                            mount_point: None,
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
                    additional_files: vec![],
                    hostname: None,
                },
                scripts: Scripts {
                    post_configure: vec![Script {
                        name: "wheel".into(),
                        servicing_type: vec![ServicingType::CleanInstall, ServicingType::AbUpdate],
                        content: Some(
                            "echo \"%wheel ALL=(ALL:ALL) NOPASSWD: ALL\" > /etc/sudoers.d/wheel"
                                .into(),
                        ),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
        "encryption" => (
            "Encryption sample showcasing usage of encryption",
            HostConfiguration {
                storage: Storage {
                    disks: vec![
                        Disk {
                            id: "disk1".to_string(),
                            device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2".into(),
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
                                    size: PartitionSize::Fixed(0x100000000), // 4GiB
                                },
                                Partition {
                                    id: "swap".to_string(),
                                    partition_type: PartitionType::Swap,
                                    size: PartitionSize::Fixed(0x80000000), // 2GiB
                                },
                                Partition {
                                    id: "luks-srv".to_string(),
                                    partition_type: PartitionType::LinuxGeneric,
                                    size: PartitionSize::Fixed(0x4000000), // 64MiB
                                },
                            ],
                            adopted_partitions: vec![],
                        },
                        Disk {
                            id: "disk2".to_string(),
                            device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-3".into(),
                            partition_table_type: PartitionTableType::Gpt,
                            partitions: vec![],
                            adopted_partitions: vec![],
                        },
                    ],
                    raid: Raid { software: vec![] },
                    encryption: Some(Encryption {
                        recovery_key_url: None,
                        volumes: vec![EncryptedVolume {
                            id: "srv".to_string(),
                            device_name: "srv".to_string(),
                            target_id: "luks-srv".to_string(),
                        }],
                    }),
                    ab_update: None,
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("esp".into()),
                            fs_type: FileSystemType::Vfat,
                            mount_point: Some(MountPoint {
                                path: constants::ESP_MOUNT_POINT_PATH.into(),
                                options: MountOptions::new("umask=0077"),
                            }),
                            source: FileSystemSource::Image(Image {
                                url: "file:///trident_cdrom/data/verity.rawzst".into(),
                                sha256: ImageSha256::Checksum(
                                    "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae"
                                        .into(),
                                ),
                                format: ImageFormat::RawZst,
                            }),
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            fs_type: FileSystemType::Ext4,
                            mount_point: Some(MountPoint {
                                path: constants::ROOT_MOUNT_POINT_PATH.into(),
                                options: MountOptions::defaults(),
                            }),
                            source: FileSystemSource::Image(Image {
                                url: "file:///trident_cdrom/data/root.rawzst".into(),
                                sha256: ImageSha256::Checksum(
                                    "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                        .into(),
                                ),
                                format: ImageFormat::RawZst,
                            }),
                        },
                        FileSystem {
                            device_id: Some("swap".into()),
                            fs_type: FileSystemType::Swap,
                            source: FileSystemSource::Create,
                            mount_point: None,
                        },
                        FileSystem {
                            device_id: Some("srv".into()),
                            fs_type: FileSystemType::Ext4,
                            mount_point: Some(MountPoint {
                                path: "/srv".into(),
                                options: MountOptions::defaults(),
                            }),
                            source: FileSystemSource::Create,
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
                    additional_files: vec![],
                    hostname: None,
                },
                scripts: Scripts {
                    post_configure: vec![Script {
                        name: "wheel".into(),
                        servicing_type: vec![ServicingType::CleanInstall, ServicingType::AbUpdate],
                        content: Some(
                            "echo \"%wheel ALL=(ALL:ALL) NOPASSWD: ALL\" > /etc/sudoers.d/wheel"
                                .into(),
                        ),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
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
        let (_, host_configuration) = sample_host_configuration("basic").unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);
        assert!(&host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.filesystems.len(), 2);
        assert_eq!(host_configuration.storage.verity_filesystems.len(), 0);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.network.is_none());
        assert_eq!(host_configuration.os.users.len(), 0);
    }

    #[test]
    fn test_build_simple_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("simple").unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);
        assert!(&host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.filesystems.len(), 2);
        assert_eq!(host_configuration.storage.verity_filesystems.len(), 0);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_base_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("base").unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);

        assert!(host_configuration.storage.encryption.is_some());
        if let Some(encryption) = &host_configuration.storage.encryption {
            assert_eq!(encryption.volumes.len(), 1);
        }

        assert_eq!(host_configuration.storage.raid.software.len(), 1);
        assert_eq!(host_configuration.storage.filesystems.len(), 6);
        assert_eq!(host_configuration.storage.verity_filesystems.len(), 0);
        assert!(host_configuration.storage.ab_update.is_some());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_verity_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("verity").unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);
        assert!(host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.filesystems.len(), 7);
        assert_eq!(host_configuration.storage.verity_filesystems.len(), 1);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_advanced_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("advanced").unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 2);

        assert!(host_configuration.storage.encryption.is_some());
        if let Some(encryption) = &host_configuration.storage.encryption {
            assert_eq!(encryption.volumes.len(), 1);
        }

        assert_eq!(host_configuration.storage.raid.software.len(), 14);
        assert_eq!(host_configuration.storage.filesystems.len(), 11);
        assert_eq!(host_configuration.storage.verity_filesystems.len(), 1);
        assert!(host_configuration.storage.ab_update.is_some());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_raid_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("raid").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 2);

        assert!(host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 1);
        assert_eq!(host_configuration.storage.filesystems.len(), 5);
        assert_eq!(host_configuration.storage.verity_filesystems.len(), 0);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_encryption_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("encryption").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 2);

        assert!(host_configuration.storage.encryption.is_some());
        if let Some(encryption) = &host_configuration.storage.encryption {
            assert_eq!(encryption.volumes.len(), 1);
        }

        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.filesystems.len(), 4);
        assert_eq!(host_configuration.storage.verity_filesystems.len(), 0);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.network.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }
}
