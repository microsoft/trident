//! # Block device graph builder
//!
//! This module contains the core logic to build a block device graph based on
//! the provided host configuration.
//!
//! The struct BlockDeviceGraphBuilder collects all definitions from the host
//! configuration.
//!
//! After entering all information, the build() function is called to build the
//! graph.
//!
//! The build() function will perform the following checks:
//! - Generic checks that are applicable to all block devices:
//!   - Checking for duplicate IDs.
//!   - Checking that all references are valid.
//!   - Check that all nodes/(aka block devices) are referenced by at most one
//!     other.
//!   - Check that all targets/members for a node are distinct. (i.e. can't use
//!     the same block device twice)
//! - Call the validation rules defined in the `rules` module to perform
//!   per-kind validation.
//! - Validate that all mount points are unique and valid.
//! - Validate that all filesystems are valid.
//!   - Check that all filesystems have a valid block device ID if required.
//!   - Check that all filesystems have a valid mount point if provided.
//!
//! The end result is a BlockDeviceGraph struct that contains all the nodes and
//! their relationships. This graph, can be considered as fully valid.
//!
//! If the output is Err, it means that the host configuration provided is
//! invalid.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use anyhow::Context;
use log::warn;

use crate::{
    config::{FileSystem, PartitionSize, VerityFileSystem},
    BlockDeviceId,
};

use super::{
    error::BlockDeviceGraphBuildError,
    graph::BlockDeviceGraph,
    types::{BlkDevKind, BlkDevNode, BlkDevReferrerKind, FileSystemSourceKind, NodeFileSystem},
};

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BlockDeviceGraphBuilder<'a> {
    nodes: Vec<BlkDevNode<'a>>,
    // images: Vec<&'a Image>,
    // mount_points: Vec<&'a MountPoint>,
    filesystems: Vec<&'a FileSystem>,
    verity_filesystems: Vec<&'a VerityFileSystem>,
}

impl<'a> BlockDeviceGraphBuilder<'a> {
    /// Adds a new block device node to the graph
    pub(crate) fn add_node(&mut self, node: BlkDevNode<'a>) {
        self.nodes.push(node);
    }

    // /// Adds a new image to the graph
    // pub(crate) fn add_image(&mut self, image: &'a Image) {
    //     self.images.push(image);
    // }

    // /// Adds a new mount point to the graph
    // pub(crate) fn add_mount_point(&mut self, mount_point: &'a MountPoint) {
    //     self.mount_points.push(mount_point);
    // }

    pub(crate) fn add_filesystem(&mut self, filesystem: &'a FileSystem) {
        self.filesystems.push(filesystem);
    }

    pub(crate) fn add_verity_filesystem(&mut self, verity_filesystem: &'a VerityFileSystem) {
        self.verity_filesystems.push(verity_filesystem);
    }

    /// Builds the block device graph
    ///
    /// This function will check that all nodes, their references, mount points,
    /// and images are valid.
    ///
    /// It will also check that no block device is referenced by more than one
    /// other block device, enforcing exclusive ownership of block devices.
    pub(crate) fn build(self) -> Result<BlockDeviceGraph<'a>, BlockDeviceGraphBuildError> {
        // Create a map of block device IDs to nodes
        let mut nodes: BTreeMap<BlockDeviceId, BlkDevNode<'a>> = BTreeMap::new();

        // First, add all the nodes to the map to check for duplicates.
        for node in self.nodes {
            if let Some(other_definition) = nodes.insert(node.id.clone(), node) {
                return Err(BlockDeviceGraphBuildError::DuplicateDeviceId(
                    other_definition.id.clone(),
                ));
            }
        }

        // Check that all nodes and their references are valid
        Self::build_nodes(&mut nodes)?;

        // Check that all mountpoints are unique and valid
        Self::check_mountpounts(&self.filesystems, &self.verity_filesystems)?;

        // Check that all filesystems are valid and register them with their respective nodes
        // Get a list of filesystems that do not have a block device ID
        let deviceless_filesystems = Self::build_filesystems(&self.filesystems, &mut nodes)?;

        // Check that all verity filesystems are valid and have unique names
        Self::build_verity_filesystems(&self.verity_filesystems, &mut nodes)?;

