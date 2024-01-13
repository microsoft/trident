use std::collections::BTreeMap;

use crate::BlockDeviceId;

use super::types::BlkDevNode;

#[derive(Debug, Clone)]
pub struct BlockDeviceGraph<'a> {
    pub nodes: BTreeMap<BlockDeviceId, BlkDevNode<'a>>,
}

impl<'a> BlockDeviceGraph<'a> {
    /// Get a reference to a specific node
    pub fn get(&self, id: &BlockDeviceId) -> Option<&BlkDevNode<'a>> {
        self.nodes.get(id)
    }

    /// Get a list of references to the members of a specific node
    pub fn targets(&self, id: &BlockDeviceId) -> Option<Vec<&BlkDevNode<'_>>> {
        self.nodes
            .get(id)
            .map(|node| &node.targets)
            .and_then(|targets| {
                targets
                    .iter()
                    .map(|target| self.get(target))
                    .collect::<Option<Vec<&BlkDevNode<'a>>>>()
            })
    }
}
