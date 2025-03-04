use log::{trace, warn};
use petgraph::{
    visit::{EdgeRef, IntoNodeReferences},
    Direction,
};

use crate::{
    config::{
        host::storage::storage_graph::{
            containers::{BlkDevAttrList, PathAllowBlockList},
            error::StorageGraphBuildError,
            graph::{NodeIndex, StoragePetgraph},
            node::StorageGraphNode,
            references::{ReferenceKind, SpecialReferenceKind},
            types::HostConfigBlockDevice,
        },
        Partition, PartitionSize,
    },
    storage_graph::{
        containers::AllowBlockList, graph, node::BlockDevice, rules::expected_partition_type,
    },
};

impl SpecialReferenceKind {
    /// Returns whether this special reference kind should pass through partition attributes.
    fn pass_through_partition_attributes(&self) -> bool {
        match self {
            // The verity data device should pass through partition attributes,
            // these are what will be seen by the filesystem.
            SpecialReferenceKind::VerityDataDevice => true,

            // The verity hash device should NOT pass through partition
            // attributes as the hash device is entirely consumed by the verity
            // device.
            SpecialReferenceKind::VerityHashDevice => false,
        }
    }
}

/// Checks referrers for partition size homogeneity.
pub(super) fn check_partition_size_homogeneity(
    graph: &StoragePetgraph,
) -> Result<(), StorageGraphBuildError> {
    // Get all top-level nodes
    for (node_idx, node) in get_top_level_nodes(graph) {
        trace!(
            "Checking partition size homogeneity for top-level node: {}",
            node.describe()
        );

        explore_tree_partitions(
            graph,
            node_idx,
            &|part| part.size,
            &|_, _| Ok(()),
            &|node, attr_list| {
                // Check if we care about checking partition sizes at this level.
                if !node.referrer_kind().enforce_homogeneous_partition_sizes() {
                    return Ok(());
                }

                // Ensure all partitions have fixed sizes.
                attr_list.iter().try_for_each(|attr| {
                    if let PartitionSize::Fixed(_) = attr.value {
                        return Ok(());
                    }

                    Err(StorageGraphBuildError::PartitionSizeNotFixed {
                        node_identifier: node.identifier(),
                        kind: node.referrer_kind(),
                        partition_id: attr.id.clone(),
                    })
                })?;

                // Ensure all partitions have the same size.
                if !attr_list.is_homogeneous() {
                    return Err(StorageGraphBuildError::PartitionSizeMismatch {
                        node_identifier: node.identifier(),
                        kind: node.referrer_kind(),
                    });
                }

                Ok(())
            },
        )?;
    }

    Ok(())
}

/// Checks referrers for partition type homogeneity and for valid partition
/// types.
pub(super) fn check_partition_types(graph: &StoragePetgraph) -> Result<(), StorageGraphBuildError> {
    // Get all top-level nodes
    for (node_idx, node) in get_top_level_nodes(graph) {
        trace!(
            "Checking partition type homogeneity and validity for top-level node: {}",
            node.describe()
        );

        // If this is a filesystem with a mount point, get the expected partition
        // type for the filesystem's mount point, if any. Otherwise, allow any
        // partition type.
        let expected_partition_type = node
            .as_filesystem()
            .and_then(|fs| fs.mount_point.as_ref())
            .map(|mntp| expected_partition_type(&mntp.path))
            .unwrap_or(AllowBlockList::Any);

        let partition_types = explore_tree_partitions(
            graph,
            node_idx,
            &|part| part.partition_type,
            &|special_kind, attr_list| {
                // Check if we care about checking partition types at this level.
                if special_kind
                    .enforce_homogeneous_partition_types()
                    .is_some_and(|r| r)
                {
                    // Ensure all partitions have the same type.
                    if !attr_list.is_homogeneous() {
                        return Err(StorageGraphBuildError::PartitionTypeMismatchSpecial {
                            node_identifier: node.identifier(),
                            kind: node.referrer_kind(),
                            special_ref_kind: special_kind,
                        });
                    }
                }

                if let Some(allowed_types) = special_kind.allowed_partition_types() {
                    // Ensure all partitions have valid types.
                    attr_list.iter().try_for_each(|attr| {
                        if allowed_types.contains(attr.value) {
                            return Ok(());
                        }

                        Err(StorageGraphBuildError::InvalidPartitionType {
                            node_identifier: node.identifier(),
                            kind: node.referrer_kind(),
                            partition_id: attr.id.clone(),
                            partition_type: attr.value,
                            valid_types: allowed_types.clone(),
                        })
                    })?;
                }

                Ok(())
            },
            &|node, attr_list| {
                // Check if we care about checking partition types at this level.
                if node.referrer_kind().enforce_homogeneous_partition_types() {
                    // Ensure all partitions have the same type.
                    if !attr_list.is_homogeneous() {
                        return Err(StorageGraphBuildError::PartitionTypeMismatch {
                            node_identifier: node.identifier(),
                            kind: node.referrer_kind(),
                        });
                    }
                }

                // Get allowed types for the node.
                let allowed_types = node.referrer_kind().allowed_partition_types();

                // Ensure all partitions have valid types.
                attr_list.iter().try_for_each(|attr| {
                    if !expected_partition_type.contains(attr.value) {
                        Err(StorageGraphBuildError::InvalidPartitionType {
                            node_identifier: node.identifier(),
                            kind: node.referrer_kind(),
                            partition_id: attr.id.clone(),
                            partition_type: attr.value,
                            valid_types: expected_partition_type.clone(),
                        })
                    } else if !allowed_types.contains(attr.value) {
                        Err(StorageGraphBuildError::InvalidPartitionType {
                            node_identifier: node.identifier(),
                            kind: node.referrer_kind(),
                            partition_id: attr.id.clone(),
                            partition_type: attr.value,
                            valid_types: allowed_types.clone(),
                        })
                    } else {
                        Ok(())
                    }
                })?;

                Ok(())
            },
        )?;

        // Check if this is a filesystem with a mount point.
        let mnt_point = match node {
            StorageGraphNode::FileSystem(fs) => {
                fs.mount_point.as_ref().map(|mntp| mntp.path.as_path())
            }
            _ => None,
        };

        // If we have a mount point, check if the partition type is valid for the
        // filesystem.
        if let Some(mnt_path) = mnt_point {
            // Try to get the homogeneous partition type.
            if let Some(part_type) = partition_types.get_homogeneous() {
                // Check if the partition type is valid for the filesystem.
                let valid_mntpoints = part_type.valid_mountpoints();
                if !valid_mntpoints.contains(mnt_path) {
                    warn!(
                        "Mount point '{}' for {} may not be valid for partition type '{}', partitions \
                            of this type will generally be mounted at {}.",
                        mnt_path.display(),
                        node.describe(),
                        part_type,
                        PathAllowBlockList::from(valid_mntpoints),
                    );
                }
            } else if !partition_types.is_empty() {
                // IF we couldn't get the homogeneous partition type, but the
                // list of types is NOT empty, it means we have a mix of
                // partition types, but all node types with a mountpoint should
                // enforce this requirement, and it should have been checked
                // before getting here.
                return Err(StorageGraphBuildError::InternalError {
                    body: format!(
                        "Expected {} to have homogeneous partition types.",
                        node.describe()
                    ),
                });
            } else {
                // The implicit else here is that the list is empty, which generally
                // means we have something like an adopted partition, where we
                // cannot tell what type it is yet.
            }
        }
    }

    Ok(())
}

