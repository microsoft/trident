//! A module for testing the storage graph validation.

// Currently the folloowing error variants are not produceable and therefore are not tested:
// - StorageGraphBuildError::InvalidPartitionTypeSpecial
// - StorageGraphBuildError::InvalidSpecialReferenceKind
// - StorageGraphBuildError::InvalidTargets
// - StorageGraphBuildError::ReferrerForbiddenSharing
// - StorageGraphBuildError::PartitionTypeMismatchSpecial

use std::path::Path;

use super::{
    builder::StorageGraphBuilder,
    error::StorageGraphBuildError,
    node::StorageGraphNode,
    types::{BlkDevReferrerKind, HostConfigBlockDevice},
};

use crate::{
    config::{
        AbVolumePair, AdoptedPartition, Disk, EncryptedVolume, FileSystem, FileSystemSource,
        FileSystemType, Image, ImageFormat, ImageSha256, MountOptions, MountPoint, Partition,
        PartitionSize, PartitionTableType, PartitionType, RaidLevel, SoftwareRaidArray,
    },
    constants::{ESP_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
    storage_graph::{
        containers::ItemList,
        rules::expected_partition_type,
        types::{BlkDevKind, FileSystemSourceKind},
    },
};

// Helper function to create a generic partition used in unit tests
fn generic_partition() -> Partition {
    Partition {
        id: "partition".into(),
        partition_type: PartitionType::LinuxGeneric,
        size: PartitionSize::Fixed(4096.into()),
    }
}

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
            path: ROOT_MOUNT_POINT_PATH.into(),
            options: MountOptions::empty(),
        }),
    };
    builder.add_node((&fs).into());
    nodes.push(StorageGraphNode::from(&fs));

    let mut graph = builder.build().unwrap();

    // Check that all nodes were successfully added
    assert_eq!(nodes.len(), graph.inner.node_count());
    for index in graph.inner.node_indices().rev() {
        let removed = graph.inner.remove_node(index);
        assert_eq!(removed.unwrap(), nodes.pop().unwrap());
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

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::DuplicateDeviceId("disk".to_string())
    );
}

#[test]
fn test_duplicate_mountpoint() {
    let mut builder = StorageGraphBuilder::default();

    let fs1 = FileSystem {
        device_id: Some("partition".into()),
        fs_type: FileSystemType::Ext4,
        source: FileSystemSource::OsImage,
        mount_point: Some(MountPoint {
            path: ROOT_MOUNT_POINT_PATH.into(),
            options: MountOptions::defaults(),
        }),
    };
    builder.add_node((&fs1).into());
    builder.add_node((&fs1).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::DuplicateMountPoint(ROOT_MOUNT_POINT_PATH.to_string())
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

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::DuplicateTargetId {
            node_identifier: StorageGraphNode::from(&ab_volume_pair).identifier(),
            kind: BlkDevReferrerKind::ABVolume,
            target_id: "partition".to_string()
        }
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

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::DuplicateTargetId {
            node_identifier: StorageGraphNode::from(&raid_array).identifier(),
            kind: BlkDevReferrerKind::RaidArray,
            target_id: "partition".to_string()
        }
    );
}

