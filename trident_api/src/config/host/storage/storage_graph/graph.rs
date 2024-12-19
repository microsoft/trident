use petgraph::{csr::DefaultIx, graph::NodeIndex as PetgraphNodeIndex, Directed, Graph};

use super::{node::StorageGraphNode, references::ReferenceKind};

/// The type of the node index used in the StorageGraph.
pub(super) type NodeIndex = PetgraphNodeIndex<DefaultIx>;

/// The type of the graph used to store block devices and their relationships.
pub(super) type StoragePetgraph = Graph<StorageGraphNode, ReferenceKind, Directed, DefaultIx>;

#[derive(Debug, Clone, Default)]
pub struct StorageGraph {
    pub inner: StoragePetgraph,
}
