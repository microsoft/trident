use anyhow::{bail, ensure, Context, Error};

use crate::config::{
    host::storage::blkdev_graph::{graph::BlockDeviceGraph, types::BlkDevNode},
    Partition, PartitionSize,
};

fn check_partition_size_equals(partitions: &[&Partition]) -> Result<(), Error> {
    // Get the size of each partition, while ensuring that all partitions have a fixed size
    let sizes = partitions
        .iter()
        .map(|part| {
            if let PartitionSize::Fixed(size) = part.size {
                Ok(size)
            } else {
                bail!(
                    "RAID array references partition '{}', which does not have a fixed size.",
                    part.id
                );
            }
        })
        .collect::<Result<Vec<u64>, Error>>()
        .context("Not all members have fixed sizes.")?;

    // Ensure that all partitions have the same size
    //
    // Get the size of the first partition, then ensure that all other partitions have
    // the same size.
    let first_size = *sizes
        .first()
        .context("Failed to get first partition size.")?;

    ensure!(
        sizes.into_iter().all(|size| size == first_size),
        "RAID array references partitions with different sizes."
    );

    Ok(())
}

pub(super) fn check_targets(
    _node: &BlkDevNode,
    targets: &[&BlkDevNode],
    _graph: &BlockDeviceGraph,
) -> Result<(), Error> {
    check_partition_size_equals(
        &targets
            .iter()
            // Assumption: all targets are partitions
            .map(|target| target.host_config_ref.unwrap_partition())
            .collect::<Result<Vec<&Partition>, Error>>()
            .context("Failed to get partitions for RAID array.")?,
    )
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use crate::config::{
        host::storage::blkdev_graph::types::{BlkDevKind, HostConfigBlockDevice},
        PartitionType, RaidLevel, SoftwareRaidArray,
    };

    use super::*;

    #[test]
    fn test_check_partition_size_equals() {
        let partitions = [
            &Partition {
                id: "part1".to_string(),
                size: PartitionSize::Fixed(100),
                partition_type: PartitionType::Root,
            },
            &Partition {
                id: "part2".to_string(),
                size: PartitionSize::Fixed(100),
                partition_type: PartitionType::Root,
            },
        ];
        assert!(check_partition_size_equals(&partitions).is_ok());

        // different sizes
        let partitions = [
            &Partition {
                id: "part1".to_string(),
                size: PartitionSize::Fixed(100),
                partition_type: PartitionType::Root,
            },
            &Partition {
                id: "part2".to_string(),
                size: PartitionSize::Fixed(200),
                partition_type: PartitionType::Root,
            },
        ];
        assert_eq!(
            check_partition_size_equals(&partitions)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "RAID array references partitions with different sizes."
        );

        // grow vs specific size
        let partitions = [
            &Partition {
                id: "part1".to_string(),
                size: PartitionSize::Fixed(100),
                partition_type: PartitionType::Root,
            },
            &Partition {
                id: "part2".to_string(),
                size: PartitionSize::Grow,
                partition_type: PartitionType::Root,
            },
        ];
        assert_eq!(
            check_partition_size_equals(&partitions)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "RAID array references partition 'part2', which does not have a fixed size."
        );

        // grows
        let partitions = [
            &Partition {
                id: "part1".to_string(),
                size: PartitionSize::Grow,
                partition_type: PartitionType::Root,
            },
            &Partition {
                id: "part2".to_string(),
                size: PartitionSize::Grow,
                partition_type: PartitionType::Root,
            },
        ];
        assert_eq!(
            check_partition_size_equals(&partitions)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "RAID array references partition 'part1', which does not have a fixed size."
        );
    }

    #[test]
    fn test_check_targets() {
        let raid_array = SoftwareRaidArray {
            id: "raid1".to_string(),
            name: "raid1".to_string(),
            devices: vec![],
            metadata_version: "1".into(),
            level: RaidLevel::Raid1,
        };
        let part1 = Partition {
            id: "part1".to_string(),
            size: PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part2 = Partition {
            id: "part2".to_string(),
            size: PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part2b = Partition {
            id: "part2".to_string(),
            size: PartitionSize::Fixed(200),
            partition_type: PartitionType::Root,
        };

        let node = BlkDevNode {
            id: "raid1".to_string(),
            kind: BlkDevKind::RaidArray,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::RaidArray(&raid_array),
            mount_points: vec![],
            image: None,
            targets: vec![],
        };
        let targets = [
            &BlkDevNode {
                id: "part1".to_string(),
                kind: BlkDevKind::Partition,
                dependents: vec![],
                host_config_ref: HostConfigBlockDevice::Partition(&part1),
                mount_points: vec![],
                image: None,
                targets: vec![],
            },
            &BlkDevNode {
                id: "part2".to_string(),
                kind: BlkDevKind::Partition,
                dependents: vec![],
                host_config_ref: HostConfigBlockDevice::Partition(&part2),
                mount_points: vec![],
                image: None,
                targets: vec![],
            },
        ];
        assert!(check_targets(
            &node,
            &targets,
            &BlockDeviceGraph {
                nodes: BTreeMap::new()
            }
        )
        .is_ok());

        // different sizes
        let targets = [
            &BlkDevNode {
                id: "part1".to_string(),
                kind: BlkDevKind::Partition,
                dependents: vec![],
                host_config_ref: HostConfigBlockDevice::Partition(&part1),
                mount_points: vec![],
                image: None,
                targets: vec![],
            },
            &BlkDevNode {
                id: "part2".to_string(),
                kind: BlkDevKind::Partition,
                dependents: vec![],
                host_config_ref: HostConfigBlockDevice::Partition(&part2b),
                mount_points: vec![],
                image: None,
                targets: vec![],
            },
        ];
        assert_eq!(
            check_targets(
                &node,
                &targets,
                &BlockDeviceGraph {
                    nodes: BTreeMap::new()
                }
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "RAID array references partitions with different sizes."
        );
    }
}