#[test]
fn test_filesystem_incompatible_source() {
    let mut builder_base = StorageGraphBuilder::default();
    let partition = generic_partition();
    builder_base.add_node((&partition).into());

    // Should pass
    let fs1 = FileSystem {
        device_id: Some("partition".into()),
        fs_type: FileSystemType::Ext4,
        source: FileSystemSource::Image(Image {
            url: "http://image".into(),
            sha256: ImageSha256::Checksum("checksum".into()),
            format: ImageFormat::RawZst,
        }),
        mount_point: Some(MountPoint {
            path: ROOT_MOUNT_POINT_PATH.into(),
            options: MountOptions::empty(),
        }),
    };
    let mut builder = builder_base.clone();
    builder.add_node((&fs1).into());
    builder.build().unwrap();

    // Should fail
    // FileSystemType::Other is only compatible with source types Image and OsImage
    let fs2 = FileSystem {
        device_id: Some("partition".into()),
        fs_type: FileSystemType::Other,
        source: FileSystemSource::New,
        mount_point: None,
    };
    let mut builder = builder_base.clone();
    builder.add_node((&fs2).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::FilesystemIncompatibleSource {
            fs_desc: "src:new, type:other, dev:partition".to_string(),
            fs_source: FileSystemSourceKind::New,
            fs_compatible_sources: ItemList(vec![
                FileSystemSourceKind::Image,
                FileSystemSourceKind::OsImage
            ])
        }
    );

    // Should fail
    // FileSystemType::Swap is only compatible with source type Create
    let fs3 = FileSystem {
        device_id: Some("partition".into()),
        fs_type: FileSystemType::Swap,
        source: FileSystemSource::OsImage,
        mount_point: None,
    };
    let mut builder = builder_base.clone();
    builder.add_node((&fs3).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::FilesystemIncompatibleSource {
            fs_desc: "src:os-image, type:swap, dev:partition".into(),
            fs_source: FileSystemSourceKind::OsImage,
            fs_compatible_sources: ItemList(vec![FileSystemSourceKind::New])
        }
    );
}

#[test]
fn test_filesystem_missing_blkdev_id() {
    let builder_base = StorageGraphBuilder::default();

    let fs = FileSystem {
        device_id: None,
        fs_type: FileSystemType::Vfat,
        source: FileSystemSource::OsImage,
        mount_point: Some(MountPoint {
            path: ROOT_MOUNT_POINT_PATH.into(),
            options: MountOptions::empty(),
        }),
    };
    let mut builder = builder_base.clone();
    builder.add_node((&fs).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::FilesystemMissingBlockDeviceId {
            fs_desc: "src:os-image, type:vfat, mnt:/".into()
        },
    );
}

#[test]
fn test_filesystem_missing_mp() {
    // FileSystemType::Tmpfs expects a mount point
    let mut fs = FileSystem {
        device_id: None,
        fs_type: FileSystemType::Tmpfs,
        source: FileSystemSource::New,
        mount_point: None,
    };
    let mut builder = StorageGraphBuilder::default();
    builder.add_node((&fs).into());
    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::FilesystemMissingMountPoint {
            fs_desc: "src:new, type:tmpfs".into()
        }
    );

    // build() should pass after adding mount point
    fs.mount_point = Some(MountPoint {
        path: ROOT_MOUNT_POINT_PATH.into(),
        options: MountOptions::defaults(),
    });
    let mut builder = StorageGraphBuilder::default();
    builder.add_node((&fs).into());
    builder.build().unwrap();
}

#[test]
fn test_unexpected_blkdev_id() {
    let mut builder = StorageGraphBuilder::default();

    let partition = generic_partition();
    builder.add_node((&partition).into());

    // FileSystemType::Tmpfs does not expect device_id
    let fs = FileSystem {
        device_id: Some("partition".into()),
        fs_type: FileSystemType::Tmpfs,
        source: FileSystemSource::New,
        mount_point: Some(MountPoint {
            path: ROOT_MOUNT_POINT_PATH.into(),
            options: MountOptions::defaults(),
        }),
    };
    builder.add_node((&fs).into());
    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::FilesystemUnexpectedBlockDeviceId {
            fs_desc: "src:new, type:tmpfs, dev:partition, mnt:/".into()
        }
    );
}

