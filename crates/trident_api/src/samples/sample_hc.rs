use std::{collections::HashMap, vec};

use anyhow::{bail, Error};
use url::Url;

use netplan_types::{
    CommonPropertiesAllDevices, CommonPropertiesPhysicalDeviceType, EthernetConfig, MatchConfig,
    NetworkConfig,
};

use crate::{
    config::{
        host::os::{KernelCommandLine, Selinux, SelinuxMode},
        AbUpdate, AbVolumePair, AdditionalFile, Disk, EncryptedVolume, Encryption, FileSystem,
        FileSystemSource, HostConfiguration, ImageSha384, MountOptions, MountPoint,
        NewFileSystemType, Os, OsImage, Partition, PartitionTableType, PartitionType, Raid,
        RaidLevel, Script, ScriptSource, Scripts, Services, ServicingTypeSelection,
        SoftwareRaidArray, SshMode, Storage, Swap, User, VerityDevice,
    },
    constants::{self, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
};
use sysdefs::tpm2::Pcr;

const SAMPLE_SHA384: &str = "ec9a9aa23f02b30f4ec6a168b9bc24733b652eeab4f8abc243630666a5e34cea1667c34313a13ec1564ac4871b80112f";

pub fn sample_host_configuration(name: &str) -> Result<(&'static str, HostConfiguration), Error> {
    let sample = match name {
        "basic" => (
            "Basic sample with a bootable deployment.",
            HostConfiguration {
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/image.cosi").unwrap(),
                    sha384: ImageSha384::Checksum(SAMPLE_SHA384.into()),
                }),
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                        partition_table_type: PartitionTableType::Gpt,
                        partitions: vec![
                            Partition {
                                id: "esp".to_string(),
                                partition_type: PartitionType::Esp,
                                size: 0x4000000.into(), // 64MiB
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: 0x200000000.into(), // 8GiB
                            },
                        ],
                        adopted_partitions: vec![],
                    }],
                    filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        mount_point: Some(MountPoint {
                            path: constants::ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image,
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
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/image.cosi").unwrap(),
                    sha384: ImageSha384::Checksum(SAMPLE_SHA384.into()),
                }),
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 0x4000000.into(), // 64MiB
                        },
                        Partition {
                            id: "root".to_string(),
                            partition_type: PartitionType::Root,
                            size: 0x200000000.into(), // 8GiB
                        },
                    ],
                    adopted_partitions: vec![],
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        mount_point: Some(MountPoint {
                            path: constants::ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image,
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
                netplan: Some(NetworkConfig {
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
                ..Default::default()
            },
            scripts: Scripts {
                pre_servicing: vec![Script {
                    name: "sample-pre-servicing-script".into(),
                    run_on: vec![ServicingTypeSelection::All],
                    source: ScriptSource::Content("echo 'Running before Trident servicing'".into()),
                    ..Default::default()
                }],
                post_provision: vec![Script {
                    name: "sample-provision-script".into(),
                    run_on: vec![ServicingTypeSelection::CleanInstall, ServicingTypeSelection::AbUpdate],
                    source: ScriptSource::Content("ls".into()),
                    arguments: vec!["$TARGET_ROOT".into(), "-l".into()],
                    ..Default::default()
                }],
                post_configure: vec![Script {
                    name: "sample-configure-script".into(),
                    run_on: vec![ServicingTypeSelection::All],
                    source: ScriptSource::Content("/var/config-script.sh".into()),
                    environment_variables: HashMap::from([(
                        "SAMPLE_VARIABLE".into(),
                        "sample-variable-value".into(),
                    )]),
                    ..Default::default()
                }],
            },
            ..Default::default()
            }
        ),
        "base" => (
            "Base sample config showcasing raid, encryption and A/B update.",
            HostConfiguration {
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/image.cosi").unwrap(),
                    sha384: ImageSha384::Checksum(SAMPLE_SHA384.into()),
                }),
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 0x4000000.into(), // 64MiB
                        },
                        Partition {
                            id: "root-a".to_string(),
                            partition_type: PartitionType::Root,
                            size: 0x200000000.into(), // 8GiB
                        },
                        Partition {
                            id: "root-b".to_string(),
                            partition_type: PartitionType::Root,
                            size: 0x200000000.into(), // 8GiB
                        },
                        Partition {
                            id: "swap".to_string(),
                            partition_type: PartitionType::Swap,
                            size: 0x80000000.into(), // 2GiB
                        },
                        Partition {
                            id: "trident".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 0x8000000.into(), // 1GiB
                        },
                        Partition {
                            id: "enc-srv".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 0x40000000.into(), // 128MiB
                        },
                        Partition {
                            id: "raid-a".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 0x40000000.into(), // 1GiB
                        },
                        Partition {
                            id: "raid-b".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 0x40000000.into(), // 1GiB
                        },
                    ],
                    adopted_partitions: vec![],
                }],
                encryption: Some(Encryption {
                    recovery_key_url: Some(Url::parse("file:///recovery.key").unwrap()),
                    volumes: vec![EncryptedVolume {
                        id: "srv".to_string(),
                        device_name: "luks-srv".to_string(),
                        device_id: "enc-srv".to_string(),
                    }],
                    pcrs: vec![Pcr::Pcr7],
                    ..Default::default()
                }),
                raid: Raid {
                    software: vec![SoftwareRaidArray {
                        id: "some_raid".to_string(),
                        name: "some_raid1".to_string(),
                        level: RaidLevel::Raid1,
                        devices: vec!["raid-a".to_string(), "raid-b".to_string()],
                    }],
                    ..Default::default()
                },
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        mount_point: Some(MountPoint {
                            path: constants::ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                    },
                    FileSystem {
                        device_id: Some("srv".into()),
                        mount_point: Some(MountPoint {
                            path: "/srv".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                    },
                    FileSystem {
                        device_id: Some("some_raid".into()),
                        mount_point: Some(MountPoint {
                            path: "/mnt/raid".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![AbVolumePair {
                        id: "root".into(),
                        volume_a_id: "root-a".into(),
                        volume_b_id: "root-b".into(),
                    }],
                }),
                swap: vec![Swap {
                    device_id: "swap".into(),
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
                netplan: Some(NetworkConfig {
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
                ..Default::default()
            },
            scripts: Scripts {
                pre_servicing: vec![Script {
                    name: "sample-pre-servicing-script".into(),
                    run_on: vec![ServicingTypeSelection::All],
                    source: ScriptSource::Content("echo 'Running before Trident servicing'".into()),
                    ..Default::default()
                }],
                post_provision: vec![Script {
                    name: "sample-provision-script".into(),
                    run_on: vec![ServicingTypeSelection::CleanInstall, ServicingTypeSelection::AbUpdate],
                    source: ScriptSource::Content("ls $TARGET_ROOT".into()),
                    ..Default::default()
                }],
                post_configure: vec![Script {
                    name: "sample-configure-script".into(),
                    run_on: vec![ServicingTypeSelection::All],
                    source: ScriptSource::Content("/var/config-script.sh".into()),
                    environment_variables: HashMap::from([(
                        "SAMPLE_VARIABLE".into(),
                        "sample-variable-value".into(),
                    )]),
                    ..Default::default()
                }],
            },
            ..Default::default()
            }
        ),
        "verity" => (
            "Verity sample showcasing usage of dm-verity.",
            HostConfiguration {
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/verity_image.cosi").unwrap(),
                    sha384: ImageSha384::Checksum(SAMPLE_SHA384.into()),
                }),
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 0x4000000.into(), // 64MiB
                        },
                        Partition {
                            id: "boot".to_string(),
                            partition_type: PartitionType::Xbootldr,
                            size: 0x20000000.into(), // 512MiB
                        },
                        Partition {
                            id: "root-data".to_string(),
                            partition_type: PartitionType::Root,
                            size: 0x200000000.into(), // 8GiB
                        },
                        Partition {
                            id: "root-hash".to_string(),
                            partition_type: PartitionType::RootVerity,
                            size: 0x19000000.into(), // 400MiB
                        },
                        Partition {
                            id: "trident".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 0x8000000.into(), // 128MiB
                        },
                        Partition {
                            id: "trident-overlay".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 0x8000000.into(), // 128MiB
                        },
                        Partition {
                            id: "var".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 0x40000000.into(), // 1GiB
                        },
                        Partition {
                            id: "home".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 0x40000000.into(), // 1GiB
                        },
                    ],
                    adopted_partitions: vec![],
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("boot".into()),
                        mount_point: Some(MountPoint {
                            path: constants::BOOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident-overlay".into()),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident-overlay".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("var".into()),
                        source: FileSystemSource::Image,
                        mount_point: Some(MountPoint {
                            path: "/var".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("home".into()),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                        mount_point: Some(MountPoint {
                            path: "/home".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        source: FileSystemSource::Image,
                        mount_point: Some(MountPoint {
                            path: ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
                        }),
                    }
                ],
                verity: vec![VerityDevice {
                    id: "root".into(),
                    data_device_id: "root-data".into(),
                    hash_device_id: "root-hash".into(),
                    name: "root".into(),
                    ..Default::default()
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
                netplan: Some(NetworkConfig {
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
                    run_on: vec![ServicingTypeSelection::All],
                    source: ScriptSource::Content("mkdir -p /var/lib/trident-overlay/etc-rw/upper && mkdir -p /var/lib/trident-overlay/etc-rw/work".into()),
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
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/verity_image.cosi").unwrap(),
                    sha384: ImageSha384::Checksum(SAMPLE_SHA384.into()),
                }),
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
                                size: 0x4000000.into(), // 64MiB
                            },
                            Partition {
                                id: "boot-a1".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: 0x20000000.into(), // 512MiB
                            },
                            Partition {
                                id: "boot-b1".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: 0x20000000.into(), // 512MiB
                            },
                            Partition {
                                id: "root-data-a1".to_string(),
                                partition_type: PartitionType::Root,
                                size: 0x100000000.into(), // 4GiB
                            },
                            Partition {
                                id: "root-data-b1".to_string(),
                                partition_type: PartitionType::Root,
                                size: 0x100000000.into(), // 4GiB
                            },
                            Partition {
                                id: "root-hash-a1".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: 0x19000000.into(), // 400MiB
                            },
                            Partition {
                                id: "root-hash-b1".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: 0x19000000.into(), // 400MiB
                            },
                            Partition {
                                id: "swap1".to_string(),
                                partition_type: PartitionType::Swap,
                                size: 0x80000000.into(), // 2GiB
                            },
                            Partition {
                                id: "trident1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x8000000.into(), // 128MiB
                            },
                            Partition {
                                id: "trident-overlay-a1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x8000000.into(), // 128MiB
                            },
                            Partition {
                                id: "trident-overlay-b1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x8000000.into(), // 128MiB
                            },
                            Partition {
                                id: "var-a1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x40000000.into(), // 1GiB
                            },
                            Partition {
                                id: "var-b1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x40000000.into(), // 1GiB
                            },
                            Partition {
                                id: "enc-home1".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x40000000.into(), // 1GiB
                            },
                        ],
                        ..Default::default()
                    },
                    Disk {
                        id: "disk2".to_string(),
                        device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-3".into(),
                        partition_table_type: PartitionTableType::Gpt,
                        partitions: vec![
                            Partition {
                                id: "esp2".to_string(),
                                partition_type: PartitionType::Esp,
                                size: 0x4000000.into(), // 64MiB
                            },
                            Partition {
                                id: "boot-a2".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: 0x20000000.into(), // 512MiB
                            },
                            Partition {
                                id: "boot-b2".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: 0x20000000.into(), // 512MiB
                            },
                            Partition {
                                id: "root-data-a2".to_string(),
                                partition_type: PartitionType::Root,
                                size: 0x100000000.into(), // 4GiB
                            },
                            Partition {
                                id: "root-data-b2".to_string(),
                                partition_type: PartitionType::Root,
                                size: 0x100000000.into(), // 4GiB
                            },
                            Partition {
                                id: "root-hash-a2".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: 0x19000000.into(), // 400MiB
                            },
                            Partition {
                                id: "root-hash-b2".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: 0x19000000.into(), // 400MiB
                            },
                            Partition {
                                id: "swap2".to_string(),
                                partition_type: PartitionType::Swap,
                                size: 0x80000000.into(), // 2GiB
                            },
                            Partition {
                                id: "trident2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x8000000.into(), // 128MiB
                            },
                            Partition {
                                id: "trident-overlay-a2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x8000000.into(), // 128MiB
                            },
                            Partition {
                                id: "trident-overlay-b2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x8000000.into(), // 128MiB
                            },
                            Partition {
                                id: "var-a2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x40000000.into(), // 1GiB
                            },
                            Partition {
                                id: "var-b2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x40000000.into(), // 1GiB
                            },
                            Partition {
                                id: "enc-home2".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 0x40000000.into(), // 1GiB
                            },
                        ],
                        ..Default::default()
                    },
                ],
                raid: Raid {
                    // add 3 minute timeout for syncing
                    sync_timeout: Some(180), // 180 seconds, 3 minutes
                    software: vec![
                        SoftwareRaidArray {
                            id: "boot-a".to_string(),
                            name: "boot-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["boot-a1".to_string(), "boot-a2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "boot-b".to_string(),
                            name: "boot-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["boot-b1".to_string(), "boot-b2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "root-data-a".to_string(),
                            name: "root-data-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root-data-a1".to_string(), "root-data-a2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "root-data-b".to_string(),
                            name: "root-data-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root-data-b1".to_string(), "root-data-b2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "root-hash-a".to_string(),
                            name: "root-hash-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root-hash-a1".to_string(), "root-hash-a2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "root-hash-b".to_string(),
                            name: "root-hash-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root-hash-b1".to_string(), "root-hash-b2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "trident".to_string(),
                            name: "trident".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["trident1".to_string(), "trident2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "trident-overlay-a".to_string(),
                            name: "trident-overlay-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec![
                                "trident-overlay-a1".to_string(),
                                "trident-overlay-a2".to_string(),
                            ],
                        },
                        SoftwareRaidArray {
                            id: "trident-overlay-b".to_string(),
                            name: "trident-overlay-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec![
                                "trident-overlay-b1".to_string(),
                                "trident-overlay-b2".to_string(),
                            ],
                        },
                        SoftwareRaidArray {
                            id: "var-a".to_string(),
                            name: "var-a".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["var-a1".to_string(), "var-a2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "var-b".to_string(),
                            name: "var-b".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["var-b1".to_string(), "var-b2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "enc-home".to_string(),
                            name: "home".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["enc-home1".to_string(), "enc-home2".to_string()],
                        },
                    ],
                },
                encryption: Some(Encryption {
                    recovery_key_url: Some(Url::parse("file:///recovery.key").unwrap()),
                    volumes: vec![EncryptedVolume {
                        id: "home".to_string(),
                        device_name: "home".to_string(),
                        device_id: "enc-home".to_string(),
                    }],
                    pcrs: vec![Pcr::Pcr7],
                    ..Default::default()
                }),
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![
                        AbVolumePair {
                            id: "boot".into(),
                            volume_a_id: "boot-a".into(),
                            volume_b_id: "boot-b".into(),
                        },
                        AbVolumePair {
                            id: "root-data".into(),
                            volume_a_id: "root-data-a".into(),
                            volume_b_id: "root-data-b".into(),
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
                    ],
                }),
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp1".into()),
                        mount_point: Some(MountPoint {
                            path: constants::ESP_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new("umask=0077"),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("esp2".into()),
                        mount_point: None,
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("boot".into()),
                        mount_point: Some(MountPoint {
                            path: constants::BOOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident-overlay".into()),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                        mount_point: Some(MountPoint {
                            path: "/var/lib/trident-overlay".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("var".into()),
                        source: FileSystemSource::Image,
                        mount_point: Some(MountPoint {
                            path: "/var".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("home".into()),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                        mount_point: Some(MountPoint {
                            path: "/home".into(),
                            options: MountOptions::defaults(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        source: FileSystemSource::Image,
                        mount_point: Some(MountPoint {
                            path: ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
                        }),
                    }
                ],
                verity: vec![VerityDevice {
                    id: "root".into(),
                    data_device_id: "root-data".into(),
                    hash_device_id: "root-hash".into(),
                    name: "root".into(),
                    ..Default::default()
                }],
                swap: vec![
                    Swap {
                        device_id: "swap1".into(),
                    },
                    Swap {
                        device_id: "swap2".into(),
                    },
                ]
            },
            os: Os {
                users: vec![User {
                    name: "my-custom-user".into(),
                    ssh_public_keys: vec!["<MY_PUBLIC_SSH_KEY>".into()],
                    ssh_mode: SshMode::KeyOnly,
                    ..Default::default()
                }],
                netplan: Some(NetworkConfig {
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
                    run_on: vec![ServicingTypeSelection::All],
                    source: ScriptSource::Content("mkdir -p /var/lib/trident-overlay/etc-rw/upper && mkdir -p /var/lib/trident-overlay/etc-rw/work".into()),
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
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/image.cosi").unwrap(),
                    sha384: ImageSha384::Checksum(SAMPLE_SHA384.into()),
                }),
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
                                    size: 0x4000000.into(), // 64MiB
                                },
                                Partition {
                                    id: "root1".to_string(),
                                    partition_type: PartitionType::Root,
                                    size: 0x100000000.into(), // 4GiB
                                },
                                Partition {
                                    id: "swap1".to_string(),
                                    partition_type: PartitionType::Swap,
                                    size: 0x80000000.into(), // 2GiB
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
                                    size: 0x4000000.into(), // 64MiB
                                },
                                Partition {
                                    id: "root2".to_string(),
                                    partition_type: PartitionType::Root,
                                    size: 0x100000000.into(), // 4GiB
                                },
                                Partition {
                                    id: "swap2".to_string(),
                                    partition_type: PartitionType::Swap,
                                    size: 0x80000000.into(), // 2GiB
                                },
                            ],
                            adopted_partitions: vec![],
                        },
                    ],
                    raid: Raid {
                        sync_timeout: Some(180), // 180 seconds, 3 minutes
                        software: vec![SoftwareRaidArray {
                            id: "root".to_string(),
                            name: "root".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root1".to_string(), "root2".to_string()],
                        }],
                    },
                    encryption: None,
                    ab_update: None,
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("esp1".into()),
                            mount_point: Some(MountPoint {
                                path: constants::ESP_MOUNT_POINT_PATH.into(),
                                options: MountOptions::new("umask=0077"),
                            }),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            mount_point: Some(MountPoint {
                                path: constants::ROOT_MOUNT_POINT_PATH.into(),
                                options: MountOptions::defaults(),
                            }),
                            source: FileSystemSource::Image,
                        },
                    ],
                    swap: vec![Swap {
                        device_id: "swap1".into(),
                    }, Swap {
                        device_id: "swap2".into(),
                    }],
                    ..Default::default()
                },
                os: Os {
                    selinux: Selinux {
                        mode: Some(SelinuxMode::Permissive),
                    },
                    users: vec![User {
                        name: "my-custom-user".into(),
                        ssh_public_keys: vec!["<MY_PUBLIC_SSH_KEY>".into()],
                        ssh_mode: SshMode::KeyOnly,
                        ..Default::default()
                    }],
                    netplan: Some(NetworkConfig {
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
                    modules: vec![],
                    services: Services {
                        enable: vec![],
                        disable: vec![],
                    },
                    kernel_command_line: KernelCommandLine {
                        extra_command_line: vec![],
                    },
                    sysexts: vec![],
                    confexts: vec![],
                    uefi_fallback: None,
                },
                scripts: Scripts {
                    post_configure: vec![Script {
                        name: "wheel".into(),
                        run_on: vec![ServicingTypeSelection::CleanInstall, ServicingTypeSelection::AbUpdate],
                        source: ScriptSource::Content(
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
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/image.cosi").unwrap(),
                    sha384: ImageSha384::Checksum(SAMPLE_SHA384.into()),
                }),
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
                                    size: 0x4000000.into(), // 64MiB
                                },
                                Partition {
                                    id: "root".to_string(),
                                    partition_type: PartitionType::Root,
                                    size: 0x100000000.into(), // 4GiB
                                },
                                Partition {
                                    id: "swap".to_string(),
                                    partition_type: PartitionType::Swap,
                                    size: 0x80000000.into(), // 2GiB
                                },
                                Partition {
                                    id: "luks-srv".to_string(),
                                    partition_type: PartitionType::LinuxGeneric,
                                    size: 0x4000000.into(), // 64MiB
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
                    raid: Raid { software: vec![], sync_timeout: None },
                    encryption: Some(Encryption {
                        recovery_key_url: None,
                        volumes: vec![EncryptedVolume {
                            id: "srv".to_string(),
                            device_name: "srv".to_string(),
                            device_id: "luks-srv".to_string(),
                        }],
                        pcrs: vec![Pcr::Pcr7],
                        ..Default::default()
                    }),
                    ab_update: None,
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("esp".into()),
                            mount_point: Some(MountPoint {
                                path: constants::ESP_MOUNT_POINT_PATH.into(),
                                options: MountOptions::new("umask=0077"),
                            }),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            mount_point: Some(MountPoint {
                                path: constants::ROOT_MOUNT_POINT_PATH.into(),
                                options: MountOptions::defaults(),
                            }),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            device_id: Some("srv".into()),
                            mount_point: Some(MountPoint {
                                path: "/srv".into(),
                                options: MountOptions::defaults(),
                            }),
                            source: FileSystemSource::New(NewFileSystemType::Ext4),
                        },
                    ],
                    swap: vec![Swap {
                        device_id: "swap".into(),
                    }],
                    ..Default::default()
                },
                os: Os {
                    selinux: Selinux {
                        mode: Some(SelinuxMode::Permissive),
                    },
                    users: vec![User {
                        name: "my-custom-user".into(),
                        ssh_public_keys: vec!["<MY_PUBLIC_SSH_KEY>".into()],
                        ssh_mode: SshMode::KeyOnly,
                        ..Default::default()
                    }],
                    netplan: Some(NetworkConfig {
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
                    modules: vec![],
                    services: Services {
                        enable: vec![],
                        disable: vec![],
                    },
                    kernel_command_line: KernelCommandLine {
                        extra_command_line: vec![],
                    },
                    sysexts: vec![],
                    confexts: vec![],
                    uefi_fallback: None,
                },
                scripts: Scripts {
                    post_configure: vec![Script {
                        name: "wheel".into(),
                        run_on: vec![ServicingTypeSelection::CleanInstall, ServicingTypeSelection::AbUpdate],
                        source: ScriptSource::Content(
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
        "raid-mirrored" => (
            "Example of RAID mirroring demonstrating the use of RAID1 on ESP, root, and Trident.",
            HostConfiguration {
                image: Some(OsImage {
                    url: Url::parse("file:///path/to/image.cosi").unwrap(),
                    sha384: ImageSha384::Checksum(SAMPLE_SHA384.into()),
                }),
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
                                    size: 0x4000000.into(), // 64MiB
                                },
                                Partition {
                                    id: "root1".to_string(),
                                    partition_type: PartitionType::Root,
                                    size: 0x100000000.into(), // 4GiB
                                },
                                Partition {
                                    id: "trident1".to_string(),
                                    partition_type: PartitionType::LinuxGeneric,
                                    size: 0x8000000.into(), // 1GiB
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
                                    size: 0x4000000.into(), // 64MiB
                                },
                                Partition {
                                    id: "root2".to_string(),
                                    partition_type: PartitionType::Root,
                                    size: 0x100000000.into(), // 4GiB
                                },
                                Partition {
                                    id: "trident2".to_string(),
                                    partition_type: PartitionType::LinuxGeneric,
                                    size: 0x8000000.into(), // 1GiB
                                },
                            ],
                            adopted_partitions: vec![],
                        },
                    ],
                    raid: Raid {
                        sync_timeout: Some(180), // 180 seconds, 3 minutes
                        software: vec![ SoftwareRaidArray {
                                id: "esp".to_string(),
                                name: "esp".to_string(),
                                level: RaidLevel::Raid1,
                                devices: vec!["esp1".to_string(), "esp2".to_string()],
                            },
                            SoftwareRaidArray {
                            id: "root".to_string(),
                            name: "root".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["root1".to_string(), "root2".to_string()],
                        },
                        SoftwareRaidArray {
                            id: "trident".to_string(),
                            name: "trident".to_string(),
                            level: RaidLevel::Raid1,
                            devices: vec!["trident1".to_string(), "trident2".to_string()],
                        }],
                    },
                    encryption: None,
                    ab_update: None,
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("esp".into()),
                            mount_point: Some(MountPoint {
                                path: constants::ESP_MOUNT_POINT_PATH.into(),
                                options: MountOptions::new("umask=0077"),
                            }),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            mount_point: Some(MountPoint {
                                path: constants::ROOT_MOUNT_POINT_PATH.into(),
                                options: MountOptions::defaults(),
                            }),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            device_id: Some("trident".into()),
                            mount_point: Some(MountPoint {
                                path: "/var/lib/trident".into(),
                                options: MountOptions::defaults(),
                            }),
                            source: FileSystemSource::New(NewFileSystemType::Ext4),
                        },
                    ],
                    ..Default::default()
                },
                os: Os {
                    selinux: Selinux {
                        mode: Some(SelinuxMode::Permissive),
                    },
                    users: vec![User {
                        name: "my-custom-user".into(),
                        ssh_public_keys: vec!["<MY_PUBLIC_SSH_KEY>".into()],
                        ssh_mode: SshMode::KeyOnly,
                        ..Default::default()
                    }],
                    netplan: Some(NetworkConfig {
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
                    modules: vec![],
                    services: Services {
                        enable: vec![],
                        disable: vec![],
                    },
                    kernel_command_line: KernelCommandLine {
                        extra_command_line: vec![],
                    },
                    sysexts: vec![],
                    confexts: vec![],
                    uefi_fallback: None,
                },
                scripts: Scripts {
                    post_configure: vec![Script {
                        name: "wheel".into(),
                        run_on: vec![ServicingTypeSelection::CleanInstall, ServicingTypeSelection::AbUpdate],
                        source: ScriptSource::Content(
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

    /// This test is mostly to ensure that we try to build a Host Configuration
    /// and fail if the build fails to let us know that the sample is out of date.
    #[test]
    fn test_build_basic_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("basic").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);
        assert!(&host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.filesystems.len(), 2);
        assert_eq!(host_configuration.storage.verity.len(), 0);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.netplan.is_none());
        assert_eq!(host_configuration.os.users.len(), 0);
    }

    #[test]
    fn test_build_simple_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("simple").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);
        assert!(&host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.filesystems.len(), 2);
        assert_eq!(host_configuration.storage.verity.len(), 0);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.netplan.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_base_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("base").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);

        assert!(host_configuration.storage.encryption.is_some());
        if let Some(encryption) = &host_configuration.storage.encryption {
            assert_eq!(encryption.volumes.len(), 1);
        }

        assert_eq!(host_configuration.storage.raid.software.len(), 1);
        assert_eq!(host_configuration.storage.filesystems.len(), 5);
        assert_eq!(host_configuration.storage.verity.len(), 0);
        assert!(host_configuration.storage.ab_update.is_some());
        assert!(host_configuration.os.netplan.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_verity_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("verity").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 1);
        assert!(host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 0);
        assert_eq!(host_configuration.storage.filesystems.len(), 7);
        assert_eq!(host_configuration.storage.verity.len(), 1);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.netplan.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_advanced_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("advanced").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 2);

        assert!(host_configuration.storage.encryption.is_some());
        if let Some(encryption) = &host_configuration.storage.encryption {
            assert_eq!(encryption.volumes.len(), 1);
        }

        assert_eq!(host_configuration.storage.raid.software.len(), 12);
        assert_eq!(host_configuration.storage.filesystems.len(), 8);
        assert_eq!(host_configuration.storage.verity.len(), 1);
        assert!(host_configuration.storage.ab_update.is_some());
        assert!(host_configuration.os.netplan.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_raid_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("raid").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 2);

        assert!(host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 1);
        assert_eq!(host_configuration.storage.filesystems.len(), 2);
        assert_eq!(host_configuration.storage.verity.len(), 0);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.netplan.is_some());
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
        assert_eq!(host_configuration.storage.filesystems.len(), 3);
        assert_eq!(host_configuration.storage.verity.len(), 0);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.netplan.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }

    #[test]
    fn test_build_raid_mirrored_host_configuration() {
        let (_, host_configuration) = sample_host_configuration("raid-mirrored").unwrap();
        host_configuration.validate().unwrap();
        assert_eq!(host_configuration.storage.disks.len(), 2);

        assert!(host_configuration.storage.encryption.is_none());
        assert_eq!(host_configuration.storage.raid.software.len(), 3);
        assert_eq!(host_configuration.storage.filesystems.len(), 3);
        assert_eq!(host_configuration.storage.verity.len(), 0);
        assert!(host_configuration.storage.ab_update.is_none());
        assert!(host_configuration.os.netplan.is_some());
        assert_eq!(host_configuration.os.users.len(), 1);
    }
}
