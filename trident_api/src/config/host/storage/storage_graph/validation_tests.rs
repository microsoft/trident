//! A module for testing the storage graph validation.

use crate::{
    config::{
        AbVolumePair, AdoptedPartition, Disk, EncryptedVolume, FileSystem, FileSystemSource,
        FileSystemType, Image, ImageFormat, ImageSha256, MountOptions, MountPoint, Partition,
        PartitionSize, PartitionTableType, PartitionType, RaidLevel, SoftwareRaidArray,
    },
    constants,
};

use super::{
    builder::StorageGraphBuilder,
    error::StorageGraphBuildError,
    node::StorageGraphNode,
    types::{BlkDevReferrerKind, HostConfigBlockDevice},
};

#[test]
fn test_basic_graph() {
    let mut builder = StorageGraphBuilder::default();
    let mut nodes = Vec::new();

    let disk = Disk {
        id: "disk".into(),
        device: "/dev/sda".into(),
        partition_table_type: PartitionTableType::Gpt,
        ..Default::default()
    };
    builder.add_node((&disk).into());
    nodes.push(StorageGraphNode::from(&disk));

    let partitions = (1..=6)
        .map(|i| Partition {
            id: format!("partition{}", i),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::LinuxGeneric,
        })
        .collect::<Vec<_>>();
    partitions.iter().for_each(|p| builder.add_node(p.into()));
    partitions.iter().for_each(|p| {
        nodes.push(StorageGraphNode::from(p));
    });

    let raid_array = SoftwareRaidArray {
        id: "raid_array".into(),
        name: "md0".into(),
        devices: vec!["partition1".into(), "partition2".into()],
        level: RaidLevel::Raid1,
    };
    builder.add_node((&raid_array).into());
    nodes.push(StorageGraphNode::from(&raid_array));

    let ab_volume_pair = AbVolumePair {
        id: "ab_volume_pair".into(),
        volume_a_id: "partition3".into(),
        volume_b_id: "partition4".into(),
    };
    builder.add_node((&ab_volume_pair).into());
    nodes.push(StorageGraphNode::from(&ab_volume_pair));

    let encrypted_volume = EncryptedVolume {
        id: "encrypted_volume".into(),
        device_id: "partition5".into(),
        device_name: "encrypted_volume".into(),
    };
    builder.add_node((&encrypted_volume).into());
    nodes.push(StorageGraphNode::from(&encrypted_volume));

    let fs = FileSystem {
        device_id: Some("partition6".into()),
        fs_type: FileSystemType::Ext4,
        source: FileSystemSource::Image(Image {
            url: "http://image".into(),
            sha256: ImageSha256::Checksum("checksum".into()),
            format: ImageFormat::RawZst,
        }),
        mount_point: Some(MountPoint {
            path: constants::ROOT_MOUNT_POINT_PATH.into(),
            options: MountOptions::empty(),
        }),
    };
    builder.add_node((&fs).into());
    nodes.push(StorageGraphNode::from(&fs));

    let mut graph = builder.build().unwrap();

    // Check that all nodes were successfully added
    assert!(nodes.len() == graph.inner.node_count());
    for index in graph.inner.node_indices().rev() {
        let removed = graph.inner.remove_node(index);
        assert!(removed.unwrap() == nodes.pop().unwrap());
    }
}

#[test]
fn test_duplicate_node() {
    let mut builder = StorageGraphBuilder::default();

    let disk = Disk {
        id: "disk".into(),
        device: "/dev/sda".into(),
        partition_table_type: PartitionTableType::Gpt,
        ..Default::default()
    };
    builder.add_node((&disk).into());
    builder.add_node((&disk).into());

    matches!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::DuplicateDeviceId { .. }
    );
}

#[test]
fn test_duplicate_member() {
    let partition = Partition {
        id: "partition".into(),
        size: PartitionSize::Fixed(4096.into()),
        partition_type: PartitionType::Esp,
    };

    // Duplicate member in A/B volume
    let mut builder = StorageGraphBuilder::default();
    builder.add_node((&partition).into());

    let ab_volume_pair = AbVolumePair {
        id: "ab_volume_pair".into(),
        volume_a_id: "partition".into(),
        volume_b_id: "partition".into(),
    };

    builder.add_node((&ab_volume_pair).into());
    matches!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::DuplicateTargetId { .. }
    );

    // Duplicate member in RAID volume
    let mut builder = StorageGraphBuilder::default();
    builder.add_node((&partition).into());

    let raid_array = SoftwareRaidArray {
        id: "raid_array".into(),
        name: "md0".into(),
        devices: vec!["partition".into(), "partition".into()],
        level: RaidLevel::Raid1,
    };

    builder.add_node((&raid_array).into());

    matches!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::DuplicateTargetId { .. }
    );
}

