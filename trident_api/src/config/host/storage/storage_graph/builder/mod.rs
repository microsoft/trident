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

use std::collections::{BTreeMap, BTreeSet};

use log::{debug, trace};
use petgraph::visit::{EdgeRef, IntoNodeReferences};

use crate::BlockDeviceId;

use super::{
    error::StorageGraphBuildError,
    graph::{NodeIndex, StorageGraph, StoragePetgraph},
    node::StorageGraphNode,
    references::ReferenceKind,
};

mod devices;
mod filesystems;
mod partition;
mod raid;
mod relationships;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct StorageGraphBuilder {
    nodes: Vec<StorageGraphNode>,
}

impl StorageGraphBuilder {
    /// Adds a new block device node to the graph.
    pub(crate) fn add_node(&mut self, node: StorageGraphNode) {
        self.nodes.push(node);
    }

    /// Builds the block device graph.
    ///
    /// This function will check that all nodes, their references, mount points,
    /// and images are valid.
    ///
    /// It will also check that no block device is referenced by more than one
    /// other block device, enforcing exclusive ownership of block devices.
    pub(crate) fn build(self) -> Result<StorageGraph, StorageGraphBuildError> {
        debug!("Building storage graph");
        // Populate the graph with all nodes and check that all block devices have unique IDs.
        let (mut graph, node_id_index_map) = populate_graph_nodes(self.nodes)?;

        // Check basic properties of each block device.
        trace!("Checking block devices");
        devices::check_block_devices(&graph)?;

        // Check unique field values requirements for block devices.
        devices::check_unique_fields(&graph)?;

        // Check all filesystems and ensure mount points are unique.
        trace!("Checking filesystems");
        filesystems::check_filesystems(&graph)?;

        // Check that all nodes and their references are valid and add all
        // correct edges to the graph.
        trace!("Populating edges");
        populate_graph_edges(&mut graph, &node_id_index_map)?;

        // Shadow the graph to make it immutable after it's been fully built.
        let graph = graph;

        // Log graph structure
        trace!("Built storage graph structure:\n{}", describe_graph(&graph));

        // Check graph for sharing compatibility.
        trace!("Checking sharing");
        relationships::check_sharing(&graph)?;

        // Check that all nodes & filesystems have dependents of the same block
        // device kind when required
        trace!("Checking dependency kind homogeneity");
        relationships::check_dependency_kind_homogeneity(&graph)?;

        // Check that all filesystems on verity devices have correct types
        relationships::check_filesystems_on_verity(&graph)?;

        // Check partition size homogeneity
        trace!("Checking partition size homogeneity");
        partition::check_partition_size_homogeneity(&graph)?;

        // Check partition type homogeneity
        trace!("Checking partition type homogeneity");
        partition::check_partition_types(&graph)?;

        // Check that verity devices have congruent partition types
        trace!("Checking verity partition types");
        partition::check_verity_partition_types(&graph)?;

        // Check RAID levels
        trace!("Checking RAID levels");
        raid::check_raid_levels(&graph)?;

        // Additional checks
        trace!("Checking targets");
        check_targets(&graph)?;

        // TODO: the current rules make this impossible, but we should
        // eventually check for cycles if there are any risks of one arising.
        debug!(
            "Storage graph built successfully with {} nodes and {} edges",
            graph.node_count(),
            graph.edge_count()
        );
        Ok(StorageGraph { inner: graph })
    }
}

/// Populates a basic graph with all nodes.
///
/// Node ID uniqueness for block devices is checked here.
///
/// Edges are NOT added at this stage.
fn populate_graph_nodes(
    nodes: Vec<StorageGraphNode>,
) -> Result<(StoragePetgraph, BTreeMap<BlockDeviceId, NodeIndex>), StorageGraphBuildError> {
    // The inner graph type. Estimate the capacity based on the number of nodes.
    let mut graph = StoragePetgraph::with_capacity(nodes.len(), nodes.len());

    // Create a map of BlockDeviceId->NodeIndex to store the indices while building the graph.
    let mut node_id_index_map: BTreeMap<BlockDeviceId, NodeIndex> = BTreeMap::new();

    // First, add all the nodes to the graph.
    for node in nodes {
        let id = node.id().cloned();

        // If the node has an ID, check that it is unique.
        if let Some(id) = &id {
            if node_id_index_map.contains_key(id) {
                return Err(StorageGraphBuildError::DuplicateDeviceId(id.clone()));
            }
        }

        // Add the node to the graph.
        trace!("Adding node: {}", node.describe());
        let idx = graph.add_node(node);

        // If the node has an ID, store the index in the map.
        if let Some(id) = id {
            node_id_index_map.insert(id, idx);
        }
    }

    Ok((graph, node_id_index_map))
}

