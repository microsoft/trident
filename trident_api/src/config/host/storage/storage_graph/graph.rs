use std::path::Path;

use petgraph::{
    csr::DefaultIx, graph::NodeIndex as PetgraphNodeIndex, visit::IntoNodeReferences, Directed,
    Graph,
};

use crate::{config::FileSystemSource, constants::ROOT_MOUNT_POINT_PATH};

use super::{node::StorageGraphNode, references::ReferenceKind, types::BlkDevKind};

/// The type of the node index used in the StorageGraph.
pub(super) type NodeIndex = PetgraphNodeIndex<DefaultIx>;

/// The type of the graph used to store block devices and their relationships.
pub(super) type StoragePetgraph = Graph<StorageGraphNode, ReferenceKind, Directed, DefaultIx>;

#[derive(Debug, Clone, Default)]
pub struct StorageGraph {
    pub(super) inner: StoragePetgraph,
}

impl StorageGraph {
    /// Returns the node idex and a reference to the node of the root
    /// filesystem.
    #[allow(dead_code)]
    fn root_fs_node(&self) -> Option<(NodeIndex, &StorageGraphNode)> {
        // Iterate over all nodes. Find the first filesystem or verity
        // filesystem node that is mounted on the root mount point.
        self.inner.node_references().find(|(_, node)| {
            // Go over all filesystems.
            node.as_filesystem().map_or(false, |fs| {
                // Check if the filesystem is the root filesystem.
                fs.mount_point
                    .as_ref()
                    .map_or(false, |mp| mp.path == Path::new(ROOT_MOUNT_POINT_PATH))
            })
            // OR, go over all verity filesystems.
            || node
                .as_verity_filesystem()
                .map_or(false, |vfs| {
                    // Check if the verity filesystem is the root filesystem.
                    vfs.mount_point.path == Path::new(ROOT_MOUNT_POINT_PATH)})
        })
    }

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

