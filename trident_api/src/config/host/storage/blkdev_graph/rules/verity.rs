use anyhow::{bail, Context, Error};

use crate::config::{
    host::storage::blkdev_graph::{
        graph::BlockDeviceGraph,
        types::{BlkDevKind, BlkDevNode, HostConfigBlockDevice},
    },
    Partition, PartitionType,
};

/// Ensure that a partition is of a type that supports verity.
///
/// Allowed types are:
/// - root
/// - root-verity
///
/// Returns Ok(()) if the partition is of an acceptable type, otherwise returns
/// an Error with the details.
fn check_partition_type_supports_verity(part: &Partition) -> Result<(), Error> {
    if !matches!(
        part.partition_type,
        PartitionType::Root | PartitionType::RootVerity
    ) {
        bail!(
            "Partition '{}' is of unsupported type '{:?}'",
            part.id,
            part.partition_type
        );
    }

    Ok(())
}

/// Ensure that the targets of a verity device are initialized by an image.
///
/// Returns Ok(()) if the targets are initialized by an image, otherwise returns
/// an Error with the details.
fn check_targets_image(node: &BlkDevNode, targets: &[&BlkDevNode]) -> Result<(), Error> {
    // Ensure that targets are initialized by an image
    for target in targets {
        // Check the target is initialized by image
        target
            .image
            .context(format!("Block device '{}' is not initialized using image, which is required for verity device '{}' to work", target.id, node.id))?;
    }

    Ok(())
}

/// Ensure that the targets of a verity device are using partitions of expected
/// types. We expect exactly one root and one root-verity partition. If RAID
/// arrays are used instead, we expect one array of root partitions and one
/// array of root-verity partitions. If A/B update volumes are used instead, we
/// expect all underlying partitions to be of the same type and that there is
/// one of each (root, root-verity).
///
/// Returns Ok(()) if the targets are using partitions of expected types, otherwise
/// returns an Error with the details.
fn check_targets_partition_type(
    node: &BlkDevNode,
    targets: &[&BlkDevNode],
    graph: &BlockDeviceGraph,
) -> Result<(), Error> {
    // We have already validated in valid_target_count that there
    // are two targets

    // We have already validated in valid_target_kinds that the
    // targets are either RAID arrays or partitions

    // Ensure that are using partitions of expected types (one root and one
    // root-verity)
    let mut data_found: bool = false;
    let mut hash_found: bool = false;
    for target in targets {
        // check the target is using partitions of expected type
        let part_type = match target.host_config_ref {
            // If the target is a partition, ensure it is of an
            // acceptable type
            HostConfigBlockDevice::Partition(part) => {
                check_partition_type_supports_verity(part).map(|()| part.partition_type)?
            }
            // If the target is a RAID array, ensure all its underlying
            // partitions are of an acceptable type
            HostConfigBlockDevice::RaidArray(_) => check_all_part_types_matching(graph, target)
                .context(format!(
                    "Verity device '{}' targets incompatible RAID array '{}'",
                    node.id, target.id
                ))?,
            // Only supports A/B update volumes over partitions and RAID arrays.
            // Validates that all underlying partitions are of the same type and
            // that there is one of each (root, root-verity).
            HostConfigBlockDevice::ABVolume(_) => check_all_part_types_matching(graph, target)
                .context(format!(
                    "Verity device '{}' targets incompatible A/B update volume '{}'",
                    node.id, target.id
                ))?,

            // Assumption: all other types are invalid
            _ => bail!(
                "Verity device references block device '{}' of invalid kind '{}'",
                target.id,
                target.kind
            ),
        };

        if part_type == PartitionType::Root {
            if data_found {
                bail!(
                    "Verity device '{}' references multiple partitions of type 'root'",
                    node.id
                );
            }
            data_found = true;
        } else {
            // PartitionType::RootVerity
            if hash_found {
                bail!(
                    "Verity device '{}' references multiple partitions of type 'root-verity'",
                    node.id
                );
            }
            hash_found = true;
        }
    }

    if !data_found || !hash_found {
        // should not be reachable
        bail!(
            "Verity device '{}' references a partition of type '{}' without a corresponding partition of type '{}'",
            node.id,
            if data_found { "root-verity" } else { "root" },
            if data_found { "root" } else { "root-verity" }
        );
    }

    Ok(())
}