#[test]
fn test_member_validity() {
    let mut builder = StorageGraphBuilder::default();

    let disk = Disk {
        id: "disk".into(),
        device: "/dev/sda".into(),
        partition_table_type: PartitionTableType::Gpt,
        ..Default::default()
    };
    builder.add_node((&disk).into());

    let partition = Partition {
        id: "partition".into(),
        size: PartitionSize::Fixed(4096.into()),
        partition_type: PartitionType::Esp,
    };
    builder.add_node((&partition).into());

    let ab_volume_pair = AbVolumePair {
        id: "ab_volume_pair".into(),
        volume_a_id: "disk".into(),
        volume_b_id: "partition".into(),
    };
    builder.add_node((&ab_volume_pair).into());

    matches!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::InvalidReferenceKind { .. }
    );
}

#[test]
fn test_cardinality() {
    // Test zero cardinalities
    let zero_cardinalities = [
        // Create default instances of all zero cardinality items (the contents of the devices do
        // not matter for this test)
        HostConfigBlockDevice::Disk(Disk::default()),
        HostConfigBlockDevice::Partition(Partition {
            id: "partition".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::LinuxGeneric,
        }),
        HostConfigBlockDevice::AdoptedPartition(AdoptedPartition {
            id: "adopted_partition".into(),
            match_label: None,
            match_uuid: None,
        }),
    ];
    for item in zero_cardinalities.iter() {
        let cardinality = item.referrer_kind().valid_target_count();
        assert!(cardinality.contains(0), "{item:?} should contain 0");
        assert_eq!(
            cardinality.min().unwrap(),
            0,
            "{item:?} cardinality start should be 0"
        );
        assert_eq!(
            cardinality.max().unwrap(),
            0,
            "{item:?} cardinality end should be 0"
        );
    }

    // Test RaidArray cardinality
    let raid_cardinality = BlkDevReferrerKind::RaidArray.valid_target_count();
    assert!(
        !raid_cardinality.contains(0),
        "RaidArray should not contain 0"
    );
    assert!(
        !raid_cardinality.contains(1),
        "RaidArray should not contain 1"
    );
    assert!(raid_cardinality.contains(2), "RaidArray should contain 2");
    assert!(raid_cardinality.contains(3), "RaidArray should contain 3");
    assert_eq!(
        raid_cardinality.min().unwrap(),
        2,
        "RaidArray cardinality start should be 2"
    );
    assert!(
        raid_cardinality.max().is_none(),
        "RaidArray cardinality end should be none"
    );

    // Test ABVolume cardinality
    let ab_volume_cardinality = BlkDevReferrerKind::ABVolume.valid_target_count();
    assert!(
        !ab_volume_cardinality.contains(1),
        "ABVolume should not contain 1"
    );
    assert!(
        ab_volume_cardinality.contains(2),
        "ABVolume should contain 2"
    );
    assert!(
        !ab_volume_cardinality.contains(3),
        "ABVolume should not contain 3"
    );
    assert_eq!(
        ab_volume_cardinality.min().unwrap(),
        2,
        "ABVolume cardinality start should be 2"
    );
    assert_eq!(
        ab_volume_cardinality.max().unwrap(),
        2,
        "ABVolume cardinality end should be 2"
    );

    // Test EncryptedVolume cardinality
    let encrypted_volume_cardinality = BlkDevReferrerKind::EncryptedVolume.valid_target_count();
    assert!(
        !encrypted_volume_cardinality.contains(0),
        "EncryptedVolume should not contain 0"
    );
    assert!(
        encrypted_volume_cardinality.contains(1),
        "EncryptedVolume should contain 1"
    );
    assert!(
        !encrypted_volume_cardinality.contains(2),
        "EncryptedVolume should not contain 2"
    );
    assert_eq!(
        encrypted_volume_cardinality.min().unwrap(),
        1,
        "EncryptedVolume cardinality start should be 1"
    );
    assert_eq!(
        encrypted_volume_cardinality.max().unwrap(),
        1,
        "EncryptedVolume cardinality end should be 1"
    );
}

