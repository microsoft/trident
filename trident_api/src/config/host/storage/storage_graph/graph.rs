use std::path::Path;

use petgraph::{csr::DefaultIx, graph::NodeIndex as PetgraphNodeIndex, Directed, Graph};

use crate::config::FileSystemSource;

use super::{node::StorageGraphNode, references::ReferenceKind};

/// The type of the node index used in the StorageGraph.
pub(super) type NodeIndex = PetgraphNodeIndex<DefaultIx>;

/// The type of the graph used to store block devices and their relationships.
pub(super) type StoragePetgraph = Graph<StorageGraphNode, ReferenceKind, Directed, DefaultIx>;

#[derive(Debug, Clone, Default)]
pub struct StorageGraph {
    pub inner: StoragePetgraph,
}

impl StorageGraph {
    /// Check if a volume is present and backed by an image.
    ///
    /// A volume is a file system on a specific mount point.
    pub(crate) fn get_volume_status(&self, mnt_point: impl AsRef<Path>) -> VolumeStatus {
        self.inner
            .node_weights()
            .filter_map(|node| node.as_filesystem())
            .find(|fs| {
                fs.mount_point
                    .as_ref()
                    .map(|mp| mp.path == mnt_point.as_ref())
                    .unwrap_or_default()
            })
            .map(|fs| match fs.source {
                FileSystemSource::Image(_)
                | FileSystemSource::EspImage(_)
                | FileSystemSource::OsImage => VolumeStatus::PresentAndBackedByImage,
                FileSystemSource::Adopted => VolumeStatus::PresentAndBackedByAdoptedFs,
                FileSystemSource::Create => VolumeStatus::PresentButNotBackedByImage,
            })
            .unwrap_or(VolumeStatus::NotPresent)
    }
}

/// Helper enum to report the status of volumes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VolumeStatus {
    /// The volume is present and backed by an image
    PresentAndBackedByImage,

    /// The volume is present and backed by an adopted filesystem
    PresentAndBackedByAdoptedFs,

    /// The volume is present but not backed by an image
    PresentButNotBackedByImage,

    /// The volume is not present
    NotPresent,
}