        // The graph can be built now
        // After this point we only do immutable checks on it!
        let graph = BlockDeviceGraph {
            nodes,
            deviceless_filesystems,
        };

        // Check that all nodes & filesystems have dependents of the same block
        // device kind when required
        Self::check_dependency_kind_homogeneity(&graph)?;

        // Check all dependents for sharing compatibility
        Self::check_sharing(&graph)?;

        // Check unique field values requirements
        Self::check_unique_fields(&graph)?;

        // Check that underlying partitions are homogeneous when required
        Self::check_partition_homogeneity(&graph)?;

        // Check that underlying partitions are of valid types
        Self::check_valid_partition_types(&graph)?;

        // Check targets for each node
        Self::check_targets(&graph)?;

        Ok(graph)
    }

    /// Check that all nodes and their references are valid
    fn build_nodes(
        nodes: &mut BTreeMap<BlockDeviceId, BlkDevNode<'a>>,
    ) -> Result<(), BlockDeviceGraphBuildError> {
        // Fun stuff to get around the borrow checker!
        //
        // We want to iterate over all the nodes, but while we do that, we also
        // want to be able to modify other nodes in the map to add dependents.
        //
        // To achieve this, we iterate over a copy of the keys, and then get an
        // immutable reference from the map to the current node, which we clone
        // so we don't hold an immutable reference to the map. We do NOT modify
        // this node!
        //
        // We do NOT ADD OR REMOVE NODES from the map while iterating, so this
        // is safe. This is similar to iterating over the indices of a vector,
        // instead of an iterator.
        //
        // Note: We clone and explicitly collect to make sure the clone happens
        // and we drop all references to the map before the loop starts.
        for node_id in nodes.keys().cloned().collect::<Vec<BlockDeviceId>>() {
            // Clone of the current node from the map.
            let node = nodes
                .get(&node_id)
                .ok_or(BlockDeviceGraphBuildError::InternalError {
                    body: format!("Failed to get known node '{node_id}' from map of nodes"),
                })?
                .clone();

            // Perform basic checks on the node.
            node.host_config_ref.basic_check().map_err(|e| {
                BlockDeviceGraphBuildError::BasicCheckFailed {
                    node_id: node.id.clone(),
                    kind: node.kind(),
                    body: e.to_string(),
                }
            })?;

            // Check that all members are unique.
            {
                let mut unique_targets = BTreeSet::new();
                for target in node.targets.iter() {
                    if !unique_targets.insert(target) {
                        return Err(BlockDeviceGraphBuildError::DuplicateTargetId {
                            kind: node.kind(),
                            node_id: node.id,
                            target_id: target.clone(),
                        });
                    }
                }
            }

            // Check that we have a valid number of members.
            {
                let valid_cardinality = node.kind().as_blkdev_referrer().valid_target_count();
                let target_count = node.targets.len();

                if !valid_cardinality.contains(target_count) {
                    return Err(BlockDeviceGraphBuildError::InvalidTargetCount {
                        kind: node.kind(),
                        node_id: node.id,
                        target_count,
                        expected: valid_cardinality,
                    });
                }
            }

            // Then check each member individually.
            for target in node.targets.iter() {
                // Try to get a mutable reference to the member node on the map.
                let target_node = nodes.get_mut(target).ok_or_else(|| {
                    BlockDeviceGraphBuildError::NonExistentReference {
                        node_id: node.id.clone(),
                        kind: node.kind(),
                        target_id: target.clone(),
                    }
                })?;

                // Get the list of kinds compatible with the current node.
                let compatible_kinds = node.kind().as_blkdev_referrer().compatible_kinds();

                // Check that the target is of a compatible kind.
                if !compatible_kinds.contains(target_node.kind().as_flag()) {
                    return Err(BlockDeviceGraphBuildError::InvalidReferenceKind {
                        kind: node.kind(),
                        node_id: node.id,
                        target_id: target.clone(),
                        target_kind: target_node.kind(),
                        valid_references: compatible_kinds,
                    });
                }

                // Add the current node as a dependent of the target node.
                // This will be further checked later once we build the full graph.
                target_node.dependents.push(node.id.clone());
            }
        }

        Ok(())
    }

    /// Check that all mountpoints are unique and valid
    fn check_mountpounts(
        filesystems: &[&'a FileSystem],
        verity_filesystems: &[&'a VerityFileSystem],
    ) -> Result<(), BlockDeviceGraphBuildError> {
        // Create a set of all unique mount points
        let mut unique_mount_points = BTreeSet::new();

        // Create an iterator with all mount points from all filesystems and
        // verity filesystems
        filesystems
            .iter()
            .flat_map(|fs| fs.mount_point.iter())
            .chain(verity_filesystems.iter().map(|fs| &fs.mount_point))
            .try_for_each(|mntp| {
                if !unique_mount_points.insert(mntp.path.clone()) {
                    return Err(BlockDeviceGraphBuildError::DuplicateMountPoint(
                        mntp.path.to_string_lossy().into(),
                    ));
                }

                // Ensure the mount point path is absolute
                if !mntp.path.is_absolute() {
                    return Err(BlockDeviceGraphBuildError::MountPointPathNotAbsolute(
                        mntp.path.to_string_lossy().into(),
                    ));
                }

                Ok(())
            })
    }

    // Check that all filesystems are valid
    // Register all filesystems with their respective nodes
    // Returns a list of filesystems that do not have a block device ID
    fn build_filesystems(
        filesystems: &[&'a FileSystem],
        nodes: &mut BTreeMap<BlockDeviceId, BlkDevNode<'a>>,
    ) -> Result<Vec<&'a FileSystem>, BlockDeviceGraphBuildError> {
        let mut deviceless_filesystems: Vec<&'a FileSystem> = Vec::new();

        // Check that all filesystems are valid
        for fs in filesystems {
            // Check if the mountpoint *can* be provided
            // Eg. swap cannot have a mountpoint
            if !fs.fs_type.can_have_mountpoint() && fs.mount_point.is_some() {
                return Err(BlockDeviceGraphBuildError::FilesystemUnexpectedMountPoint(
                    fs.fs_type,
                ));
            }

            // Check that the filesystem source is compatible with the filesystem type.
            {
                let compatible_sources = fs.fs_type.valid_sources();
                let fs_src_kind = FileSystemSourceKind::from(&fs.source);
                if !compatible_sources.contains(fs_src_kind) {
                    return Err(BlockDeviceGraphBuildError::FilesystemIncompatibleSource {
                        fs_desc: fs.description(),
                        fs_source: fs_src_kind,
                        fs_compatible_sources: compatible_sources,
                    });
                }
            }

            // Do checks depending on whether a block device ID was provided
            if let Some(device_id) = fs.device_id.as_ref() {
                // Check if we were not expecting a block device ID and got one
                if !fs.fs_type.requires_block_device_id() {
                    return Err(
                        BlockDeviceGraphBuildError::FilesystemUnexpectedBlockDeviceId(fs.fs_type),
                    );
                }

                // Try to get the node from the map.
                let node = nodes.get_mut(device_id).ok_or_else(|| {
                    BlockDeviceGraphBuildError::FilesystemNonExistentReference {
                        target_id: device_id.clone(),
                        fs_desc: fs.description(),
                    }
                })?;

                // Depending on the details of the filesystem, we can have different compatible
                // referrer kinds.
                let compatible_kinds = BlkDevReferrerKind::from(*fs).compatible_kinds();

                // Check that the node is of a compatible kind.
                if !compatible_kinds.contains(node.kind().as_flag()) {
                    return Err(
                        BlockDeviceGraphBuildError::FilesystemIncompatibleReference {
                            fs_desc: fs.description(),  // The filesystem's description.
                            target_id: node.id.clone(), // The node being referenced.
                            target_kind: node.kind(),   // The node's kind.
                            compatible_kinds, // A list of kinds compatible with the given filesystem.
                        },
                    );
                }

                update_node_filesystem(node, NodeFileSystem::Regular(fs))?;
            } else {
                // Check if we were expecting a block device ID and did not get one.
                if fs.fs_type.requires_block_device_id() {
                    return Err(BlockDeviceGraphBuildError::FilesystemMissingBlockDeviceId(
                        fs.fs_type,
                    ));
                }

                // Add the filesystem to the list of deviceless filesystems
                deviceless_filesystems.push(fs);
            }
        }

        Ok(deviceless_filesystems)
    }

    /// Check that all verity filesystems are valid and have unique names
    /// Also associate the filesystems with the nodes
    fn build_verity_filesystems(
        verity_filesystems: &[&'a VerityFileSystem],
        nodes: &mut BTreeMap<BlockDeviceId, BlkDevNode<'a>>,
    ) -> Result<(), BlockDeviceGraphBuildError> {
        // Set to check for unique names
        let mut unique_names = BTreeSet::new();

        for vfs in verity_filesystems {
            // Check if the filesystem is supported and can have a mountpoint
            if !vfs.fs_type.supports_verity() || !vfs.fs_type.can_have_mountpoint() {
                return Err(
                    BlockDeviceGraphBuildError::VerityFileSystemUnsupportedType {
                        name: vfs.name.clone(),
                        fs_type: vfs.fs_type,
                    },
                );
            }

            // Check for unique names
            if !unique_names.insert(vfs.name.clone()) {
                return Err(BlockDeviceGraphBuildError::VerityDuplicateName {
                    name: vfs.name.clone(),
                });
            }

            // Get and update data node
            let data_node = nodes.get_mut(&vfs.data_device_id).ok_or_else(|| {
                BlockDeviceGraphBuildError::VerityFilesystemNonExistentReference {
                    name: vfs.name.clone(),
                    target_id: vfs.data_device_id.clone(),
                    fs_type: vfs.fs_type,
                    role: "data".into(),
                }
            })?;

            // Depending on the details of the filesystem, we can have different compatible referrer kinds.
            let data_compatible_kinds: super::types::BlkDevKindFlag =
                BlkDevReferrerKind::VerityFileSystemData.compatible_kinds();

            // Check that the node is of a compatible kind.
            if !data_compatible_kinds.contains(data_node.kind().as_flag()) {
                return Err(
                    BlockDeviceGraphBuildError::VerityFilesystemIncompatibleReferenceData {
                        name: vfs.name.clone(),
                        fs_type: vfs.fs_type,
                        target_id: data_node.id.clone(),
                        target_kind: data_node.kind(),
                        compatible_kinds: data_compatible_kinds,
                    },
                );
            }

            update_node_filesystem(data_node, NodeFileSystem::VerityData(vfs))?;

            // Get and update hash node
            let hash_node = nodes.get_mut(&vfs.hash_device_id).ok_or_else(|| {
                BlockDeviceGraphBuildError::VerityFilesystemNonExistentReference {
                    name: vfs.name.clone(),
                    target_id: vfs.hash_device_id.clone(),
                    fs_type: vfs.fs_type,
                    role: "hash".into(),
                }
            })?;

            let hash_compatible_kinds = BlkDevReferrerKind::VerityFileSystemData.compatible_kinds();

            // Check that the node is of a compatible kind.
            if !hash_compatible_kinds.contains(hash_node.kind().as_flag()) {
                return Err(
                    BlockDeviceGraphBuildError::VerityFilesystemIncompatibleReferenceHash {
                        name: vfs.name.clone(),
                        fs_type: vfs.fs_type,
                        target_id: hash_node.id.clone(),
                        target_kind: hash_node.kind(),
                        compatible_kinds: hash_compatible_kinds,
                    },
                );
            }

            update_node_filesystem(hash_node, NodeFileSystem::VerityHash(vfs))?;
        }

        Ok(())
    }

    /// Check all dependencies for homogeneity kind
    fn check_dependency_kind_homogeneity(
        graph: &BlockDeviceGraph<'a>,
    ) -> Result<(), BlockDeviceGraphBuildError> {
        // Get all the nodes for which we need to check the homogeneity of the dependents
        for node in graph.nodes.values().filter(|n| {
            n.kind()
                .as_blkdev_referrer()
                .enforce_homogeneous_reference_kinds()
        }) {
            let target_kinds = graph
                .targets(&node.id)
                .ok_or(BlockDeviceGraphBuildError::InternalError {
                    body: format!(
                        "Failed to get targets for node '{}' of kind '{}'",
                        node.id,
                        node.kind()
                    ),
                })?
                .iter()
                .map(|target| target.kind())
                .collect::<Vec<_>>();

            if target_kinds.is_empty() {
                return Ok(()); // Nothing to check
            }

            let first_kind = target_kinds[0];
            if !target_kinds.iter().all(|k| *k == first_kind) {
                return Err(BlockDeviceGraphBuildError::ReferenceKindMismatch {
                    referrer: node.id.clone(),
                    kind: node.kind().as_blkdev_referrer(),
                });
            }
        }

        // Now check that for all filesystems
        // We iterate over all nodes to get the filesystems
        for fs in graph
            .nodes
            .values()
            .filter_map(|node| node.filesystem)
            .filter(|fs| BlkDevReferrerKind::from(*fs).enforce_homogeneous_reference_kinds())
        {
            // Get IDs of all targets of this filesystem
            let targets = fs
                .targets()
                .into_iter()
                .map(|target| {
                    graph
                        .get(&target)
                        .ok_or(BlockDeviceGraphBuildError::InternalError {
                            body: format!(
                                "Failed to get known node '{}' from map of nodes",
                                target
                            ),
                        })
                })
                .collect::<Result<Vec<&BlkDevNode>, BlockDeviceGraphBuildError>>()?;

            if targets.is_empty() {
                return Ok(()); // Nothing to check
            }

            let target_kinds = targets
                .iter()
                .map(|target| target.kind())
                .collect::<Vec<_>>();
            let first_kind = target_kinds[0];
            if !target_kinds.iter().all(|k| *k == first_kind) {
                return Err(BlockDeviceGraphBuildError::ReferenceKindMismatch {
                    referrer: fs.identity(),
                    kind: BlkDevReferrerKind::from(fs),
                });
            }
        }

        Ok(())
    }

    /// Check all dependents for sharing compatibility
    fn check_sharing(graph: &BlockDeviceGraph<'a>) -> Result<(), BlockDeviceGraphBuildError> {
        for node in graph.nodes.values() {
            let dependents = node
                .dependents
                .iter()
                .map(|id| {
                    graph
                        .get(id)
                        .ok_or(BlockDeviceGraphBuildError::InternalError {
                            body: format!("Failed to get known node '{}' from map of nodes", id),
                        })
                })
                .collect::<Result<Vec<&BlkDevNode>, BlockDeviceGraphBuildError>>()?;

            // Good 'ol 1/2 n^2 loop to check all dependents for sharing compatibility among each other.
            for (i, dependent_a) in dependents.iter().enumerate() {
                for dependent_b in dependents.iter().skip(i + 1) {
                    check_mutual_sharing_peers(
                        &node.id,
                        node.kind(),
                        &dependent_a.id,
                        dependent_a.kind().as_blkdev_referrer(),
                        &dependent_b.id,
                        dependent_b.kind().as_blkdev_referrer(),
                    )?;
                }
            }

            // Check that nodes with filesystems are not shared with other referrers of invalid kind.
            if let Some(fs) = node.filesystem {
                for dependent in dependents.iter() {
                    check_mutual_sharing_peers(
                        &node.id,
                        node.kind(),
                        fs.identity(),
                        BlkDevReferrerKind::from(fs),
                        &dependent.id,
                        dependent.kind().as_blkdev_referrer(),
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Check unique field values requirements
    fn check_unique_fields(graph: &BlockDeviceGraph<'a>) -> Result<(), BlockDeviceGraphBuildError> {
        // Create a hash map to keep track of field uniqueness.
        let mut unique_fields: HashMap<BlkDevKind, HashMap<&'static str, HashMap<&[u8], &str>>> =
            HashMap::new();

        // Iterate over all nodes and check for unique fields.
        for (id, node) in graph.nodes.iter() {
            // Check if we have uniqueness constraints for this node kind
            if let Some(uniqueness_constraint) = node.kind().uniqueness_constraints() {
                // Iterate over each uniqueness constraint and check if the field is unique
                for (field_name, extractor) in uniqueness_constraint {
                    // For every contrained field, extract its value using the provided extractor function.
                    let opt_field_value = extractor(&node.host_config_ref)
                        // Add some context about what we were doing.
                        .with_context(|| {
                            format!(
                                "Failed to extract field '{}' from node '{}' of kind '{}'",
                                field_name,
                                id,
                                node.kind()
                            )
                        })
                        // Map the error to an internal error, this should never happen.
                        .map_err(|err| BlockDeviceGraphBuildError::InternalError {
                            body: format!("{:?}", err),
                        })?;

                    // We only care to check if there is a value for the field
                    if let Some(field_value) = opt_field_value {
                        // Check if the field value is unique
                        if let Some(other_id) = unique_fields
                            // First get the map of this specific node kind
                            .entry(node.kind())
                            .or_default()
                            // Then get the entry of this specific field
                            .entry(field_name)
                            .or_default()
                            // Finally, try to insert the field value
                            .insert(field_value, id)
                        {
                            // If we got here, another node od the same kind had
                            // the same value for this field :(
                            return Err(BlockDeviceGraphBuildError::UniqueFieldConstraintError {
                                node_id: id.clone(),
                                other_id: other_id.into(),
                                kind: node.kind(),
                                field_name: field_name.into(),
                                value: String::from_utf8_lossy(field_value).into(),
                            });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn check_partition_homogeneity(
        graph: &BlockDeviceGraph<'a>,
    ) -> Result<(), BlockDeviceGraphBuildError> {
        // Check partition size homogeneity for all nodes that require it
        for node in graph.nodes.values().filter(|node| {
            node.kind()
                .as_blkdev_referrer()
                .enforce_homogeneous_partition_sizes()
        }) {
            let partition_sizes = graph.get_partition_sizes(&node.id)?.ok_or(
                BlockDeviceGraphBuildError::InternalError {
                    body: format!(
                        "check_partition_homogeneity: Failed to get partitions for node '{}' of kind '{}'",
                        node.id, node.kind()
                    ),
                },
            )?;

            if partition_sizes.is_empty() {
                return Err(BlockDeviceGraphBuildError::InternalError {
                    body: format!(
                        "check_partition_homogeneity: partition_sizes is empty for node '{}' of kind '{}'",
                        node.id, node.kind()
                    ),
                });
            }

            // Ensure all partition sizes are fixed
            partition_sizes.iter().try_for_each(|attr| {
                if !matches!(attr.value, PartitionSize::Fixed(_)) {
                    return Err(BlockDeviceGraphBuildError::PartitionSizeNotFixed {
                        node_id: node.id.clone(),
                        kind: node.kind(),
                        partition_id: attr.id.to_string(),
                    });
                }

                Ok(())
            })?;

            // Ensure all are equal
            if !partition_sizes.is_homogeneous() {
                return Err(BlockDeviceGraphBuildError::PartitionSizeMismatch {
                    node_id: node.id.clone(),
                    kind: node.kind(),
                });
            }
        }

        // Check partition type homogeneity for all nodes that require it
        for node in graph.nodes.values().filter(|node| {
            node.kind()
                .as_blkdev_referrer()
                .enforce_homogeneous_partition_types()
        }) {
            let partition_types = graph.get_partition_type(&node.id)?.ok_or(
                BlockDeviceGraphBuildError::InternalError {
                    body: format!(
                        "check_partition_homogeneity: Failed to get partitions for node '{}' of kind '{}'",
                        node.id, node.kind()
                    ),
                },
            )?;

            if partition_types.is_empty() {
                return Err(BlockDeviceGraphBuildError::InternalError {
                    body: format!(
                        "check_partition_homogeneity: partition_types is empty for node '{}' of kind '{}'",
                        node.id, node.kind()
                    ),
                });
            }

            // Ensure all partition types are equal
            if !partition_types.is_homogeneous() {
                return Err(BlockDeviceGraphBuildError::PartitionTypeMismatch {
                    node_id: node.id.clone(),
                    kind: node.kind(),
                });
            }
        }

        // Check that all nodes with filesystems have homogeneous partition types
        for (node, fs) in graph
            .nodes
            .values()
            .filter(|node| node.filesystem.is_some())
            .map(|node| (node, node.filesystem.unwrap()))
            .filter(|(_, fs)| BlkDevReferrerKind::from(*fs).enforce_homogeneous_partition_types())
        {
            // Get all partitions for the node
            let partition_types = graph.get_partition_type(&node.id)?.ok_or(
                BlockDeviceGraphBuildError::InternalError {
                    body: format!(
                        "check_partition_homogeneity: Failed to get partitions for node '{}' of kind '{}'",
                        node.id, node.kind()
                    ),
                },
            )?;

            // This should have already been checked, but just in case
            if partition_types.is_empty() {
                return Err(BlockDeviceGraphBuildError::InternalError {
                    body: format!(
                        "check_partition_homogeneity: partition_types is empty for node '{}' of kind '{}'",
                        node.id, node.kind()
                    ),
                });
            }

            // Ensure all partition types are equal
            if !partition_types.is_homogeneous() {
                return Err(
                    BlockDeviceGraphBuildError::FilesystemHeterogenousPartitionTypes {
                        referrer: BlkDevReferrerKind::from(fs),
                        fs_desc: fs.description(),
                    },
                );
            }
        }

        Ok(())
    }

    /// Check that all underlying partitions are of valid types
    fn check_valid_partition_types(
        graph: &BlockDeviceGraph<'a>,
    ) -> Result<(), BlockDeviceGraphBuildError> {
        // Iterate over all nodes that can have partition kinds.
        // This means skip disks and adopted partitions.
        for node in graph
            .nodes
            .values()
            .filter(|node| node.kind().has_partition_type())
        {
            // Get all partitions for the node
            let partition_types = graph.get_partition_type(&node.id)?.ok_or(
                BlockDeviceGraphBuildError::InternalError {
                        body: format!(
                            "check_valid_partition_types: Failed to get partitions for node '{}' of kind '{}'",
                            node.id, node.kind()
                        ),
                    },
                )?;

            // Check that the node has a valid partition type
            let valid_part_types = node.kind().as_blkdev_referrer().allowed_partition_types();
            partition_types.iter().try_for_each(|attr| {
                if !valid_part_types.contains(attr.value) {
                    return Err(BlockDeviceGraphBuildError::InvalidPartitionType {
                        node_id: node.id.clone(),
                        kind: node.kind(),
                        partition_id: attr.id.to_string(),
                        partition_type: attr.value,
                        valid_types: valid_part_types.clone(),
                    });
                }

                Ok(())
            })?;

            // If the node has an associated filesystem, check the filesystem's partition type requirements
            if let Some(node_fs) = node.filesystem {
                // This has already been checked in check_partition_homogeneity, but just in case
                let partition_type = partition_types.get_homogeneous().ok_or(
                    BlockDeviceGraphBuildError::InternalError { body: format!(
                        "check_valid_partition_types: Failed to get homogenous partition type for node '{}' of kind '{}'",
                        node.id, node.kind()
                    ) },
                )?;

                // If this filesystem is being mounted and check the valid mountpoints for the partition type
                // This check may be too restrictive to produce a hard error, but we still want to warn the user
                // about a potential issue.
                if let Some(mount_point) = node_fs.mountpoint() {
                    let valid = partition_type.valid_mountpoints();
                    if !valid.contains(&mount_point.path) {
                        warn!(
                            "Mount point '{}' may not valid for partition type '{}', partitions of \
                                this type will generally be mounted at {}.",
                            mount_point.path.display(),
                            partition_type,
                            valid,
                        );
                    }
                }

                // Assuming we got a homogeneous partition type, check that it is compatible with
                // the given filesystem.
                let compatible_part_types =
                    BlkDevReferrerKind::from(node_fs).allowed_partition_types();
                if !compatible_part_types.contains(*partition_type) {
                    return Err(
                        BlockDeviceGraphBuildError::FilesystemIncompatiblePartitionType {
                            referrer: BlkDevReferrerKind::from(node_fs),
                            fs_desc: node_fs.description(),
                            partition_type: *partition_type,
                            compatible_types: compatible_part_types.clone(),
                        },
                    );
                }

                // Finally, if the node has a verity filesystem, check that the partition types match
                if let NodeFileSystem::VerityData(vfs) = node_fs {
                    // Get the expected hash partition type for the type of this data partition
                    let expected_hash_part_type = partition_type.to_verity().ok_or_else(|| {
                        BlockDeviceGraphBuildError::VerityFilesystemInvalidDataPartitionType {
                            name: vfs.name.clone(),
                            fs_type: vfs.fs_type,
                            partition_type: *partition_type,
                        }
                    })?;

                    // Get the type of the hash partition
                    let hash_part_type = *graph.get_partition_type(&vfs.hash_device_id)?.ok_or_else(||
                        BlockDeviceGraphBuildError::InternalError {
                            body: format!(
                                "check_valid_partition_types: Failed to get partitions for node '{}' of kind '{}'",
                                vfs.hash_device_id, node.kind()
                            ),
                        },
                    )?
                    // Ensure it is homogeneous
                    .get_homogeneous().ok_or({
                        BlockDeviceGraphBuildError::FilesystemHeterogenousPartitionTypes {
                            referrer: BlkDevReferrerKind::VerityFileSystemHash,
                            fs_desc: vfs.description(),
                        }
                    })?;

                    // Finally check that the hash partition type matches the expected type
                    if hash_part_type != expected_hash_part_type {
                        return Err(
                            BlockDeviceGraphBuildError::VerityFilesystemPartitionTypeMismatch {
                                name: vfs.name.clone(),
                                fs_type: vfs.fs_type,
                                data_part_type: *partition_type,
                                expected_type: expected_hash_part_type,
                                actual_type: hash_part_type,
                            },
                        );
                    }
                }
            }
        }

        Ok(())
    }

    /// Check targets for each node
    fn check_targets(graph: &BlockDeviceGraph<'a>) -> Result<(), BlockDeviceGraphBuildError> {
        for node in graph.nodes.values().filter(|n| !n.targets.is_empty()) {
            // This should never fail, since we already checked that all targets exist.
            let targets =
                graph
                    .targets(&node.id)
                    .ok_or(BlockDeviceGraphBuildError::InternalError {
                        body: format!(
                            "Failed to get targets for node '{}' of kind '{}'",
                            node.id,
                            node.kind()
                        ),
                    })?;

            node.kind()
                .as_blkdev_referrer()
                .check_targets(node, &targets, graph)
                .map_err(|e| BlockDeviceGraphBuildError::InvalidTargets {
                    node_id: node.id.clone(),
                    kind: node.kind(),
                    body: format!("{:#}", e),
                })?;
        }
        Ok(())
    }
}

/// Check that two referrers can share a target
fn check_mutual_sharing_peers(
    target_id: impl Into<String>,
    target_kind: BlkDevKind,
    referrer_a_id: impl Into<String>,
    referrer_a: BlkDevReferrerKind,
    referrer_b_id: impl Into<String>,
    referrer_b: BlkDevReferrerKind,
) -> Result<(), BlockDeviceGraphBuildError> {
    let referrer_a_valid_sharing_peers = referrer_a.valid_sharing_peers();
    let referrer_b_valid_sharing_peers = referrer_b.valid_sharing_peers();
    if !(referrer_a_valid_sharing_peers.contains(referrer_b.as_flag())
        && referrer_b_valid_sharing_peers.contains(referrer_a.as_flag()))
    {
        return Err(BlockDeviceGraphBuildError::ReferrerForbiddenSharing {
            target_id: target_id.into(),
            target_kind,
            referrer_a_id: referrer_a_id.into(),
            referrer_a_kind: referrer_a,
            referrer_b_id: referrer_b_id.into(),
            referrer_b_kind: referrer_b,
            referrer_a_valid_sharing_peers,
            referrer_b_valid_sharing_peers,
        });
    }

    Ok(())
}

/// Associate the filesystem with the node when possible.
/// Otherwise, throw an error if the node already has a filesystem associated with it.
fn update_node_filesystem<'a>(
    node: &mut BlkDevNode<'a>,
    nfs: NodeFileSystem<'a>,
) -> Result<(), BlockDeviceGraphBuildError> {
    match node.filesystem {
        None => {
            node.filesystem = Some(nfs);
            Ok(())
        }
        Some(NodeFileSystem::Regular(other_fs)) => {
            Err(BlockDeviceGraphBuildError::FilesystemReferenceInUse {
                fs_desc: nfs.description(),
                target_id: node.id.clone(),
                other_fs_desc: other_fs.description(),
            })
        }
        Some(NodeFileSystem::VerityData(other_vfs))
        | Some(NodeFileSystem::VerityHash(other_vfs)) => {
            Err(BlockDeviceGraphBuildError::FilesystemReferenceInUseVerity {
                fs_desc: nfs.description(),
                target_id: node.id.clone(),
                other_vfs_name: other_vfs.name.clone(),
                other_vfs_type: other_vfs.fs_type,
            })
        }
    }
}