#[test]
fn test_unexpected_mp() {
    let mut builder = StorageGraphBuilder::default();

    let partition = generic_partition();
    builder.add_node((&partition).into());

    // FileSystemType::Swap should not have a mount point
    let fs = FileSystem {
        device_id: Some("partition".into()),
        fs_type: FileSystemType::Swap,
        source: FileSystemSource::OsImage,
        mount_point: Some(MountPoint {
            path: ROOT_MOUNT_POINT_PATH.into(),
            options: MountOptions::empty(),
        }),
    };
    builder.add_node((&fs).into());
    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::FilesystemUnexpectedMountPoint {
            fs_desc: "src:os-image, type:swap, dev:partition, mnt:/".into(),
            fs_type: FileSystemType::Swap
        }
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

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::InvalidReferenceKind {
            node_identifier: StorageGraphNode::from(&ab_volume_pair).identifier(),
            kind: BlkDevReferrerKind::ABVolume,
            target_id: "disk".into(),
            target_kind: BlkDevKind::Disk,
            valid_references: BlkDevReferrerKind::ABVolume.compatible_kinds()
        }
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
fn test_valid_target_count() {
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

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::InvalidTargetCount {
            node_identifier: StorageGraphNode::from(&raid_empty).identifier(),
            kind: BlkDevReferrerKind::RaidArray,
            target_count: 0_usize,
            expected: BlkDevReferrerKind::RaidArray.valid_target_count()
        }
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

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::InvalidTargetCount {
            node_identifier: StorageGraphNode::from(&raid_single).identifier(),
            kind: BlkDevReferrerKind::RaidArray,
            target_count: 1_usize,
            expected: BlkDevReferrerKind::RaidArray.valid_target_count()
        }
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

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::BasicCheckFailed {
            node_id: "partition1".into(),
            kind: BlkDevKind::Partition,
            body: "Partition size must be a non-zero multiple of 4096 bytes.".into()
        }
    );

    let partition2 = Partition {
        id: "partition2".into(),
        size: PartitionSize::Fixed(5032.into()),
        partition_type: PartitionType::LinuxGeneric,
    };
    let mut builder = base_builder.clone();
    builder.add_node((&partition2).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::BasicCheckFailed {
            node_id: "partition2".into(),
            kind: BlkDevKind::Partition,
            body: "Partition size must be a non-zero multiple of 4096 bytes.".into()
        }
    );

    let partition_zero = Partition {
        id: "partition_zero".into(),
        size: PartitionSize::Fixed(0.into()),
        partition_type: PartitionType::LinuxGeneric,
    };
    let mut builder = base_builder.clone();
    builder.add_node((&partition_zero).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::BasicCheckFailed {
            node_id: "partition_zero".into(),
            kind: BlkDevKind::Partition,
            body: "Partition size must be a non-zero multiple of 4096 bytes.".into()
        }
    );
}

#[test]
fn test_invalid_raid_level() {
    let mut builder = StorageGraphBuilder::default();

    let part1 = Partition {
        id: "partition1".into(),
        partition_type: PartitionType::Esp,
        size: PartitionSize::Fixed(4096.into()),
    };
    builder.add_node((&part1).into());

    let part2 = Partition {
        id: "partition2".into(),
        partition_type: PartitionType::Esp,
        size: PartitionSize::Fixed(4096.into()),
    };
    builder.add_node((&part2).into());

    let raid_array = SoftwareRaidArray {
        id: "raid_array".into(),
        name: "md0".into(),
        devices: vec!["partition1".into(), "partition2".into()],
        level: RaidLevel::Raid5,
    };
    builder.add_node((&raid_array).into());

    // The referrer kind of FileSystemSource::EspImage(_) is BlkDevReferrerKind::FileSystemEsp
    // Any block device with BlkDevReferrerKind::FileSystemEsp, can only refer to a RAID array with
    // raid level 1
    let fs = FileSystem {
        device_id: Some("raid_array".into()),
        fs_type: FileSystemType::Vfat,
        source: FileSystemSource::EspImage(Image {
            url: "http://image".into(),
            sha256: ImageSha256::Checksum("checksum".into()),
            format: ImageFormat::RawZst,
        }),
        mount_point: Some(MountPoint {
            path: ESP_MOUNT_POINT_PATH.into(),
            options: MountOptions::defaults(),
        }),
    };
    builder.add_node((&fs).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::InvalidRaidlevel {
            node_identifier: StorageGraphNode::from(&fs).identifier(),
            kind: BlkDevReferrerKind::FileSystemEsp,
            raid_id: "raid_array".into(),
            raid_level: RaidLevel::Raid5,
            valid_levels: BlkDevReferrerKind::FileSystemEsp
                .allowed_raid_levels()
                .unwrap()
        }
    );
}

