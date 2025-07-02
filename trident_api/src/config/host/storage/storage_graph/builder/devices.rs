use std::collections::HashMap;

use anyhow::Context;
use log::trace;

use crate::config::host::storage::storage_graph::{
    error::StorageGraphBuildError, graph::StoragePetgraph, node::StorageGraphNode,
    types::BlkDevKind,
};

/// Checks basic properties of each block device.
pub(super) fn check_block_devices(graph: &StoragePetgraph) -> Result<(), StorageGraphBuildError> {
    // Call `node_weights()` to iterate over all node objects.
    for node in graph.node_weights() {
        // Only process block devices here
        let StorageGraphNode::BlockDevice(dev) = &node else {
            continue;
        };

        // Perform basic checks on block device nodes.
        dev.host_config_ref.basic_check().map_err(|e| {
            StorageGraphBuildError::BasicCheckFailed {
                node_id: dev.id.clone(),
                kind: node.device_kind(),
                body: e.to_string(),
            }
        })?;
    }

    Ok(())
}

/// Checks the unique field constraints for block devices as defined by the
/// `uniqueness_constraints()` rule function.
///
/// For example, this is the function that ensures that all encrypted volumes
/// have unique names.
pub(super) fn check_unique_fields(graph: &StoragePetgraph) -> Result<(), StorageGraphBuildError> {
    // Create a hash map to keep track of field uniqueness.
    let mut unique_fields: HashMap<BlkDevKind, HashMap<&'static str, HashMap<&[u8], &str>>> =
        HashMap::new();

    // Call `node_weights()` to iterate over all node objects.
    for node in graph.node_weights() {
        // Only process block devices here
        let StorageGraphNode::BlockDevice(dev) = &node else {
            continue;
        };

        let Some(uniqueness_constraint) = dev.kind().uniqueness_constraints() else {
            // Skip nodes that do not have any uniqueness constraints.
            continue;
        };

        trace!(
            "Checking uniqueness constraints for block device '{}' of kind '{}'.",
            dev.id,
            dev.kind()
        );

        for (field_name, extractor) in uniqueness_constraint {
            let field_value = extractor(&dev.host_config_ref) // Add some context about what we were doing.
                .with_context(|| {
                    format!(
                        "Failed to extract field '{}' from node '{}' of kind '{}'",
                        field_name,
                        dev.id,
                        dev.kind()
                    )
                })
                // Map the error to an internal error, this should never happen.
                .map_err(|err| StorageGraphBuildError::InternalError {
                    body: format!("{err:?}"),
                })?;

            let Some(field_value) = field_value else {
                // Skip nodes that do not have a value for this field.
                continue;
            };

            // Check if the field value is unique
            if let Some(other_id) = unique_fields
                // First get the map for the kind of block device
                .entry(dev.kind())
                .or_default()
                // Then get the map for the field name
                .entry(field_name)
                .or_default()
                // Finally, try to insert the field value
                .insert(field_value, &dev.id)
            {
                // If we got here, another node of the same kind had
                // the same value for this field :(
                return Err(StorageGraphBuildError::UniqueFieldConstraintError {
                    node_id: dev.id.clone(),
                    other_id: other_id.into(),
                    kind: dev.kind(),
                    field_name: field_name.into(),
                    value: String::from_utf8_lossy(field_value).into(),
                });
            }
        }
    }

    Ok(())
}
