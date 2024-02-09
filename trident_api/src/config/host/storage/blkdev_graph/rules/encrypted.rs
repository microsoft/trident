use anyhow::{bail, Context, Error};

use crate::config::{
    host::storage::blkdev_graph::{
        graph::BlockDeviceGraph,
        types::{BlkDevNode, HostConfigBlockDevice},
    },
    Partition, PartitionType,
};

/// Ensure that a partition is of a type that supports encryption.
///
/// Disallowed types are:
/// - esp
/// - root
/// - root-verity
fn check_partition_type_supports_encryption(part: &Partition) -> Result<(), Error> {
    if matches!(
        part.partition_type,
        PartitionType::Esp | PartitionType::Root | PartitionType::RootVerity
    ) {
        bail!(
            "Partition '{}' is of unsupported type '{:?}'.",
            part.id,
            part.partition_type
        );
    }
    Ok(())
}

pub(super) fn check_targets(
    _node: &BlkDevNode,
    targets: &[&BlkDevNode],
    graph: &BlockDeviceGraph,
) -> Result<(), Error> {
    // Assumption: just one target exists.
    // We already validated that targets.len() == 1
    let target = targets[0];
    match target.host_config_ref {
        // If the target is a partition, ensure it is of an
        // acceptable type
        HostConfigBlockDevice::Partition(part) => {
            check_partition_type_supports_encryption(part)?;
        }
        // If the target is a RAID array, ensure all its underlying
        // partitions are of an acceptable type
        HostConfigBlockDevice::RaidArray(_) => {
            check_raid_part_types(graph, target)?;
        }

        // Assumption: all other types are invalid
        _ => bail!(
            "Encrypted volume references block device '{}' of invalid kind '{}'.",
            target.id,
            target.kind
        ),
    }

    Ok(())
}

