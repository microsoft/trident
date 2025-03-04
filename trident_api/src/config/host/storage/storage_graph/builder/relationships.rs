use log::trace;
use petgraph::{
    visit::{Dfs, IntoNodeReferences, Walker},
    Direction,
};

use crate::{
    config::host::storage::storage_graph::{
        error::StorageGraphBuildError, graph::StoragePetgraph, node::StorageGraphNode,
    },
    storage_graph::types::BlkDevKind,
};

/// Checks all dependents for sharing compatibility.
pub(super) fn check_sharing(graph: &StoragePetgraph) -> Result<(), StorageGraphBuildError> {
    for (node_idx, node) in graph.node_references() {
        // Only process block devices here. File systems cannot be referred to,
        // so we skip them.
        let StorageGraphNode::BlockDevice(blk_dev) = node else {
            continue;
        };

        // Get a list of all referrers of this node, i.e. all nodes that refer to it.
        let referrers = graph
            .neighbors_directed(node_idx, Direction::Incoming)
            .map(|n| &graph[n])
            .collect::<Vec<_>>();

        // If there are no referrers or only one, we can skip this block device.
        if let 0..=1 = referrers.len() {
            continue;
        }

        trace!(
            "Checking sharing of block device '{}' across {} referrers.",
            blk_dev.id,
            referrers.len(),
        );

        // Good 'ol 1/2 n^2 loop to check all dependents for sharing
        // compatibility among each other.
        for (i, referrer_a) in referrers.iter().enumerate() {
            for referrer_b in referrers.iter().skip(i + 1) {
                // Get the valid sharing peers for both referrers
                let referrer_a_valid_sharing_peers =
                    referrer_a.referrer_kind().valid_sharing_peers();
                let referrer_b_valid_sharing_peers =
                    referrer_b.referrer_kind().valid_sharing_peers();

                // Check that both valid sharing peers contain each other
                if !(referrer_a_valid_sharing_peers.contains(referrer_b.referrer_kind().as_flag())
                    && referrer_b_valid_sharing_peers
                        .contains(referrer_a.referrer_kind().as_flag()))
                {
                    return Err(StorageGraphBuildError::ReferrerForbiddenSharing {
                        target_id: blk_dev.id.clone(),
                        target_kind: node.device_kind(),
                        referrer_a_id: referrer_a.identifier(),
                        referrer_a_kind: referrer_a.referrer_kind(),
                        referrer_b_id: referrer_b.identifier(),
                        referrer_b_kind: referrer_b.referrer_kind(),
                        referrer_a_valid_sharing_peers,
                        referrer_b_valid_sharing_peers,
                    });
                }
            }
        }
    }
    Ok(())
}

/// Checks all dependencies for kind homogeneity.
pub(super) fn check_dependency_kind_homogeneity(
    graph: &StoragePetgraph,
) -> Result<(), StorageGraphBuildError> {
    for (node_idx, node) in graph.node_references() {
        if !node.referrer_kind().enforce_homogeneous_reference_kinds() {
            // Nothing to do here, this node is allowed to have different kinds of
            // referrers.
            continue;
        }

        // Get a list of all references of this node, i.e. all nodes that this node refers to.
        let references = graph
            .neighbors_directed(node_idx, Direction::Outgoing)
            .map(|n| {
                // Get dependent node, we can safely unwrap here as we know the node
                // exists because we just got it from the graph.
                graph.node_weight(n).unwrap()
            })
            .collect::<Vec<_>>();

        // Try to get the type of the first reference, if there is any.
        let Some(first_kind) = references.first().map(|r| r.referrer_kind()) else {
            // Nothing to do here, this node has no references.
            continue;
        };

        // Check that all references have the same kind
        if !references.iter().all(|r| r.referrer_kind() == first_kind) {
            return Err(StorageGraphBuildError::ReferenceKindMismatch {
                node_identifier: node.identifier(),
                kind: node.referrer_kind(),
            });
        }
    }
    Ok(())
}

/// Checks whether specific filesystem types can exist on verity nodes
pub(super) fn check_filesystems_on_verity(
    graph: &StoragePetgraph,
) -> Result<(), StorageGraphBuildError> {
    // Iterate over all nodes that are filesystems and check their mount points.
    for (node_idx, node) in graph.node_references() {
        let Some(fs) = node.as_filesystem() else {
            continue;
        };

        if Dfs::new(graph, node_idx)
            .iter(graph)
            .any(|dep_idx| graph[dep_idx].device_kind() == BlkDevKind::VerityDevice)
            && !fs.fs_type.supports_verity()
        {
            return Err(StorageGraphBuildError::FilesystemVerityIncompatible {
                fs_desc: fs.description(),
                fs_type: fs.fs_type,
            });
        }
    }

    Ok(())
}