/// Ensure that all targets of the given node are using partitions of the same
/// kind.
///
/// Returns Ok(()) if the targets are using partitions of the same kind,
/// otherwise returns an Error with the details.
fn check_all_part_types_matching(
    graph: &BlockDeviceGraph<'_>,
    node: &BlkDevNode<'_>,
) -> Result<PartitionType, Error> {
    let part_types = extract_part_types(graph, node).context(format!(
        "Block device of kind '{}' and id '{}' has partitions of invalid type",
        node.kind, node.id,
    ))?;
    let first = part_types.first().context(format!(
        "Block device of kind '{}' and id '{}' has missing underlying partitions",
        node.kind, node.id
    ))?;
    if !part_types.iter().all(|pt| first == pt) {
        bail!(
            "Block device of kind '{}' and id '{}' has partitions of different types",
            node.kind,
            node.id
        );
    }
    Ok(*first)
}

/// Extract partition types from the targets of the given node for further
/// analysis. Supports recursively extracting partition types from RAID arrays.
fn extract_part_types(
    graph: &BlockDeviceGraph<'_>,
    node: &BlkDevNode<'_>,
) -> Result<Vec<PartitionType>, Error> {
    graph
        .targets(&node.id)
        .context(format!(
            "Failed to get targets for '{}' of kind '{}'",
            node.id, node.kind
        ))?
        .iter()
        .flat_map(|target| {
            let part_vec = match target.kind {
                BlkDevKind::Partition => vec![target.host_config_ref.unwrap_partition()],
                BlkDevKind::RaidArray => {
                    target
                        .host_config_ref
                        .unwrap_raid_array()?
                        .devices
                        .iter()
                        .map(|d| {
                            graph
                                .get(d)
                                .context(format!(
                                "Failed to get block device '{}' referenced by RAID array '{}'",
                                d, target.id
                            ))?
                                .host_config_ref
                                .unwrap_partition()
                        })
                        .collect()
                }
                _ => bail!(
                    "Block device '{}' of kind '{}' references block device '{}' of invalid kind '{}'",
                    node.id,
                    node.kind,
                    target.id,
                    target.kind
                ),
            };
            Ok(part_vec)
        })
        .flatten()
        .collect::<Result<Vec<_>, Error>>()
        .context(format!(
            "Failed to get partitions for RAID array '{}'",
            node.id
        ))?
        .into_iter()
        .map(|p| check_partition_type_supports_verity(p).map(|()|p.partition_type))
        .collect::<Result<Vec<_>, Error>>()
}

/// Ensure that all targets of the given node are of the same kind. E.g. two
/// partitions, two RAID arrays or two A/B update volumes. For now, we dont want
/// to support mixing different kinds of targets, e.g. a partition and a RAID array.
///
/// Returns Ok(()) if the targets are of the same kind, otherwise returns an
/// Error with the details.
fn check_targets_kind(targets: &[&BlkDevNode]) -> Result<(), Error> {
    let first = targets.first().context("Missing targets")?.kind;
    if !targets.iter().all(|t| t.kind == first) {
        bail!("Inconsistent targets detected",);
    }

    // TODO perform deeper analysis, e.g. if the targets are A/b update volumes,
    // ensure that both all A and all B volumes are of the same kind as well.

    Ok(())
}

/// Ensure that the targets of the given node are valid for a verity device.
/// This includes checking for the following:
/// - The targets are initialized by an image.
/// - The targets are using partitions of expected types (one root and one root-verity).
/// - The targets are of the same kind.
pub(super) fn check_targets(
    node: &BlkDevNode,
    targets: &[&BlkDevNode],
    graph: &BlockDeviceGraph,
) -> Result<(), Error> {
    // We have already validated in valid_target_count that there
    // are two targets

    // We have already validated in valid_target_kinds that the
    // targets are either RAID arrays or partitions

    // Ensure that both targets are initialized by an image and are
    // using partitions of expected types (one root and one root-verity) and are
    // of the same kind.
    check_targets_image(node, targets).context(format!(
        "Verity device '{}' points to a block device that has not been initialized with an image",
        node.id
    ))?;
    check_targets_partition_type(node, targets, graph).context(format!(
        "Verity device '{}' points to block devices with incompatible partition types",
        node.id
    ))?;
    check_targets_kind(targets).context(format!(
        "Verity device '{}' point to block devices of different kinds",
        node.id
    ))?;

    Ok(())
}