#[test]
fn valid_target_count() {
    let partition1 = Partition {
        id: "partition1".into(),
        size: PartitionSize::Fixed(4096.into()),
        partition_type: PartitionType::LinuxGeneric,
    };

    let partition2 = Partition {
        id: "partition2".into(),
        size: PartitionSize::Fixed(4096.into()),
        partition_type: PartitionType::LinuxGeneric,
    };

    let mut base_builder = StorageGraphBuilder::default();
    base_builder.add_node((&partition1).into());
    base_builder.add_node((&partition2).into());

    // Should be valid
    let raid_ok = SoftwareRaidArray {
        id: "raid_array".into(),
        name: "md0".into(),
        devices: vec!["partition1".into(), "partition2".into()],
        level: RaidLevel::Raid1,
    };

    let mut builder = base_builder.clone();
    builder.add_node((&raid_ok).into());

    builder.build().unwrap();

    // Should not be ok
    let raid_empty = SoftwareRaidArray {
        id: "raid_array".into(),
        name: "md0".into(),
        devices: vec![],
        level: RaidLevel::Raid1,
    };

    let mut builder = base_builder.clone();
    builder.add_node((&raid_empty).into());

    matches!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::InvalidTargetCount { .. }
    );

    // Should not be ok
    let raid_single = SoftwareRaidArray {
        id: "raid_array".into(),
        name: "md0".into(),
        devices: vec!["partition1".into()],
        level: RaidLevel::Raid1,
    };

    let mut builder = base_builder.clone();
    builder.add_node((&raid_single).into());

    matches!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::InvalidTargetCount { .. }
    );
}

#[test]
fn test_invalid_sizes() {
    let base_builder = StorageGraphBuilder::default();

    let partition1 = Partition {
        id: "partition1".into(),
        size: PartitionSize::Fixed(2048.into()),
        partition_type: PartitionType::LinuxGeneric,
    };
    let mut builder = base_builder.clone();
    builder.add_node((&partition1).into());

    matches!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::BasicCheckFailed { .. }
    );

    let partition2 = Partition {
        id: "partition2".into(),
        size: PartitionSize::Fixed(5032.into()),
        partition_type: PartitionType::LinuxGeneric,
    };
    let mut builder = base_builder.clone();
    builder.add_node((&partition2).into());

    matches!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::BasicCheckFailed { .. }
    );

    let partition_zero = Partition {
        id: "partition_zero".into(),
        size: PartitionSize::Fixed(0.into()),
        partition_type: PartitionType::LinuxGeneric,
    };
    let mut builder = base_builder.clone();
    builder.add_node((&partition_zero).into());

    matches!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::BasicCheckFailed { .. }
    );
}

mod verity {
    use super::*;

    use crate::{config::VerityFileSystem, storage_graph::references::SpecialReferenceKind};

    #[test]
    fn test_verity_homogeneous_targets() {
        let mut builder = StorageGraphBuilder::default();

        let part1 = Partition {
            id: "part1".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::Root,
        };
        builder.add_node((&part1).into());

        let part2 = Partition {
            id: "part2".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::RootVerity,
        };
        builder.add_node((&part2).into());

        let vfs = VerityFileSystem {
            data_device_id: "part1".into(),
            hash_device_id: "part2".into(),
            fs_type: FileSystemType::Ext4,
            data_image: Image {
                url: "http://image".into(),
                sha256: ImageSha256::Checksum("checksum".into()),
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://image".into(),
                sha256: ImageSha256::Checksum("checksum".into()),
                format: ImageFormat::RawZst,
            },
            name: "verity".into(),
            mount_point: MountPoint {
                path: constants::ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::empty(),
            },
        };
        builder.add_node((&vfs).into());

        builder.build().unwrap();
    }