/// Checks that all immediate references are valid and adds them to the graph.
fn populate_graph_edges(
    graph: &mut StoragePetgraph,
    node_id_index_map: &BTreeMap<BlockDeviceId, NodeIndex>,
) -> Result<(), StorageGraphBuildError> {
    // To avoid borrowing issues, we will collect all edges in a vec first.
    let mut edges: Vec<(NodeIndex, NodeIndex, ReferenceKind)> = Vec::new();

    // Iterate over all nodes and check their references.
    for (node_idx, node) in graph.node_references() {
        trace!("Checking references for {}", node.describe());

        // Get the list of references made by the current node.
        let references = node.references();

        // Check that all references have unique targets.
        {
            let mut unique_targets = BTreeSet::new();
            for reference in references.iter() {
                if !unique_targets.insert(reference.id()) {
                    return Err(StorageGraphBuildError::DuplicateTargetId {
                        node_identifier: node.identifier(),
                        kind: node.referrer_kind(),
                        target_id: reference.id().to_string(),
                    });
                }
            }
        }

        // Check that we have a valid number of members.
        {
            let valid_cardinality = node.referrer_kind().valid_target_count();
            let target_count = references.len();

            if !valid_cardinality.contains(target_count) {
                return Err(StorageGraphBuildError::InvalidTargetCount {
                    node_identifier: node.identifier(),
                    kind: node.referrer_kind(),
                    target_count,
                    expected: valid_cardinality,
                });
            }
        }

        // Get the list of kinds compatible with the current node.
        let compatible_kinds = node.referrer_kind().compatible_kinds();

        // Then check each member individually.
        for reference in references.iter() {
            // Try to find the target node in the map.
            let target_idx = *node_id_index_map.get(reference.id()).ok_or_else(|| {
                StorageGraphBuildError::NonExistentReference {
                    node_identifier: node.identifier(),
                    kind: node.referrer_kind(),
                    target_id: reference.id().to_string(),
                }
            })?;

            // Now get the node from the graph.
            let target = &graph[target_idx];

            // Check that the target is of a compatible kind for the referrer kind.
            if !compatible_kinds.contains(target.device_kind().as_flag()) {
                return Err(StorageGraphBuildError::InvalidReferenceKind {
                    node_identifier: node.identifier(),
                    kind: node.referrer_kind(),
                    target_id: reference.id().to_string(),
                    target_kind: target.device_kind(),
                    valid_references: compatible_kinds,
                });
            }

            // Check that the target is of a compatible kind for this specific reference kind.
            if let Some((ref_kind, reference_compatible_kinds)) =
                reference.kind().is_special_then(|k| k.compatible_kinds())
            {
                if !reference_compatible_kinds.contains(target.device_kind().as_flag()) {
                    return Err(StorageGraphBuildError::InvalidSpecialReferenceKind {
                        node_identifier: node.identifier(),
                        kind: node.referrer_kind(),
                        target_id: reference.id().to_string(),
                        target_kind: target.device_kind(),
                        valid_references: reference_compatible_kinds,
                        reference_kind: ref_kind,
                    });
                }
            }

            // All direct references are valid, add them to the list of edges to add.
            trace!(
                "Adding edge from {} to {} with kind [{}]",
                node.describe(),
                target.describe(),
                reference.kind()
            );
            edges.push((node_idx, target_idx, reference.kind()));
        }
    }

    // Now that we're done with checking basic references, we can add the edges.
    for (source, target, kind) in edges.into_iter() {
        graph.add_edge(source, target, kind);
    }

    Ok(())
}

/// Checks targets for each node.
fn check_targets(graph: &StoragePetgraph) -> Result<(), StorageGraphBuildError> {
    for (node_idx, node) in graph.node_references() {
        node.referrer_kind()
            .check_targets(node_idx, node, graph)
            .map_err(|e| StorageGraphBuildError::InvalidTargets {
                node_identifier: node.identifier(),
                kind: node.referrer_kind(),
                body: format!("{:#}", e),
            })?;
    }

    Ok(())
}

/// Returns a user-friendly description of the graph structure.
fn describe_graph(graph: &StoragePetgraph) -> String {
    let mut buf: Vec<String> = Vec::new();
    for (node_idx, node) in graph.node_references() {
        buf.push(format!("[{}] {}", node_idx.index(), node.describe()));
        for edge in graph.edges(node_idx) {
            let target = &graph[edge.target()];
            buf.push(format!(
                "  -> [{}] {} ({})",
                edge.target().index(),
                target.describe(),
                edge.weight() // The relationship kind
            ));
        }
    }

    buf.join("\n")
}
