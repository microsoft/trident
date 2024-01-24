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
//! - Validate that all images are valid.
//! - Validate that all mount points are valid.
//! - Validate that all mount points are unique. (except for swap and none)
//!
//! The end result is a BlockDeviceGraph struct that contains all the nodes and
//! their relationships. This graph, can be considered as fully valid.
//!
//! If the output is Err, it means that the host configuration provided is
//! invalid.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::{
    config::{Image, MountPoint},
    constants::{NONE_MOUNT_POINT, SWAP_MOUNT_POINT},
    BlockDeviceId,
};

use super::{
    error::BlockDeviceGraphBuildError,
    graph::BlockDeviceGraph,
    rules::VALID_NON_PATH_MOUNT_POINTS,
    types::{BlkDevKind, BlkDevNode, BlkDevReferrerKind},
};

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct BlockDeviceGraphBuilder<'a> {
    nodes: Vec<BlkDevNode<'a>>,
    images: Vec<&'a Image>,
    mount_points: Vec<&'a MountPoint>,
}

impl<'a> BlockDeviceGraphBuilder<'a> {
    /// Adds a new block device node to the graph
    pub(crate) fn add_node(&mut self, node: BlkDevNode<'a>) {
        self.nodes.push(node);
    }

    /// Adds a new image to the graph
    pub(crate) fn add_image(&mut self, image: &'a Image) {
        self.images.push(image);
    }