    #[test]
    fn test_verity_heterogeneous_targets_fail() {
        let mut builder = StorageGraphBuilder::default();

        let part1 = Partition {
            id: "part1".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::Root,
        };
        builder.add_node((&part1).into());

        let part2 = Partition {
            id: "part2".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::Root,
        };
        builder.add_node((&part2).into());

        let part3 = Partition {
            id: "part3".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::RootVerity,
        };
        builder.add_node((&part3).into());

        let raid = SoftwareRaidArray {
            id: "raid".into(),
            name: "md0".into(),
            devices: vec!["part1".into(), "part2".into()],
            level: RaidLevel::Raid1,
        };
        builder.add_node((&raid).into());

        let vfs = VerityFileSystem {
            data_device_id: "raid".into(),
            hash_device_id: "part3".into(),
            fs_type: FileSystemType::Ext4,
            data_image: Image {
                url: "http://image".into(),
                sha256: ImageSha256::Checksum("checksum".into()),
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://image".into(),
                sha256: ImageSha256::Checksum("checksum".into()),
                format: ImageFormat::RawZst,
            },
            name: "verity".into(),
            mount_point: MountPoint {
                path: constants::ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::empty(),
            },
        };
        builder.add_node((&vfs).into());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::ReferenceKindMismatch {
                node_identifier: StorageGraphNode::from(&vfs).identifier(),
                kind: BlkDevReferrerKind::FilesystemVerity,
            }
        );
    }

    #[test]
    fn test_verity_invalid_partition_type_fail() {
        let mut builder = StorageGraphBuilder::default();

        let part1 = Partition {
            id: "part1".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::LinuxGeneric,
        };
        builder.add_node((&part1).into());

        let part2 = Partition {
            id: "part2".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::RootVerity,
        };
        builder.add_node((&part2).into());

        let vfs = VerityFileSystem {
            data_device_id: "part1".into(),
            hash_device_id: "part2".into(),
            fs_type: FileSystemType::Ext4,
            data_image: Image {
                url: "http://image".into(),
                sha256: ImageSha256::Checksum("checksum".into()),
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://image".into(),
                sha256: ImageSha256::Checksum("checksum".into()),
                format: ImageFormat::RawZst,
            },
            name: "verity".into(),
            mount_point: MountPoint {
                path: constants::ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::empty(),
            },
        };
        let vfs_node = StorageGraphNode::from(&vfs);
        builder.add_node(vfs_node.clone());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::InvalidPartitionType {
                node_identifier: vfs_node.identifier(),
                kind: vfs_node.referrer_kind(),
                partition_id: "part1".into(),
                partition_type: PartitionType::LinuxGeneric,
                valid_types: SpecialReferenceKind::VerityDataDevice
                    .allowed_partition_types()
                    .unwrap(),
            }
        );
    }

    #[test]
    fn test_verity_invalid_hash_partition_type_fail() {
        let mut builder = StorageGraphBuilder::default();

        let part1 = Partition {
            id: "part1".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::Root,
        };
        builder.add_node((&part1).into());

        let part2 = Partition {
            id: "part2".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::LinuxGeneric,
        };
        builder.add_node((&part2).into());

        let vfs = VerityFileSystem {
            data_device_id: "part1".into(),
            hash_device_id: "part2".into(),
            fs_type: FileSystemType::Ext4,
            data_image: Image {
                url: "http://image".into(),
                sha256: ImageSha256::Checksum("checksum".into()),
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://image".into(),
                sha256: ImageSha256::Checksum("checksum".into()),
                format: ImageFormat::RawZst,
            },
            name: "verity".into(),
            mount_point: MountPoint {
                path: constants::ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::empty(),
            },
        };
        let vfs_node = StorageGraphNode::from(&vfs);
        builder.add_node(vfs_node.clone());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::InvalidPartitionType {
                node_identifier: vfs_node.identifier(),
                kind: vfs_node.referrer_kind(),
                partition_id: "part2".into(),
                partition_type: PartitionType::LinuxGeneric,
                valid_types: SpecialReferenceKind::VerityHashDevice
                    .allowed_partition_types()
                    .unwrap(),
            }
        );
    }
}

mod ab {
    use super::*;

    use crate::config::EncryptedVolume;

    #[test]
    fn test_ab_volume_heterogeneous_references_fail() {
        let mut builder = StorageGraphBuilder::default();

        let part1 = Partition {
            id: "part1".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::Root,
        };
        builder.add_node((&part1).into());

        let part2 = Partition {
            id: "part2".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::Root,
        };
        builder.add_node((&part2).into());

        let enc = EncryptedVolume {
            id: "enc".into(),
            device_id: "part2".into(),
            device_name: "encrypted".into(),
        };
        builder.add_node((&enc).into());

        let ab = AbVolumePair {
            id: "ab".into(),
            volume_a_id: "part1".into(),
            volume_b_id: "enc".into(),
        };
        builder.add_node((&ab).into());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::ReferenceKindMismatch {
                node_identifier: StorageGraphNode::from(&ab).identifier(),
                kind: BlkDevReferrerKind::ABVolume,
            }
        );
    }
}