fn check_raid_part_types(
    graph: &BlockDeviceGraph<'_>,
    target: &BlkDevNode<'_>,
) -> Result<(), Error> {
    graph
        .targets(&target.id)
        .context(format!(
            "Failed to get targets for RAID array '{}'.",
            target.id
        ))?
        .iter()
        // Assumption: all targets are partitions
        .map(|target| target.host_config_ref.unwrap_partition())
        .collect::<Result<Vec<&Partition>, Error>>()
        .context(format!(
            "Failed to get partitions for RAID array '{}'.",
            target.id
        ))?
        .into_iter()
        .try_for_each(check_partition_type_supports_encryption)
        .context("Encrypted volume references invalid RAID array.")?;
    Ok(())
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use crate::config::{
        host::storage::blkdev_graph::types::BlkDevKind, EncryptedVolume, PartitionSize, RaidLevel,
        SoftwareRaidArray,
    };

    use super::*;

    #[test]
    fn test_check_partition_type_supports_encryption() {
        let partition = Partition {
            id: "foo".into(),
            partition_type: PartitionType::Root,
            size: crate::config::PartitionSize::Fixed(0),
        };
        assert_eq!(
            check_partition_type_supports_encryption(&partition)
                .unwrap_err()
                .to_string(),
            "Partition 'foo' is of unsupported type 'Root'."
        );

        let partition = Partition {
            id: "foo".into(),
            partition_type: PartitionType::RootVerity,
            size: crate::config::PartitionSize::Fixed(0),
        };
        assert_eq!(
            check_partition_type_supports_encryption(&partition)
                .unwrap_err()
                .to_string(),
            "Partition 'foo' is of unsupported type 'RootVerity'."
        );

        let partition = Partition {
            id: "foo".into(),
            partition_type: PartitionType::Esp,
            size: crate::config::PartitionSize::Fixed(0),
        };
        assert_eq!(
            check_partition_type_supports_encryption(&partition)
                .unwrap_err()
                .to_string(),
            "Partition 'foo' is of unsupported type 'Esp'."
        );

        let partition = Partition {
            id: "foo".into(),
            partition_type: PartitionType::LinuxGeneric,
            size: crate::config::PartitionSize::Fixed(0),
        };
        assert!(check_partition_type_supports_encryption(&partition).is_ok());
    }

    #[test]
    fn test_check_raid_part_types() {
        let part1 = Partition {
            id: "part1".to_string(),
            size: PartitionSize::Fixed(100),
            partition_type: PartitionType::LinuxGeneric,
        };
        let part1_node = BlkDevNode {
            id: "part1".to_string(),
            kind: BlkDevKind::Partition,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part1),
            mount_points: vec![],
            image: None,
            targets: vec![],
        };
        let part2 = Partition {
            id: "part2".to_string(),
            size: PartitionSize::Fixed(100),
            partition_type: PartitionType::LinuxGeneric,
        };
        let part2_node = BlkDevNode {
            id: "part2".to_string(),
            kind: BlkDevKind::Partition,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part2),
            mount_points: vec![],
            image: None,
            targets: vec![],
        };
        let part3 = Partition {
            id: "part3".to_string(),
            size: PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part3_node = BlkDevNode {
            id: "part3".to_string(),
            kind: BlkDevKind::Partition,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part3),
            mount_points: vec![],
            image: None,
            targets: vec![],
        };

        let raid1 = SoftwareRaidArray {
            id: "raid1".into(),
            level: RaidLevel::Raid1,
            name: "raid1".into(),
            metadata_version: "1".into(),
            devices: vec!["part1".into(), "part2".into()],
        };

        let raid_node = BlkDevNode {
            id: "raid1".to_string(),
            kind: BlkDevKind::RaidArray,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::RaidArray(&raid1),
            mount_points: vec![],
            image: None,
            targets: vec!["part1".to_string(), "part2".to_string()],
        };

        let mut nodes = BTreeMap::from_iter(vec![
            (part1_node.id.clone(), part1_node.clone()),
            (part2_node.id.clone(), part2_node.clone()),
            (part3_node.id.clone(), part3_node.clone()),
            (raid_node.id.clone(), raid_node.clone()),
        ]);

        check_raid_part_types(
            &BlockDeviceGraph {
                nodes: nodes.clone(),
            },
            &raid_node,
        )
        .unwrap();

        let raid2 = SoftwareRaidArray {
            id: "raid2".into(),
            level: RaidLevel::Raid1,
            name: "raid1".into(),
            metadata_version: "1".into(),
            devices: vec!["part1".into(), "part3".into()],
        };

        let raid2_node = BlkDevNode {
            id: "raid2".to_string(),
            kind: BlkDevKind::RaidArray,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::RaidArray(&raid2),
            mount_points: vec![],
            image: None,
            targets: vec!["part1".into(), "part3".into()],
        };
        nodes.insert(raid2_node.id.clone(), raid2_node.clone());
        assert_eq!(
            check_raid_part_types(
                &BlockDeviceGraph {
                    nodes: nodes.clone(),
                },
                &raid2_node,
            )
            .unwrap_err()
            .to_string(),
            "Encrypted volume references invalid RAID array."
        );
    }

    #[test]
    fn test_check_targets() {
        let encrypted_volume = EncryptedVolume {
            id: "foo".into(),
            device_name: "foo".into(),
            target_id: "foo".into(),
        };
        let part1 = Partition {
            id: "part1".to_string(),
            size: PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part2 = Partition {
            id: "part2".to_string(),
            size: PartitionSize::Fixed(100),
            partition_type: PartitionType::LinuxGeneric,
        };

        let node = BlkDevNode {
            id: "luks1".to_string(),
            kind: BlkDevKind::EncryptedVolume,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::EncryptedVolume(&encrypted_volume),
            mount_points: vec![],
            image: None,
            targets: vec![],
        };
        let targets = [&BlkDevNode {
            id: "part2".to_string(),
            kind: BlkDevKind::Partition,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part2),
            mount_points: vec![],
            image: None,
            targets: vec![],
        }];
        assert!(check_targets(
            &node,
            &targets,
            &BlockDeviceGraph {
                nodes: BTreeMap::new()
            }
        )
        .is_ok());

        // different sizes
        let targets = [&BlkDevNode {
            id: "part1".to_string(),
            kind: BlkDevKind::Partition,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part1),
            mount_points: vec![],
            image: None,
            targets: vec![],
        }];
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
            "Partition 'part1' is of unsupported type 'Root'."
        );

        // RAID
        let raid1 = SoftwareRaidArray {
            id: "raid1".into(),
            level: RaidLevel::Raid1,
            name: "raid1".into(),
            metadata_version: "1".into(),
            devices: vec!["part1".into(), "part2".into()],
        };
        let raid_node = BlkDevNode {
            id: "raid1".to_string(),
            kind: BlkDevKind::RaidArray,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::RaidArray(&raid1),
            mount_points: vec![],
            image: None,
            targets: vec!["part2".to_string()],
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(raid_node.id.clone(), raid_node.clone());
        let targets = [&BlkDevNode {
            id: "part2".to_string(),
            kind: BlkDevKind::Partition,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part2),
            mount_points: vec![],
            image: None,
            targets: vec![],
        }];
        check_targets(
            &node,
            &targets,
            &BlockDeviceGraph {
                nodes: nodes.clone(),
            },
        )
        .unwrap();

        let raid_node = BlkDevNode {
            id: "raid1".to_string(),
            kind: BlkDevKind::RaidArray,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::RaidArray(&raid1),
            mount_points: vec![],
            image: None,
            targets: vec!["part1".to_string()],
        };
        let mut nodes = BTreeMap::new();
        nodes.insert(raid_node.id.clone(), raid_node.clone());
        let targets = [&BlkDevNode {
            id: "part1".to_string(),
            kind: BlkDevKind::Partition,
            dependents: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part1),
            mount_points: vec![],
            image: None,
            targets: vec![],
        }];
        assert_eq!(
            check_targets(
                &node,
                &targets,
                &BlockDeviceGraph {
                    nodes: nodes.clone()
                }
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Partition 'part1' is of unsupported type 'Root'."
        );
    }
}