    /// Adds a new mount point to the graph
    pub(crate) fn add_mount_point(&mut self, mount_point: &'a MountPoint) {
        self.mount_points.push(mount_point);
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
                    kind: node.kind,
                    body: e.to_string(),
                }
            })?;

            // Check that all members are unique.
            {
                let mut unique_targets = BTreeSet::new();
                for target in node.targets.iter() {
                    if !unique_targets.insert(target) {
                        return Err(BlockDeviceGraphBuildError::DuplicateTargetId {
                            node_id: node.id,
                            kind: node.kind,
                            target_id: target.clone(),
                        });
                    }
                }
            }

            // Check that we have a valid number of members.
            {
                let valid_cardinality = node.kind.as_blkdev_referrer().valid_target_count();
                let target_count = node.targets.len();

                if !valid_cardinality.contains(target_count) {
                    return Err(BlockDeviceGraphBuildError::InvalidTargetCount {
                        node_id: node.id,
                        kind: node.kind,
                        target_count,
                        expected: valid_cardinality,
                    });
                }
            }

            // Then check each member individually.
            for target in node.targets.iter() {
                // Try to get a mutable reference to the member node on the map.
                let target_node = nodes.get_mut(target);

                // Ensure that the member node exists.
                if target_node.is_none() {
                    return Err(BlockDeviceGraphBuildError::NonExistentReference {
                        node_id: node.id,
                        kind: node.kind,
                        target_id: target.clone(),
                    });
                }

                // Unwrap the target node since we know it exists.
                let target_node = target_node.unwrap();

                // Get the valid references for the current node.
                let valid_references = node.kind.as_blkdev_referrer().valid_target_kinds();

                // Check that the target is of a valid kind.
                if !valid_references.contains(target_node.kind.as_flag()) {
                    return Err(BlockDeviceGraphBuildError::InvalidReferenceKind {
                        node_id: node.id,
                        kind: node.kind,
                        target_id: target.clone(),
                        target_kind: target_node.kind,
                        valid_references,
                    });
                }

                // Add the current node as a dependent of the target node.
                // This will be further checked later once we build the full graph.
                target_node.dependents.push(node.id.clone());
            }
        }

        // Check that all images are valid.
        for image in self.images.iter() {
            // Try to get target the node from the map.
            let node = nodes.get_mut(&image.target_id);

            // Ensure that the target node exists.
            if node.is_none() {
                return Err(BlockDeviceGraphBuildError::ImageNonExistentReference {
                    image_id: image.url.clone(),
                    target_id: image.target_id.clone(),
                });
            }

            // Unwrap the node since we know it exists.
            let node = node.unwrap();

            // Depending on the image format, we can have different referrer kinds.
            // The implementation of this `From` trait lives in `conversions.rs`.
            let valid_references = BlkDevReferrerKind::from(*image).valid_target_kinds();

            // Check that the node is of a valid kind.
            if !valid_references.contains(node.kind.as_flag()) {
                return Err(BlockDeviceGraphBuildError::ImageInvalidReference {
                    image_id: image.url.clone(), // The image's path.
                    target_id: node.id.clone(),  // The node being referenced.
                    target_kind: node.kind,      // The node's kind.
                    valid_references,            // The valid kinds of nodes for an image.
                });
            }

            if let Some(other_image) = node.image {
                return Err(BlockDeviceGraphBuildError::ImageReferenceAlreadyImaging {
                    image_id: image.url.clone(),             // The image's path.
                    target_id: node.id.clone(),              // The node being referenced.
                    other_image_id: other_image.url.clone(), // The other image that references this target.
                });
            }

            // Set the node's image
            node.image = Some(image);
        }

        // Check that all mount points are unique
        {
            let mut unique_mount_points = BTreeSet::new();
            for mount_point in self.mount_points.iter() {
                // Swap is a special case since it is not a real mount point and tghere can be many of them.
                // The `none` case is also explicitly skipped. Note that currently these are the same.
                if mount_point.path.as_os_str() == SWAP_MOUNT_POINT
                    || mount_point.path.as_os_str() == NONE_MOUNT_POINT
                {
                    continue;
                }

                if !unique_mount_points.insert(mount_point.path.clone()) {
                    return Err(BlockDeviceGraphBuildError::DuplicateMountPoint(
                        mount_point.path.to_string_lossy().into(),
                    ));
                }
            }
        }

        // Check that all mount points are valid
        for mount_point in self.mount_points.iter() {
            // Try to get the target node from the map
            let node = nodes.get_mut(&mount_point.target_id);

            // Ensure that the target node exists
            if node.is_none() {
                return Err(BlockDeviceGraphBuildError::MountPointNonExistentReference {
                    mount_point: mount_point.path.to_string_lossy().into(),
                    target_id: mount_point.target_id.clone(),
                });
            }

            // Unwrap the node since we know it exists
            let node = node.unwrap();

            // Check that the node is of a valid kind
            if !BlkDevReferrerKind::MountPoint
                .valid_target_kinds()
                .contains(node.kind.as_flag())
            {
                return Err(BlockDeviceGraphBuildError::MountPointInvalidReference {
                    mount_point: mount_point.path.to_string_lossy().into(), // The mount point's path
                    target_id: node.id.clone(), // The node being referenced
                    target_kind: node.kind,     // The node's kind
                    valid_references: BlkDevReferrerKind::MountPoint.valid_target_kinds(), // The valid kinds of nodes for a mount point
                });
            }

            // Ensure the mount point is valid
            if !(mount_point.path.is_absolute()
                || VALID_NON_PATH_MOUNT_POINTS
                    .iter()
                    .any(|p| p == &mount_point.path.as_os_str()))
            {
                return Err(BlockDeviceGraphBuildError::InvalidMountPoint {
                    mount_point: mount_point.path.to_string_lossy().into(),
                    valid_mount_points: VALID_NON_PATH_MOUNT_POINTS.join(", "), // Stringified list of other valid mount points
                });
            }

            // Add the mount point to the node
            node.mount_points.push(mount_point);
        }

        // Check all dependents for sharing compatibility
        for (_, node) in nodes.iter() {
            let dependents = node
                .dependents
                .iter()
                .map(|id| {
                    nodes
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
                        node.kind,
                        &dependent_a.id,
                        dependent_a.kind.as_blkdev_referrer(),
                        &dependent_b.id,
                        dependent_b.kind.as_blkdev_referrer(),
                    )?;
                }
            }

            // Check that nodes being imaged are not shared with other referrers of invalid kind.
            if let Some(image) = node.image {
                // Check image and dependents sharing
                for dependent in dependents.iter() {
                    check_mutual_sharing_peers(
                        &node.id,
                        node.kind,
                        &image.url,
                        BlkDevReferrerKind::from(image),
                        &dependent.id,
                        dependent.kind.as_blkdev_referrer(),
                    )?;
                }

                // Check image and mount point sharing
                for mount_point in node.mount_points.iter() {
                    check_mutual_sharing_peers(
                        &node.id,
                        node.kind,
                        &image.url,
                        BlkDevReferrerKind::from(image),
                        mount_point.path.to_string_lossy(),
                        BlkDevReferrerKind::MountPoint,
                    )?;
                }
            }

            // Check that nodes with mount points are not shared with other referrers of invalid kind.
            for mount_point in node.mount_points.iter() {
                // Check mount point and dependents sharing
                for dependent in dependents.iter() {
                    check_mutual_sharing_peers(
                        &node.id,
                        node.kind,
                        mount_point.path.to_string_lossy(),
                        BlkDevReferrerKind::MountPoint,
                        &dependent.id,
                        dependent.kind.as_blkdev_referrer(),
                    )?;
                }
            }
        }

        // Check unique field values requirements
        {
            let mut unique_fields: HashMap<
                BlkDevKind,
                HashMap<&'static str, HashMap<&[u8], &str>>,
            > = HashMap::new();
            for (id, node) in nodes.iter() {
                if let Some(uniqueness_constraint) = node.host_config_ref.uniqueness_constraints() {
                    let kind = node.host_config_ref.kind();
                    for (field_name, field_value) in uniqueness_constraint {
                        if let Some(other_id) = unique_fields
                            .entry(kind)
                            .or_default()
                            .entry(field_name)
                            .or_default()
                            .insert(field_value, id)
                        {
                            return Err(BlockDeviceGraphBuildError::UniqueFieldConstraintError {
                                node_id: id.clone(),
                                other_id: other_id.into(),
                                kind,
                                field_name: field_name.into(),
                                value: String::from_utf8_lossy(field_value).into(),
                            });
                        }
                    }
                }
            }
        }

        // Build the graph structure
        let graph = BlockDeviceGraph { nodes };

        // Check targets for each node
        for node in graph.nodes.values().filter(|n| !n.targets.is_empty()) {
            // This should never fail, since we already checked that all targets exist.
            let targets =
                graph
                    .targets(&node.id)
                    .ok_or(BlockDeviceGraphBuildError::InternalError {
                        body: format!(
                            "Failed to get targets for node '{}' of kind '{}'",
                            node.id, node.kind
                        ),
                    })?;

            node.kind
                .as_blkdev_referrer()
                .check_targets(node, &targets, &graph)
                .map_err(|e| BlockDeviceGraphBuildError::InvalidTargets {
                    node_id: node.id.clone(),
                    kind: node.kind,
                    body: format!("{:#}", e),
                })?;
        }

        Ok(graph)
    }
}

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