#[test]
fn test_mount_point_path_not_absolute() {
    let mut builder = StorageGraphBuilder::default();

    let partition = generic_partition();
    builder.add_node((&partition).into());

    let fs = FileSystem {
        device_id: Some("partition".into()),
        fs_type: FileSystemType::Ext4,
        source: FileSystemSource::OsImage,
        mount_point: Some(MountPoint {
            path: "not/absolute".into(),
            options: MountOptions::defaults(),
        }),
    };
    builder.add_node((&fs).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::MountPointPathNotAbsolute("not/absolute".to_string())
    );
}

#[test]
fn test_nonexistent_ref() {
    let mut builder = StorageGraphBuilder::default();

    let fs = FileSystem {
        device_id: Some("partition".into()),
        fs_type: FileSystemType::Ext4,
        source: FileSystemSource::OsImage,
        mount_point: Some(MountPoint {
            path: ROOT_MOUNT_POINT_PATH.into(),
            options: MountOptions::empty(),
        }),
    };
    builder.add_node((&fs).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::NonExistentReference {
            node_identifier: StorageGraphNode::from(&fs).identifier(),
            kind: BlkDevReferrerKind::FileSystemOsImage,
            target_id: "partition".into()
        }
    );
}

#[test]
fn test_nonexistent_ref_raid() {
    let mut builder = StorageGraphBuilder::default();

    let partition = generic_partition();
    builder.add_node((&partition).into());

    let raid_array = SoftwareRaidArray {
        id: "raid_array".into(),
        name: "md0".into(),
        devices: vec!["partition".into(), "nonexistent-partition".into()],
        level: RaidLevel::Raid1,
    };
    builder.add_node((&raid_array).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::NonExistentReference {
            node_identifier: StorageGraphNode::from(&raid_array).identifier(),
            kind: BlkDevReferrerKind::RaidArray,
            target_id: "nonexistent-partition".into()
        }
    )
}

#[test]
fn test_unique_field_constraint_error() {
    let mut builder = StorageGraphBuilder::default();

    let disk1 = Disk {
        id: "disk1".into(),
        device: "/dev/sda".into(),
        partition_table_type: PartitionTableType::Gpt,
        ..Default::default()
    };
    builder.add_node((&disk1).into());

    let disk2 = Disk {
        id: "disk2".into(),
        device: "/dev/sda".into(),
        partition_table_type: PartitionTableType::Gpt,
        ..Default::default()
    };
    builder.add_node((&disk2).into());

    // Graph build should fail because across all disk nodes, the "device" field must be unique.
    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::UniqueFieldConstraintError {
            node_id: "disk2".into(),
            other_id: "disk1".into(),
            kind: BlkDevKind::Disk,
            field_name: "device".into(),
            value: "/dev/sda".into()
        }
    );
}

#[test]
fn test_esp_enforce_partition_type() {
    // Test success case
    let mut builder = StorageGraphBuilder::default();

    let mut partition = Partition {
        id: "partition".into(),
        size: PartitionSize::Fixed(4096.into()),
        // Correct type for ESP
        partition_type: PartitionType::Esp,
    };
    builder.add_node((&partition).into());

    let fs = FileSystem {
        device_id: Some("partition".into()),
        fs_type: FileSystemType::Vfat,
        source: FileSystemSource::OsImage,
        mount_point: Some(MountPoint {
            path: ESP_MOUNT_POINT_PATH.into(),
            options: MountOptions::defaults(),
        }),
    };
    builder.add_node((&fs).into());
    builder.build().unwrap();

    // Test failure case
    let mut builder = StorageGraphBuilder::default();
    // Incorrect type for ESP
    partition.partition_type = PartitionType::LinuxGeneric;

    builder.add_node((&partition).into());
    builder.add_node((&fs).into());

    assert_eq!(
        builder.build().unwrap_err(),
        StorageGraphBuildError::InvalidPartitionType {
            node_identifier: StorageGraphNode::from(&fs).identifier(),
            kind: BlkDevReferrerKind::FileSystemOsImage,
            partition_id: "partition".into(),
            partition_type: PartitionType::LinuxGeneric,
            valid_types: expected_partition_type(Path::new(ESP_MOUNT_POINT_PATH))
        }
    );
}

mod verity {
    use super::*;

