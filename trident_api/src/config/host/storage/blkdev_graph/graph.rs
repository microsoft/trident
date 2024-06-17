use std::{collections::BTreeMap, path::Path};

use crate::{
    config::{FileSystem, Partition, PartitionSize, PartitionType},
    BlockDeviceId,
};

use super::{
    error::BlockDeviceGraphBuildError,
    partitions::PartitionAttributeList,
    types::{BlkDevNode, HostConfigBlockDevice},
};

#[derive(Debug, Clone, Default)]
pub struct BlockDeviceGraph<'a> {
    pub nodes: BTreeMap<BlockDeviceId, BlkDevNode<'a>>,
    pub deviceless_filesystems: Vec<&'a FileSystem>,
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

impl<'a> BlockDeviceGraph<'a> {
    /// Get a reference to a specific node
    pub fn get(&self, id: &BlockDeviceId) -> Option<&BlkDevNode<'a>> {
        self.nodes.get(id)
    }

    /// List all nodes
    pub fn list(&self) -> Vec<&BlkDevNode<'a>> {
        self.nodes.values().collect()
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

    /// Check if a volume is present and backed by an image
    pub(crate) fn get_volume_status(&self, mnt_point: impl AsRef<Path>) -> VolumeStatus {
        self.nodes
            .values()
            .filter_map(|node| node.filesystem)
            .find(|fs| {
                fs.mountpoint()
                    .map(|mp| mp.path == mnt_point.as_ref())
                    .unwrap_or_default()
            })
            .map(|fs| {
                if fs.is_image_backed() {
                    VolumeStatus::PresentAndBackedByImage
                } else if fs.is_adopted_backed() {
                    VolumeStatus::PresentAndBackedByAdoptedFs
                } else {
                    VolumeStatus::PresentButNotBackedByImage
                }
            })
            .unwrap_or(VolumeStatus::NotPresent)
    }

    /// Get the partition type of a node
    /// For base nodes, the partition type is the partition type of the node itself when it is a partition, None otherwise.
    /// For composite nodes, such as RAID arrays, the partition type is the list of partition types of the targets.
    pub(super) fn get_partition_type(
        &self,
        target_node: &str,
    ) -> Result<Option<PartitionAttributeList<PartitionType>>, BlockDeviceGraphBuildError> {
        self.get_partition_data(target_node, |p| p.partition_type)
    }

    /// Get partition sizes from a node
    /// For base nodes, the size is the size of the node itself when it is a partition, None otherwise.
    /// For composite nodes, such as RAID arrays, the size is the list of sizes of the targets.
    pub(super) fn get_partition_sizes(
        &self,
        target_node: &str,
    ) -> Result<Option<PartitionAttributeList<PartitionSize>>, BlockDeviceGraphBuildError> {
        self.get_partition_data(target_node, |p| p.size)
    }

    /// Get generic partition data from a node
    /// For base nodes, the data is the data of the node itself when it is a partition, None otherwise.
    /// For composite nodes, such as RAID arrays, the data is the list of data of the targets.
    pub(super) fn get_partition_data<F, T>(
        &self,
        target_node: &str,
        func: F,
    ) -> Result<Option<PartitionAttributeList<T>>, BlockDeviceGraphBuildError>
    where
        F: FnOnce(&Partition) -> T + Copy,
    {
        let node =
            self.nodes
                .get(target_node)
                .ok_or(BlockDeviceGraphBuildError::InternalError {
                    body: format!(
                        "get_partition_data: Could not find node '{target_node}' in the graph"
                    ),
                })?;

        Ok(match node.host_config_ref {
            // Disks are not partitions
            HostConfigBlockDevice::Disk(_) => None,

            // Partitions are, well... partitions
            HostConfigBlockDevice::Partition(p) => {
                Some(PartitionAttributeList::new(&p.id, func(p)))
            }

            // We don't know the partition type of an adopted partition
            HostConfigBlockDevice::AdoptedPartition(_) => None,

            // Composite nodes
            HostConfigBlockDevice::RaidArray(_)
            | HostConfigBlockDevice::ABVolume(_)
            | HostConfigBlockDevice::EncryptedVolume(_) => {
                Some(
                    // These are composite nodes
                    // Make an iter of all the node's targets
                    node.targets
                        .iter()
                        // Do a recursive call to get_partition_data on each target
                        .map(|target| self.get_partition_data(target, func))
                        // Collect to turn
                        // Vec<Result<Option<PartitionDetailList<T>>>>
                        // into
                        // Result<Vec<Option<PartitionDetailList<T>>>>
                        .collect::<Result<
                            Vec<Option<PartitionAttributeList<T>>>,
                            BlockDeviceGraphBuildError,
                        >>()?
                        // Turn the vec into an iterator again
                        .into_iter()
                        // Flatten the iterator to get rid of the Option
                        .flatten()
                        // Flatten to turn Vec<PartitionDetailList<T>> into PartitionDetailList<T>
                        .flatten()
                        // Collect to turn the iterator into a Vec<PartitionDetails<T>>
                        .collect(),
                )
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::config::{
        host::storage::blkdev_graph::types::NodeFileSystem, FileSystemSource, FileSystemType,
        Image, ImageFormat, ImageSha256, MountOptions, MountPoint, Partition, PartitionType,
    };

    use super::*;

    #[test]
    fn test_check_volume_presence() {
        let mut nodes = BTreeMap::new();

        let part1 = Partition {
            id: "foo1".into(),
            partition_type: PartitionType::Root,
            size: crate::config::PartitionSize::Fixed(0.into()),
        };
        let mut node1 = BlkDevNode::from(&part1);

        let fs1 = FileSystem {
            device_id: Some("foo1".into()),
            fs_type: FileSystemType::Ext4,
            source: FileSystemSource::Image(Image {
                url: "http://foo".into(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            }),
            mount_point: Some(MountPoint {
                path: PathBuf::from("/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-123"),
                options: MountOptions::empty(),
            }),
        };
        node1.filesystem = Some(NodeFileSystem::Regular(&fs1));

        let part2 = Partition {
            id: "foo2".into(),
            partition_type: PartitionType::Root,
            size: crate::config::PartitionSize::Fixed(0.into()),
        };
        let mut node2 = BlkDevNode::from(&part2);

        let fs2 = FileSystem {
            device_id: Some("foo2".into()),
            fs_type: FileSystemType::Ext4,
            source: Default::default(),
            mount_point: Some(MountPoint {
                path: PathBuf::from("/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-456"),
                options: MountOptions::empty(),
            }),
        };
        node2.filesystem = Some(NodeFileSystem::Regular(&fs2));

        nodes.insert(node1.id.clone(), node1);
        nodes.insert(node2.id.clone(), node2);

        let graph = BlockDeviceGraph {
            nodes,
            ..Default::default()
        };

        // Exists and backed by image
        assert_eq!(
            graph.get_volume_status("/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-123"),
            VolumeStatus::PresentAndBackedByImage
        );

        // Exists but not backed by image
        assert_eq!(
            graph.get_volume_status("/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-456"),
            VolumeStatus::PresentButNotBackedByImage
        );

        // Does not exist
        assert_eq!(
            graph.get_volume_status("/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-789"),
            VolumeStatus::NotPresent,
        );
    }
}
