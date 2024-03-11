use std::collections::HashMap;
use uuid::Uuid;

use netplan_types::{
    CommonPropertiesAllDevices, CommonPropertiesPhysicalDeviceType, EthernetConfig, MatchConfig,
    NetworkConfig,
};

use crate::{
    config::{
        AbUpdate, AbVolumePair, AdditionalFile, AdoptedPartition, Disk, EncryptedVolume,
        Encryption, HostConfiguration, Image, ImageFormat, ImageSha256, MountPoint, Os, Partition,
        PartitionSize, PartitionTableType, PartitionType, Raid, RaidLevel, Script, Scripts,
        ServicingType, SoftwareRaidArray, SshMode, Storage, User,
    },
    constants,
};
use url::Url;

pub fn sample_host_configuration() -> HostConfiguration {
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
                        size: PartitionSize::Fixed(0x40000000), // 1GiB
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
                        size: PartitionSize::Fixed(0x40000000), // 1GiB
                    },
                    Partition {
                        id: "enc-srv".to_string(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::Fixed(0x40000000), // 1GiB
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
                adopted_partitions: vec![AdoptedPartition {
                    id: "root-a".to_string(),
                    uuid: Some(Uuid::parse_str("a0a0a0a0-a0a0-a0a0-a0a0-a0a0a0a0a0a0").unwrap()),
                    ..Default::default()
                }],
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
            mount_points: vec![
                MountPoint {
                    path: "/boot/efi".into(),
                    target_id: "esp".into(),
                    filesystem: "vfat".into(),
                    options: vec!["umask=0077".into()],
                },
                MountPoint {
                    path: constants::ROOT_MOUNT_POINT_PATH.into(),
                    target_id: "root".into(),
                    filesystem: "ext4".into(),
                    options: vec!["defaults".into()],
                },
                MountPoint {
                    path: "/var/lib/trident".into(),
                    target_id: "trident".into(),
                    filesystem: "ext4".into(),
                    options: vec!["defaults".into()],
                },
                MountPoint {
                    path: "none".into(),
                    target_id: "swap".into(),
                    filesystem: "swap".into(),
                    options: vec!["sw".into()],
                },
                MountPoint {
                    path: "/srv".into(),
                    target_id: "srv".into(),
                    filesystem: "ext4".into(),
                    options: vec!["defaults".into()],
                },
                MountPoint {
                    path: "/mnt/raid".into(),
                    target_id: "some_raid".into(),
                    filesystem: "ext4".into(),
                    options: vec!["defaults".into()],
                },
            ],
            images: vec![
                Image {
                    url: "file:///trident_cdrom/data/esp.rawzst".into(),
                    sha256: ImageSha256::Checksum(
                        "e15853875ce26f8fb8090177821240a889e21ac0c5acee75c5a060401bbdf0ae".into(),
                    ),
                    format: ImageFormat::RawZst,
                    target_id: "esp".into(),
                },
                Image {
                    url: "file:///trident_cdrom/data/root.rawzst".into(),
                    sha256: ImageSha256::Checksum(
                        "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293".into(),
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
                    "vmeths".into(),
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
                content: Some("echo 'Hello, world!'".into()),
                ..Default::default()
            }],
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This test is mostly to ensure that we try to build a host configuration
    /// and fail if the build fails to let us know that the sample is out of date.
    #[test]
    fn test_build_host_configuration() {
        let host_configuration = sample_host_configuration();
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
    }
}