/// Checks that all verity devices have congruent data and hash partition types.
///
/// Congruency in this context means that the hash partition type matches the
/// expected hash partition type for the corresponding data partition type.
///
/// E.g.: If the data partition type is `root`, then the hash partition type
/// must be `root-verity`. If the data partition type is `usr`, then the hash
/// partition type must be `usr-verity`.
pub(super) fn check_verity_partition_types(
    graph: &StoragePetgraph,
) -> Result<(), StorageGraphBuildError> {
    for (idx, node) in graph.node_references() {
        // Filter out non-verity devices.
        let StorageGraphNode::BlockDevice(BlockDevice {
            host_config_ref: HostConfigBlockDevice::VerityDevice(dev),
            ..
        }) = node
        else {
            continue;
        };

        let data_device_idx =
            graph::find_special_reference(graph, idx, SpecialReferenceKind::VerityDataDevice)
                .ok_or_else(|| StorageGraphBuildError::InternalError {
                    body: format!(
                        "Verity device '{}' does not have a data device reference.",
                        dev.name
                    ),
                })?;

        let hash_device_idx =
            graph::find_special_reference(graph, idx, SpecialReferenceKind::VerityHashDevice)
                .ok_or_else(|| StorageGraphBuildError::InternalError {
                    body: format!(
                        "Verity device '{}' does not have a hash device reference.",
                        dev.name
                    ),
                })?;

        // Get the partition types of the data and hash devices.
        let data_dev_partition_type = *explore_tree_partitions(
            graph,
            data_device_idx,
            &|part| part.partition_type,
            &|_, _| Ok(()),
            &|_, _| Ok(()),
        )?
        .get_homogeneous()
        .ok_or_else(|| StorageGraphBuildError::InternalError {
            body: format!(
                "Verity device '{}' does not have a homogeneous data device partition type.",
                dev.name
            ),
        })?;

        let hash_dev_partition_type = *explore_tree_partitions(
            graph,
            hash_device_idx,
            &|part| part.partition_type,
            &|_, _| Ok(()),
            &|_, _| Ok(()),
        )?
        .get_homogeneous()
        .ok_or_else(|| StorageGraphBuildError::InternalError {
            body: format!(
                "Verity device '{}' does not have a homogeneous hash device partition type.",
                dev.name
            ),
        })?;

        let expected_hash_partition_type =
            data_dev_partition_type.to_verity().ok_or_else(|| {
                StorageGraphBuildError::InternalError {
                    body: format!(
                        "Data device '{}' of verity device '{}' has an invalid partition type '{}' which does not support verity.",
                        dev.name, dev.name, data_dev_partition_type
                    ),
                }
            })?;

        if hash_dev_partition_type != expected_hash_partition_type {
            return Err(StorageGraphBuildError::InvalidVerityHashPartitionType {
                node_id: dev.name.clone(),
                data_dev_partition_type,
                hash_dev_partition_type,
                expected_type: expected_hash_partition_type,
            });
        }
    }

    Ok(())
}