    use crate::{
        config::VerityDevice, constants::MOUNT_OPTION_READ_ONLY,
        storage_graph::references::SpecialReferenceKind,
    };

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

        let verity_dev = VerityDevice {
            id: "verity_dev".into(),
            data_device_id: "part1".into(),
            hash_device_id: "part2".into(),
            name: "verity".into(),
            ..Default::default()
        };
        builder.add_node((&verity_dev).into());

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

        let verity_dev = VerityDevice {
            id: "verity_dev".into(),
            data_device_id: "raid".into(),
            hash_device_id: "part3".into(),
            name: "verity".into(),
            ..Default::default()
        };
        builder.add_node((&verity_dev).into());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::ReferenceKindMismatch {
                node_identifier: StorageGraphNode::from(&verity_dev).identifier(),
                kind: BlkDevReferrerKind::VerityDevice,
            }
        );
    }

    #[test]
    fn test_verity_invalid_partition_type_fail() {
        let mut builder = StorageGraphBuilder::default();

        let part1 = Partition {
            id: "part1".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::Home,
        };
        builder.add_node((&part1).into());

        let part2 = Partition {
            id: "part2".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::RootVerity,
        };
        builder.add_node((&part2).into());

        let verity_dev = VerityDevice {
            id: "verity_dev".into(),
            data_device_id: "part1".into(),
            hash_device_id: "part2".into(),
            name: "verity".into(),
            ..Default::default()
        };
        let vfs_node = StorageGraphNode::from(&verity_dev);
        builder.add_node(vfs_node.clone());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::InvalidPartitionType {
                node_identifier: vfs_node.identifier(),
                kind: vfs_node.referrer_kind(),
                partition_id: "part1".into(),
                partition_type: PartitionType::Home,
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
            partition_type: PartitionType::Usr,
        };
        builder.add_node((&part2).into());

        let verity_dev = VerityDevice {
            id: "verity_dev".into(),
            data_device_id: "part1".into(),
            hash_device_id: "part2".into(),
            name: "verity".into(),
            ..Default::default()
        };
        let vfs_node = StorageGraphNode::from(&verity_dev);
        builder.add_node(vfs_node.clone());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::InvalidPartitionType {
                node_identifier: vfs_node.identifier(),
                kind: vfs_node.referrer_kind(),
                partition_id: "part2".into(),
                partition_type: PartitionType::Usr,
                valid_types: SpecialReferenceKind::VerityHashDevice
                    .allowed_partition_types()
                    .unwrap(),
            }
        );
    }

    #[test]
    fn test_verity_unsupported_fs_type() {
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

        let verity_dev = VerityDevice {
            id: "verity".into(),
            data_device_id: "part1".into(),
            hash_device_id: "part2".into(),
            name: "verity".into(),
            ..Default::default()
        };

        builder.add_node((&verity_dev).into());

        let root_fs = FileSystem {
            device_id: Some("verity".into()),
            fs_type: FileSystemType::Ntfs, // Only Ext4 and Xfs are supported
            source: FileSystemSource::OsImage,
            mount_point: Some(MountPoint {
                path: ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
            }),
        };

        builder.add_node((&root_fs).into());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::FilesystemVerityIncompatible {
                fs_desc: root_fs.description(),
                fs_type: FileSystemType::Ntfs,
            }
        );
    }

    #[test]
    fn test_verity_filesystem_duplicate_name() {
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

        let verity_dev1 = VerityDevice {
            id: "verity1".into(),
            data_device_id: "part1".into(),
            hash_device_id: "part2".into(),
            name: "verity".into(),
            ..Default::default()
        };

        builder.add_node((&verity_dev1).into());

        let part3 = Partition {
            id: "part3".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::Root,
        };
        builder.add_node((&part3).into());

        let part4 = Partition {
            id: "part4".into(),
            size: PartitionSize::Fixed(4096.into()),
            partition_type: PartitionType::RootVerity,
        };
        builder.add_node((&part4).into());

        let verity_dev2 = VerityDevice {
            id: "verity2".into(),
            data_device_id: "part3".into(),
            hash_device_id: "part4".into(),
            name: "verity".into(),
            ..Default::default()
        };

        builder.add_node((&verity_dev2).into());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::UniqueFieldConstraintError {
                node_id: "verity2".into(),
                other_id: "verity1".into(),
                kind: BlkDevKind::VerityDevice,
                field_name: "name".into(),
                value: "verity".into(),
            },
        );
    }

    #[test]
    fn test_verity_nonexistent_ref() {
        let mut builder = StorageGraphBuilder::default();

        let partition = generic_partition();
        builder.add_node((&partition).into());

        let verity_dev = VerityDevice {
            id: "verity".into(),
            data_device_id: "partition".into(),
            hash_device_id: "nonexistent-hash-partition".into(),
            name: "verity".into(),
            ..Default::default()
        };

        builder.add_node((&verity_dev).into());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::NonExistentReference {
                node_identifier: StorageGraphNode::from(&verity_dev).identifier(),
                kind: BlkDevReferrerKind::VerityDevice,
                target_id: "nonexistent-hash-partition".into()
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

    #[test]
    fn test_ab_volume_partition_size_mismatch() {
        let mut builder = StorageGraphBuilder::default();

        let part1 = Partition {
            id: "part1".into(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::Fixed(4096.into()),
        };
        builder.add_node((&part1).into());

        let part2 = Partition {
            id: "part2".into(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::Fixed(8192.into()),
        };
        builder.add_node((&part2).into());

        let ab = AbVolumePair {
            id: "ab".into(),
            volume_a_id: "part1".into(),
            volume_b_id: "part2".into(),
        };
        builder.add_node((&ab).into());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::PartitionSizeMismatch {
                node_identifier: StorageGraphNode::from(&ab).identifier(),
                kind: BlkDevReferrerKind::ABVolume
            }
        );
    }

    #[test]
    fn test_ab_volume_partition_size_grow() {
        let mut builder = StorageGraphBuilder::default();

        let part1 = Partition {
            id: "part1".into(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::Grow,
        };
        builder.add_node((&part1).into());

        let part2 = Partition {
            id: "part2".into(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::Grow,
        };
        builder.add_node((&part2).into());

        let ab = AbVolumePair {
            id: "ab".into(),
            volume_a_id: "part1".into(),
            volume_b_id: "part2".into(),
        };
        builder.add_node((&ab).into());

        // AB Volume pairs expected to have equally sized, fixed volumes
        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::PartitionSizeNotFixed {
                node_identifier: StorageGraphNode::from(&ab).identifier(),
                kind: BlkDevReferrerKind::ABVolume,
                partition_id: "part2".into()
            }
        );
    }

    #[test]
    fn test_ab_volume_partition_type_mismatch() {
        let mut builder = StorageGraphBuilder::default();

        let part1 = Partition {
            id: "part1".into(),
            partition_type: PartitionType::Root,
            size: PartitionSize::Fixed(4096.into()),
        };
        builder.add_node((&part1).into());

        let part2 = Partition {
            id: "part2".into(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::Fixed(4096.into()),
        };
        builder.add_node((&part2).into());

        let ab = AbVolumePair {
            id: "ab".into(),
            volume_a_id: "part1".into(),
            volume_b_id: "part2".into(),
        };
        builder.add_node((&ab).into());

        // AB Volume pairs must have same partition type
        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::PartitionTypeMismatch {
                node_identifier: StorageGraphNode::from(&ab).identifier(),
                kind: BlkDevReferrerKind::ABVolume
            }
        );
    }

    #[test]
    fn test_ab_volume_nonexistent_ref() {
        let mut builder = StorageGraphBuilder::default();

        let partition = generic_partition();
        builder.add_node((&partition).into());

        let ab = AbVolumePair {
            id: "ab".into(),
            volume_a_id: "partition".into(),
            volume_b_id: "nonexistent-partition".into(),
        };
        builder.add_node((&ab).into());

        assert_eq!(
            builder.build().unwrap_err(),
            StorageGraphBuildError::NonExistentReference {
                node_identifier: StorageGraphNode::from(&ab).identifier(),
                kind: BlkDevReferrerKind::ABVolume,
                target_id: "nonexistent-partition".into()
            }
        );
    }
}