#[cfg(test)]
mod test {
    use std::collections::BTreeMap;

    use crate::config::{
        host::storage::{blkdev_graph::types::BlkDevKind, VerityDevice},
        AbVolumePair, Image, ImageFormat, ImageSha256, SoftwareRaidArray,
    };

    use super::*;

    #[test]
    fn test_check_partition_type_supports_verity() {
        let partition = Partition {
            id: "foo".into(),
            partition_type: PartitionType::Root,
            size: crate::config::PartitionSize::Fixed(0),
        };
        check_partition_type_supports_verity(&partition).unwrap();

        let partition = Partition {
            id: "foo".into(),
            partition_type: PartitionType::RootVerity,
            size: crate::config::PartitionSize::Fixed(0),
        };
        check_partition_type_supports_verity(&partition).unwrap();

        let partition = Partition {
            id: "foo".into(),
            partition_type: PartitionType::Esp,
            size: crate::config::PartitionSize::Fixed(0),
        };
        assert_eq!(
            check_partition_type_supports_verity(&partition)
                .unwrap_err()
                .to_string(),
            "Partition 'foo' is of unsupported type 'Esp'"
        );
    }

    #[test]
    fn test_check_targets_image() {
        let image = Image {
            url: "foo".into(),
            sha256: ImageSha256::Ignored,
            format: ImageFormat::RawZst,
            target_id: "foo".into(),
        };

        let part_root = Partition {
            id: "part_root".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part_root_node = BlkDevNode {
            id: "part_root".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root),
            mount_points: vec![],
            image: Some(&image),
            dependents: vec![],
        };

        let part_root_verity = Partition {
            id: "part_root_verity".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::RootVerity,
        };
        let part_root_verity_node = BlkDevNode {
            id: "part_root_verity".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root_verity),
            mount_points: vec![],
            image: Some(&image),
            dependents: vec![],
        };