    /// Returns whether the root filesystem is on a verity device.
    #[allow(dead_code)]
    pub fn root_fs_is_verity(&self) -> bool {
        let Some((rootfs_idx, root_fs_node)) = self.root_fs_node() else {
            return false;
        };

        // Return true for verity filesystems, nothing else to check.
        if root_fs_node.as_verity_filesystem().is_some() {
            return true;
        }

        // Check if the root filesystem is directly on a verity device.
        self.inner
            .neighbors_directed(rootfs_idx, petgraph::EdgeDirection::Outgoing)
            .any(|neighbor_idx| self.inner[neighbor_idx].device_kind() == BlkDevKind::VerityDevice)
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    use crate::{
        config::{
            FileSystem, FileSystemType, Image, ImageFormat, ImageSha256, MountPoint, Partition,
            PartitionType, VerityDevice, VerityFileSystem,
        },
        storage_graph::{node::BlockDevice, types::HostConfigBlockDevice},
    };

    #[test]
    fn test_root_fs_node() {
        let mut graph = StorageGraph::default();

        // Assert that the root filesystem is not found in an empty graph.
        assert_eq!(graph.root_fs_node(), None);

        // Add a filesystem node that is not the root filesystem.
        let fs_node = StorageGraphNode::FileSystem(FileSystem {
            fs_type: FileSystemType::Ext4,
            device_id: Some("fs1".into()),
            mount_point: Some(MountPoint::from_str("/mnt/fs1").unwrap()),
            source: FileSystemSource::Create,
        });
        let _ = graph.inner.add_node(fs_node);

        // Assert that the root filesystem is not found when the only filesystem
        // node is not the root filesystem.
        assert_eq!(graph.root_fs_node(), None);

        // Add a filesystem node that is the root filesystem.
        let root_fs_node = StorageGraphNode::FileSystem(FileSystem {
            fs_type: FileSystemType::Ext4,
            device_id: Some("rootfs".into()),
            mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
            source: FileSystemSource::Create,
        });
        let root_fs_node_idx = graph.inner.add_node(root_fs_node.clone());

        // Assert that the root filesystem is found when it is the only filesystem
        // node.
        assert_eq!(
            graph.root_fs_node(),
            Some((root_fs_node_idx, &root_fs_node))
        );

        // Remove the root filesystem node.
        graph.inner.remove_node(root_fs_node_idx);

        // Assert that the root filesystem is not found when it is removed.
        assert_eq!(graph.root_fs_node(), None);

        // Add a verity filesystem node that is the root filesystem.
        let verity_fs_node = StorageGraphNode::VerityFileSystem(VerityFileSystem {
            name: "rootfs".into(),
            data_device_id: "data".into(),
            hash_device_id: "hash".into(),
            data_image: Image {
                url: "http://example.com/data.img".into(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://example.com/hash.img".into(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            fs_type: FileSystemType::Ext4,
            mount_point: MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap(),
        });
        let verity_fs_node_idx = graph.inner.add_node(verity_fs_node.clone());

        // Assert that the root filesystem is found when it is the only verity
        // filesystem node.
        assert_eq!(
            graph.root_fs_node(),
            Some((verity_fs_node_idx, &verity_fs_node))
        );

        // Remove the verity filesystem node.
        graph.inner.remove_node(verity_fs_node_idx);

        // Assert that the root filesystem is not found when it is removed.
        assert_eq!(graph.root_fs_node(), None);
    }

    #[test]
    fn test_root_fs_is_verity() {
        let mut graph = StorageGraph::default();

        // Assert that the root filesystem is not on a verity device in an empty graph.
        assert!(!graph.root_fs_is_verity());

        // ==== VERITY FS ====

        // Add a verity filesystem node that is the root filesystem.
        let verity_fs_node = StorageGraphNode::VerityFileSystem(VerityFileSystem {
            name: "rootfs".into(),
            data_device_id: "data".into(),
            hash_device_id: "hash".into(),
            data_image: Image {
                url: "http://example.com/data.img".into(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://example.com/hash.img".into(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            fs_type: FileSystemType::Ext4,
            mount_point: MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap(),
        });
        let verity_fs_node_idx = graph.inner.add_node(verity_fs_node.clone());

        // Assert that the root filesystem is on a verity device when it is the only
        // verity filesystem node.
        assert!(graph.root_fs_is_verity());

        // Remove the verity filesystem node.
        graph.inner.remove_node(verity_fs_node_idx);

        // Assert that the root filesystem is not on a verity device when it is removed.
        assert!(!graph.root_fs_is_verity());

        // Add a filesystem node that is the root filesystem.
        let root_fs_node = StorageGraphNode::FileSystem(FileSystem {
            fs_type: FileSystemType::Ext4,
            device_id: Some("rootfs".into()),
            mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
            source: FileSystemSource::Create,
        });
        let _ = graph.inner.add_node(root_fs_node.clone());

        // Assert that the root filesystem is not on a verity device when it is the
        // only filesystem node.
        assert!(!graph.root_fs_is_verity());

        // ==== Verity Dev ====

        let mut graph = StorageGraph::default();
        let block_dev_id = "rootfs";

        // Add a root filesystem node.
        let root_fs_node = StorageGraphNode::FileSystem(FileSystem {
            fs_type: FileSystemType::Ext4,
            device_id: Some(block_dev_id.into()),
            mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
            source: FileSystemSource::Create,
        });

        let root_fs_node_idx = graph.inner.add_node(root_fs_node.clone());

        // Add a verity device to back the root filesystem.
        let backing_node_idx = graph
            .inner
            .add_node(StorageGraphNode::BlockDevice(BlockDevice {
                id: block_dev_id.into(),
                host_config_ref: HostConfigBlockDevice::VerityDevice(VerityDevice {
                    id: block_dev_id.into(),
                    name: "myVerityDevice".into(),
                    data_device_id: "data".into(),
                    hash_device_id: "hash".into(),
                    ..Default::default()
                }),
            }));

        // Add an edge from the partition to the root filesystem.
        graph
            .inner
            .add_edge(root_fs_node_idx, backing_node_idx, ReferenceKind::Regular);

        // Assert that the root filesystem is on a verity device when it is directly
        // on a verity device.
        assert!(graph.root_fs_is_verity());

        // Now change the backing node to a non-verity device.
        graph.inner[backing_node_idx] = StorageGraphNode::BlockDevice(BlockDevice {
            id: block_dev_id.into(),
            host_config_ref: HostConfigBlockDevice::Partition(Partition {
                id: block_dev_id.into(),
                partition_type: PartitionType::Root,
                size: "1G".parse().unwrap(),
            }),
        });

        // Assert that the root filesystem is not on a verity device when it is
        // directly on a non-verity device.
        assert!(!graph.root_fs_is_verity());
    }
}