/// Recursively explores the graph as a tree to extract partition attributes for
/// a given node.
///
/// For base cases (i.e. partitions), the function returns the partition's
/// attribute. For other nodes, the function recursively explores the node's
/// references and collects their attributes.
///
/// It also checks on:
/// - Specific relationships between nodes when the relationship between them is
///   of a special kind.
/// - All the attributes collected from all of the node's references.
///
/// For example, given the following graph:
///
/// ```text
/// filesystem1 (Filesystem)
/// └── array1 (RAID Array)
///     ├── partition1 (Partition)
///     ├── partition2 (Partition)
///     └── partition3 (Partition)
/// ```
///
/// If the function is called with a partition node, it will return the
/// partition's attribute as obtained by the extractor function. If the function
/// is called with the RAID array node, it will recursively explore the
/// references and collect a list of the attributes collected by the extractor
/// function from `partition1`, `partition2`, and `partition3`. It then runs the
/// node check function on the RAID array node and the list of attributes.
///
/// Because the filesystem only has the RAID array as a child, the function will
/// produce the same result as if it were called with the RAID array node, but
/// it will run the node check function on the filesystem node.
///
/// Expects:
/// - `graph` to be a valid graph.
/// - `node_idx` to be a valid node index.
/// - `node` to be the node at `node_idx`.
/// - `extractor` to be a function that extracts a value from a partition.
/// - `special_edge_check` to be a function that checks the attributes collected
///   up to a specific special relationship of a node. Example usage: check that
///   all partition types under the verity hash relationship of a verity node
///   are of a verity hash type.
/// - `node_check` to be a function that checks the attributes collected up to a
///   node.
fn explore_tree_partitions<'a, T>(
    graph: &'a StoragePetgraph,
    node_idx: NodeIndex,
    extractor: &impl Fn(&Partition) -> T,
    special_edge_check: &impl Fn(
        SpecialReferenceKind,
        &BlkDevAttrList<'a, T>,
    ) -> Result<(), StorageGraphBuildError>,
    node_check: &impl Fn(
        &StorageGraphNode,
        &BlkDevAttrList<'a, T>,
    ) -> Result<(), StorageGraphBuildError>,
) -> Result<BlkDevAttrList<'a, T>, StorageGraphBuildError> {
    let node = &graph[node_idx];
    trace!("Exploring node: {}", node.describe());

    // Base cases
    if let StorageGraphNode::BlockDevice(dev) = node {
        match &dev.host_config_ref {
            // Ignore disks
            HostConfigBlockDevice::Disk(_) => return Ok(BlkDevAttrList::default()),

            // Adopted partitions have no known attributes.
            HostConfigBlockDevice::AdoptedPartition(_) => return Ok(BlkDevAttrList::default()),

            // Return the attribute of the partition
            HostConfigBlockDevice::Partition(part) => {
                return Ok(BlkDevAttrList::new(&part.id, extractor(part)));
            }

            // Ignore other block devices
            _ => (),
        }
    }

    let mut attr_list = BlkDevAttrList::default();

    for edge in graph.edges_directed(node_idx, Direction::Outgoing) {
        // Extract the data associated with this edge. (ReferenceKind)
        let ref_kind = edge.weight();

        // Get the dependency node index and the dependency node object.
        let dependency_idx = edge.target();
        let dependency_node = &graph[edge.target()];

        let StorageGraphNode::BlockDevice(_) = dependency_node else {
            return Err(StorageGraphBuildError::InternalError {
                body: format!(
                    "Node {} is not a block device and NOT expected to have dependents, \
                        but {} depends on it.",
                    dependency_node.describe(),
                    node.describe()
                ),
            });
        };

        let edge_attr_list = explore_tree_partitions(
            graph,
            dependency_idx,
            extractor,
            special_edge_check,
            node_check,
        )?;

        // Check the edge attributes for special references.
        if let ReferenceKind::Special(special_ref_kind) = *ref_kind {
            special_edge_check(special_ref_kind, &edge_attr_list)?;
        }

        // Add the attributes of the edge to our own list ONLY for regular
        // references and special references that pass through partition
        // attributes.
        if ref_kind.is_regular_or(|r| r.pass_through_partition_attributes()) {
            attr_list.extend(edge_attr_list);
        }
    }

    // Check the attributes of the node.
    node_check(node, &attr_list)?;

    Ok(attr_list)
}

/// Returns an iterator over the top-level nodes in the graph.
///
/// A top-level node is a node that has no incoming edges.
///
/// These will generally be filesystems or block devices that are not referenced
/// by any other device or filesystem (this includes disks!).
fn get_top_level_nodes(
    graph: &StoragePetgraph,
) -> impl Iterator<Item = (NodeIndex, &StorageGraphNode)> {
    graph
        .node_references()
        .filter(|(idx, _)| graph.edges_directed(*idx, Direction::Incoming).count() == 0)
}
