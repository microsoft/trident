use petgraph::{visit::IntoNodeReferences, Direction};

use crate::storage_graph::{
    error::StorageGraphBuildError,
    graph::StoragePetgraph,
    node::{BlockDevice, StorageGraphNode},
    types::HostConfigBlockDevice,
};

/// Check that any referrer of a RAID array allows for the configured level of said RAID array.
///
/// E.g.: If ESP filesystems, when on RAID, can only use RAID 1, then a referrer
/// that is an ESP filesystem must only refer to RAID arrays that are RAID 1.
pub(super) fn check_raid_levels(graph: &StoragePetgraph) -> Result<(), StorageGraphBuildError> {
    // Call `node_weights()` to iterate over all node objects.
    for (node_idx, node) in graph.node_references() {
        let Some(allowed_raid_levels) = node.referrer_kind().allowed_raid_levels() else {
            // Skip nodes that do not have any RAID level constraints.
            continue;
        };

        // Iterate over all references in this node.
        for referred_node in graph
            .neighbors_directed(node_idx, Direction::Outgoing)
            .map(|r| &graph[r])
        {
            // Only process RAID arrays here.
            let StorageGraphNode::BlockDevice(BlockDevice {
                id,
                host_config_ref: HostConfigBlockDevice::RaidArray(raid_array),
            }) = referred_node
            else {
                continue;
            };

            // Check if the RAID level of the referrer is allowed by the RAID array.
            if !allowed_raid_levels.contains(raid_array.level) {
                return Err(StorageGraphBuildError::InvalidRaidlevel {
                    node_identifier: node.identifier(),
                    kind: node.referrer_kind(),
                    raid_id: id.clone(),
                    raid_level: raid_array.level,
                    valid_levels: allowed_raid_levels,
                });
            }
        }
    }

    Ok(())
}
