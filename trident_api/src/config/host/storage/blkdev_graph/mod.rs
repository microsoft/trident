//! # Block Device Graph & Builder
//!
//! The purpose of this module is to build a graph of block devices and their
//! relationships. A big part of the building process is validating that the graph
//! is valid.
//!
//! In broad terms, this module is used as follows:
//!
//! 1. Create a `BlockDeviceGraphBuilder` instance.
//! 2. Feed it nodes by converting Host Config Storage objects (eg. `Disk`s,
//!    `Partition`s...) into nodes (`BlkDevNode`).
//!    - This is done with the `From` traits defined in `conversions.rs` and passing
//!      the nodes to the builder with `add_node()`.
//! 3. Call `build()` to get a `BlockDeviceGraph` instance.
//! 4. On success, a valid `BlockDeviceGraph` instance is returned. Otherwise, an
//!    error detailing the issue is returned.
//!
//! Generic rules, such as checking for duplicate IDs, are implemented in the
//! building itself (`builder.rs`). Rules and constrains related to specific node
//! types are placed in the `rules` module.
//!
//! ## Layout
//!
//! ```text
//! trident_api/src/config/host/storage/blkdev_graph
//! ├── builder.rs        # BlockDeviceGraphBuilder & building logic
//! ├── cardinality.rs    # Helper trait for checking cardinality
//! ├── conversions.rs    # From traits for converting Host Config Storage objects into graph objects
//! ├── errors.rs         # Error types
//! ├── graph.rs          # BlockDeviceGraph
//! ├── mod.rs            # This file
//! ├── rules             # Rules for validating the graph
//! │   ├── encrypted.rs  # Helpers for encrypted volumes
//! │   ├── mod.rs        # Rules module
//! │   └── raid.rs       # Helpers for RAID volumes
//! └── types.rs          # Types used by the graph
//! ```
//!

pub(super) mod builder;
pub(super) mod cardinality;
pub(super) mod conversions;
pub mod error;
pub(super) mod graph;
pub(super) mod rules;
pub(super) mod types;

#[cfg(test)]
mod tests {
    use crate::{
        config::{
            AbVolumePair, Disk, EncryptedVolume, Image, ImageFormat, ImageSha256, MountPoint,
            Partition, PartitionSize, PartitionTableType, PartitionType, RaidLevel,
            SoftwareRaidArray,
        },
        constants,
    };

    use super::{
        builder::BlockDeviceGraphBuilder,
        error::BlockDeviceGraphBuildError,
        types::{BlkDevKind, BlkDevReferrerKind},
    };

    #[test]
    fn test_basic_graph() {
        let mut builder = BlockDeviceGraphBuilder::default();

        let disk = Disk {
            id: "disk".into(),
            device: "/dev/sda".into(),
            partition_table_type: PartitionTableType::Gpt,
            ..Default::default()
        };
        builder.add_node((&disk).into());

        let partitions = (1..=6)
            .map(|i| Partition {
                id: format!("partition{}", i),
                size: PartitionSize::Fixed(1024),
                partition_type: PartitionType::LinuxGeneric,
            })
            .collect::<Vec<_>>();
        partitions.iter().for_each(|p| builder.add_node(p.into()));

        let raid_array = SoftwareRaidArray {
            id: "raid_array".into(),
            name: "md0".into(),
            devices: vec!["partition1".into(), "partition2".into()],
            level: RaidLevel::Raid1,
            metadata_version: "1.2".into(),
        };
        builder.add_node((&raid_array).into());

        let ab_volume_pair = AbVolumePair {
            id: "ab_volume_pair".into(),
            volume_a_id: "partition3".into(),
            volume_b_id: "partition4".into(),
        };
        builder.add_node((&ab_volume_pair).into());

        let encrypted_volume = EncryptedVolume {
            id: "encrypted_volume".into(),
            target_id: "partition5".into(),
            device_name: "encrypted_volume".into(),
        };
        builder.add_node((&encrypted_volume).into());

        let image = Image {
            url: "http://image".into(),
            target_id: "partition6".into(),
            sha256: ImageSha256::Checksum("checksum".into()),
            format: ImageFormat::RawZst,
        };
        builder.add_image(&image);

        let mount_point = MountPoint {
            path: constants::ROOT_MOUNT_POINT_PATH.into(),
            target_id: "partition6".into(),
            filesystem: "ext4".into(),
            options: Vec::new(),
        };
        builder.add_mount_point(&mount_point);

        builder.build().unwrap();
    }