        let part_root_node_no_image = BlkDevNode {
            id: "part_root".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let part_root_verity_node_no_image = BlkDevNode {
            id: "part_root_verity".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root_verity),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let verity = VerityDevice {
            id: "verity".into(),
            device_name: "verity".into(),
            data_target_id: "part_root".into(),
            hash_target_id: "part_root_verity".into(),
        };

        let verity1 = BlkDevNode {
            id: "verity1".into(),
            kind: BlkDevKind::VerityDevice,
            targets: vec!["part_root".into(), "part_root_verity".into()],
            host_config_ref: HostConfigBlockDevice::VerityDevice(&verity),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        check_targets_image(&verity1, &[&part_root_node, &part_root_verity_node]).unwrap();

        assert_eq!(
            check_targets_image(
                &verity1,
                &[&part_root_node_no_image, &part_root_verity_node],
            )
            .unwrap_err()
            .to_string(),
            "Block device 'part_root' is not initialized using image, which is required for verity device 'verity1' to work"
        );

        assert_eq!(
            check_targets_image(
                &verity1,
                &[&part_root_node, &part_root_verity_node_no_image],
            )
            .unwrap_err()
            .to_string(),
            "Block device 'part_root_verity' is not initialized using image, which is required for verity device 'verity1' to work"
        );

        assert_eq!(
            check_targets_image(
                &verity1,
                &[&part_root_node_no_image, &part_root_verity_node_no_image],
            )
            .unwrap_err()
            .to_string(),
            "Block device 'part_root' is not initialized using image, which is required for verity device 'verity1' to work"
        );
    }

    #[test]
    fn test_extract_part_types() {
        let part1 = Partition {
            id: "part1".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part2 = Partition {
            id: "part2".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::RootVerity,
        };
        let raid = SoftwareRaidArray {
            id: "raid".to_string(),
            name: "raid".to_string(),
            devices: vec![],
            metadata_version: "1".into(),
            level: crate::config::RaidLevel::Raid1,
        };

        let part1_node = BlkDevNode {
            id: "part1".to_string(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part1),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let part2_node = BlkDevNode {
            id: "part2".to_string(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part2),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let mut raid_node = BlkDevNode {
            id: "raid".to_string(),
            kind: BlkDevKind::RaidArray,
            targets: vec!["part1".into(), "part2".into()],
            host_config_ref: HostConfigBlockDevice::RaidArray(&raid),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let mut nodes = BTreeMap::from_iter(vec![
            (part1_node.id.clone(), part1_node.clone()),
            (part2_node.id.clone(), part2_node.clone()),
            (raid_node.id.clone(), raid_node.clone()),
        ]);

        let graph = BlockDeviceGraph {
            nodes: nodes.clone(),
        };

        assert_eq!(
            extract_part_types(&graph, &raid_node)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![&PartitionType::Root, &PartitionType::RootVerity]
        );

        assert_eq!(
            extract_part_types(&graph, &raid_node)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![&PartitionType::Root, &PartitionType::RootVerity]
        );

        raid_node.targets = vec!["part1".into(), "part1".into()];
        nodes.remove(&raid_node.id.clone());
        nodes.insert(raid_node.id.clone(), raid_node.clone());
        let graph = BlockDeviceGraph {
            nodes: nodes.clone(),
        };

        assert_eq!(
            extract_part_types(&graph, &raid_node)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            vec![&PartitionType::Root, &PartitionType::Root]
        );

        let part3 = Partition {
            id: "part3".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::Esp,
        };

        let part3_node = BlkDevNode {
            id: "part3".to_string(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part3),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        raid_node.targets = vec!["part1".into(), "part3".into()];
        nodes.remove(&raid_node.id.clone());
        nodes.insert(raid_node.id.clone(), raid_node.clone());
        nodes.insert(part3_node.id.clone(), part3_node.clone());
        let graph = BlockDeviceGraph {
            nodes: nodes.clone(),
        };

        assert_eq!(
            extract_part_types(&graph, &raid_node)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Partition 'part3' is of unsupported type 'Esp'"
        )
    }

    #[test]
    fn test_check_all_part_types_matching() {
        let part1 = Partition {
            id: "part1".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part2 = Partition {
            id: "part2".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::RootVerity,
        };
        let part3 = Partition {
            id: "part3".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let raid = SoftwareRaidArray {
            id: "raid".to_string(),
            name: "raid".to_string(),
            devices: vec![],
            metadata_version: "1".into(),
            level: crate::config::RaidLevel::Raid1,
        };

        let part1_node = BlkDevNode {
            id: "part1".to_string(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part1),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let part2_node = BlkDevNode {
            id: "part2".to_string(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part2),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let part3_node = BlkDevNode {
            id: "part3".to_string(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part3),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let mut raid_node = BlkDevNode {
            id: "raid".to_string(),
            kind: BlkDevKind::RaidArray,
            targets: vec!["part1".into(), "part3".into()],
            host_config_ref: HostConfigBlockDevice::RaidArray(&raid),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let mut nodes = BTreeMap::from_iter(vec![
            (part1_node.id.clone(), part1_node.clone()),
            (part2_node.id.clone(), part2_node.clone()),
            (part3_node.id.clone(), part3_node.clone()),
            (raid_node.id.clone(), raid_node.clone()),
        ]);

        let graph = BlockDeviceGraph {
            nodes: nodes.clone(),
        };

        assert_eq!(
            check_all_part_types_matching(&graph, &raid_node).unwrap(),
            PartitionType::Root
        );

        // different types
        raid_node.targets = vec!["part1".into(), "part2".into()];
        nodes.remove(&raid_node.id.clone());
        nodes.insert(raid_node.id.clone(), raid_node.clone());
        let graph = BlockDeviceGraph {
            nodes: nodes.clone(),
        };

        assert_eq!(
            check_all_part_types_matching(&graph, &raid_node)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Block device of kind 'raid-array' and id 'raid' has partitions of different types"
        );

        // missing partitions
        raid_node.targets = vec![];
        nodes.remove(&raid_node.id.clone());
        nodes.insert(raid_node.id.clone(), raid_node.clone());
        let graph = BlockDeviceGraph {
            nodes: nodes.clone(),
        };

        assert_eq!(
            check_all_part_types_matching(&graph, &raid_node)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Block device of kind 'raid-array' and id 'raid' has missing underlying partitions"
        );
    }

    #[test]
    fn test_check_targets_partition_type() {
        let mut nodes = BTreeMap::new();

        let part_root = Partition {
            id: "part_root".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part_root_node = BlkDevNode {
            id: "part_root".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let part_root_verity = Partition {
            id: "part_root_verity".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::RootVerity,
        };
        let part_root_verity_node = BlkDevNode {
            id: "part_root_verity".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root_verity),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let part_esp = Partition {
            id: "part_esp".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::Esp,
        };
        let part_esp_node = BlkDevNode {
            id: "part_esp".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_esp),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let part_root2 = Partition {
            id: "part_root2".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part_root2_node = BlkDevNode {
            id: "part_root2".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root2),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let part_root_verity2 = Partition {
            id: "part_root_verity2".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::RootVerity,
        };
        let part_root_verity2_node = BlkDevNode {
            id: "part_root_verity2".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root_verity2),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let verity = VerityDevice {
            id: "verity".into(),
            device_name: "verity".into(),
            data_target_id: "part_root".into(),
            hash_target_id: "part_root_verity".into(),
        };

        let verity1 = BlkDevNode {
            id: "verity1".into(),
            kind: BlkDevKind::VerityDevice,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::VerityDevice(&verity),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        nodes.insert("part_root".to_string(), part_root_node.clone());
        nodes.insert(
            "part_root_verity".to_string(),
            part_root_verity_node.clone(),
        );
        nodes.insert("part_esp".to_string(), part_esp_node.clone());
        nodes.insert("part_root2".to_string(), part_root2_node.clone());
        nodes.insert("part_verity2".to_string(), part_root_verity2_node.clone());

        nodes.insert("verity1".to_string(), verity1.clone());

        check_targets_partition_type(
            &verity1,
            &[&part_root_node, &part_root_verity_node],
            &BlockDeviceGraph {
                nodes: nodes.clone(),
            },
        )
        .unwrap();

        check_targets_partition_type(
            &verity1,
            &[&part_root_verity_node, &part_root_node],
            &BlockDeviceGraph {
                nodes: nodes.clone(),
            },
        )
        .unwrap();

        assert_eq!(
            check_targets_partition_type(
                &verity1,
                &[&part_root_node, &part_esp_node],
                &BlockDeviceGraph {
                    nodes: nodes.clone(),
                }
            )
            .unwrap_err()
            .to_string(),
            "Partition 'part_esp' is of unsupported type 'Esp'"
        );

        assert_eq!(
            check_targets_partition_type(
                &verity1,
                &[&part_root_verity_node, &part_esp_node],
                &BlockDeviceGraph {
                    nodes: nodes.clone(),
                }
            )
            .unwrap_err()
            .to_string(),
            "Partition 'part_esp' is of unsupported type 'Esp'"
        );

        assert_eq!(
            check_targets_partition_type(
                &verity1,
                &[&part_esp_node, &part_esp_node],
                &BlockDeviceGraph {
                    nodes: nodes.clone(),
                }
            )
            .unwrap_err()
            .to_string(),
            "Partition 'part_esp' is of unsupported type 'Esp'"
        );

        assert_eq!(
            check_targets_partition_type(
                &verity1,
                &[&part_root_node, &part_root_node],
                &BlockDeviceGraph {
                    nodes: nodes.clone(),
                }
            )
            .unwrap_err()
            .to_string(),
            "Verity device 'verity1' references multiple partitions of type 'root'"
        );

        assert_eq!(
            check_targets_partition_type(
                &verity1,
                &[&part_root_verity_node, &part_root_verity_node],
                &BlockDeviceGraph {
                    nodes: nodes.clone(),
                }
            )
            .unwrap_err()
            .to_string(),
            "Verity device 'verity1' references multiple partitions of type 'root-verity'"
        );

        // similar for raid arrays
        let raid = SoftwareRaidArray {
            id: "raid".to_string(),
            name: "raid".to_string(),
            devices: vec![],
            metadata_version: "1".into(),
            level: crate::config::RaidLevel::Raid1,
        };
        let raid_node = BlkDevNode {
            id: "raid".to_string(),
            kind: BlkDevKind::RaidArray,
            targets: vec!["part_root".into()],
            host_config_ref: HostConfigBlockDevice::RaidArray(&raid),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        nodes.insert("raid".to_string(), raid_node.clone());

        check_targets_partition_type(
            &verity1,
            &[&part_root_verity_node, &raid_node],
            &BlockDeviceGraph {
                nodes: nodes.clone(),
            },
        )
        .unwrap();

        assert_eq!(
            check_targets_partition_type(
                &verity1,
                &[&part_root_node, &raid_node],
                &BlockDeviceGraph {
                    nodes: nodes.clone(),
                }
            )
            .unwrap_err()
            .to_string(),
            "Verity device 'verity1' references multiple partitions of type 'root'"
        );

        // similar for ab volumes
        let ab_volume = AbVolumePair {
            id: "ab_volume".to_string(),
            volume_a_id: "part_root".to_string(),
            volume_b_id: "part_root2".to_string(),
        };
        let ab_volume_node = BlkDevNode {
            id: "ab_volume".to_string(),
            kind: BlkDevKind::ABVolume,
            targets: vec!["part_root".into(), "part_root2".into()],
            host_config_ref: HostConfigBlockDevice::ABVolume(&ab_volume),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };
        nodes.insert("ab_volume".to_string(), ab_volume_node.clone());

        check_targets_partition_type(
            &verity1,
            &[&part_root_verity_node, &ab_volume_node],
            &BlockDeviceGraph {
                nodes: nodes.clone(),
            },
        )
        .unwrap();

        assert_eq!(
            check_targets_partition_type(
                &verity1,
                &[&part_root_node, &ab_volume_node],
                &BlockDeviceGraph {
                    nodes: nodes.clone(),
                }
            )
            .unwrap_err()
            .to_string(),
            "Verity device 'verity1' references multiple partitions of type 'root'"
        );
    }

    #[test]
    fn test_check_targets_kind() {
        let part_root = Partition {
            id: "part_root".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::Root,
        };
        let part_root_node = BlkDevNode {
            id: "part_root".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let part_root_verity = Partition {
            id: "part_root_verity".to_string(),
            size: crate::config::PartitionSize::Fixed(100),
            partition_type: PartitionType::RootVerity,
        };
        let part_root_verity_node = BlkDevNode {
            id: "part_root_verity".into(),
            kind: BlkDevKind::Partition,
            targets: vec![],
            host_config_ref: HostConfigBlockDevice::Partition(&part_root_verity),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let raid = SoftwareRaidArray {
            id: "raid".to_string(),
            name: "raid".to_string(),
            devices: vec![],
            metadata_version: "1".into(),
            level: crate::config::RaidLevel::Raid1,
        };
        let raid_node = BlkDevNode {
            id: "raid".to_string(),
            kind: BlkDevKind::RaidArray,
            targets: vec!["part_root".into(), "part_root_verity".into()],
            host_config_ref: HostConfigBlockDevice::RaidArray(&raid),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        let ab_volume = AbVolumePair {
            id: "ab_volume".to_string(),
            volume_a_id: "part_root".to_string(),
            volume_b_id: "part_root_verity".to_string(),
        };
        let ab_volume_node = BlkDevNode {
            id: "ab_volume".to_string(),
            kind: BlkDevKind::ABVolume,
            targets: vec!["part_root".into(), "part_root_verity".into()],
            host_config_ref: HostConfigBlockDevice::ABVolume(&ab_volume),
            mount_points: vec![],
            image: None,
            dependents: vec![],
        };

        check_targets_kind(&[&part_root_node, &part_root_verity_node]).unwrap();

        check_targets_kind(&[&raid_node, &raid_node]).unwrap();

        check_targets_kind(&[&ab_volume_node, &ab_volume_node]).unwrap();

        assert_eq!(
            check_targets_kind(&[&part_root_node, &raid_node])
                .unwrap_err()
                .to_string(),
            "Inconsistent targets detected"
        );

        assert_eq!(
            check_targets_kind(&[&part_root_node, &ab_volume_node])
                .unwrap_err()
                .to_string(),
            "Inconsistent targets detected"
        );

        assert_eq!(
            check_targets_kind(&[&raid_node, &ab_volume_node])
                .unwrap_err()
                .to_string(),
            "Inconsistent targets detected"
        );
    }
}