    #[test]
    fn test_duplicate_node() {
        let mut builder = BlockDeviceGraphBuilder::default();

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
            BlockDeviceGraphBuildError::DuplicateDeviceId { .. }
        );
    }

    #[test]
    fn test_duplicate_member() {
        let partition = Partition {
            id: "partition".into(),
            size: PartitionSize::Fixed(1024),
            partition_type: PartitionType::Esp,
        };

        // Duplicate member in A/B volume
        let mut builder = BlockDeviceGraphBuilder::default();
        builder.add_node((&partition).into());

        let ab_volume_pair = AbVolumePair {
            id: "ab_volume_pair".into(),
            volume_a_id: "partition".into(),
            volume_b_id: "partition".into(),
        };

        builder.add_node((&ab_volume_pair).into());
        matches!(
            builder.build().unwrap_err(),
            BlockDeviceGraphBuildError::DuplicateTargetId { .. }
        );

        // Duplicate member in RAID volume
        let mut builder = BlockDeviceGraphBuilder::default();
        builder.add_node((&partition).into());

        let raid_array = SoftwareRaidArray {
            id: "raid_array".into(),
            name: "md0".into(),
            devices: vec!["partition".into(), "partition".into()],
            level: RaidLevel::Raid1,
            metadata_version: "1.2".into(),
        };

        builder.add_node((&raid_array).into());

        matches!(
            builder.build().unwrap_err(),
            BlockDeviceGraphBuildError::DuplicateTargetId { .. }
        );
    }

    #[test]
    fn test_member_validity() {
        let mut builder = BlockDeviceGraphBuilder::default();

        let disk = Disk {
            id: "disk".into(),
            device: "/dev/sda".into(),
            partition_table_type: PartitionTableType::Gpt,
            ..Default::default()
        };
        builder.add_node((&disk).into());

        let partition = Partition {
            id: "partition".into(),
            size: PartitionSize::Fixed(1024),
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
            BlockDeviceGraphBuildError::InvalidReferenceKind { .. }
        );
    }

    #[test]
    fn test_cardinality() {
        // Test zero cardinalities
        let zero_cardinalities = [
            BlkDevKind::Disk,
            BlkDevKind::Partition,
            BlkDevKind::AdoptedPartition,
        ];
        for item in zero_cardinalities.iter() {
            let cardinality = item.as_blkdev_referrer().valid_target_count();
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
            size: PartitionSize::Fixed(1024),
            partition_type: PartitionType::LinuxGeneric,
        };

        let partition2 = Partition {
            id: "partition2".into(),
            size: PartitionSize::Fixed(1024),
            partition_type: PartitionType::LinuxGeneric,
        };

        let mut base_builder = BlockDeviceGraphBuilder::default();
        base_builder.add_node((&partition1).into());
        base_builder.add_node((&partition2).into());

        // Should be valid
        let raid_ok = SoftwareRaidArray {
            id: "raid_array".into(),
            name: "md0".into(),
            devices: vec!["partition1".into(), "partition2".into()],
            level: RaidLevel::Raid1,
            metadata_version: "1.2".into(),
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
            metadata_version: "1.2".into(),
        };

        let mut builder = base_builder.clone();
        builder.add_node((&raid_empty).into());

        matches!(
            builder.build().unwrap_err(),
            BlockDeviceGraphBuildError::InvalidTargetCount { .. }
        );

        // Should not be ok
        let raid_single = SoftwareRaidArray {
            id: "raid_array".into(),
            name: "md0".into(),
            devices: vec!["partition1".into()],
            level: RaidLevel::Raid1,
            metadata_version: "1.2".into(),
        };

        let mut builder = base_builder.clone();
        builder.add_node((&raid_single).into());

        matches!(
            builder.build().unwrap_err(),
            BlockDeviceGraphBuildError::InvalidTargetCount { .. }
        );
    }
}
