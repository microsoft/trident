use std::path::Path;

use anyhow::{Context, Error};
use petgraph::{
    csr::DefaultIx,
    graph::NodeIndex as PetgraphNodeIndex,
    visit::{Dfs, EdgeRef, IntoNodeReferences, Reversed, Walker},
    Directed, Direction, Graph,
};

use crate::{
    config::{FileSystem, FileSystemSource, RaidLevel, VerityDevice},
    constants::{LUKS_HEADER_SIZE_IN_MIB, ROOT_MOUNT_POINT_PATH, USR_MOUNT_POINT_PATH},
    storage_graph::references::SpecialReferenceKind,
    BlockDeviceId,
};

use super::{
    node::{BlockDevice, StorageGraphNode},
    references::ReferenceKind,
    types::{BlkDevKind, HostConfigBlockDevice},
};

/// The type of the node index used in the StorageGraph.
pub(super) type NodeIndex = PetgraphNodeIndex<DefaultIx>;

/// The type of the graph used to store block devices and their relationships.
pub(super) type StoragePetgraph = Graph<StorageGraphNode, ReferenceKind, Directed, DefaultIx>;

#[derive(Debug, Clone, Default)]
pub struct StorageGraph {
    pub(super) inner: StoragePetgraph,
}

impl StorageGraph {
    /// Returns the node index and a reference to the node of the filesystem mounted at the exact given path.
    fn filesystem_node_by_mount_point(
        &self,
        mount_point: impl AsRef<Path>,
    ) -> Option<(NodeIndex, &StorageGraphNode)> {
        // Iterate over all nodes. Find the first filesystem node that is
        // mounted on the given path.
        self.inner.node_references().find(|(_, node)| {
            // Go over all filesystems.
            node.as_filesystem().is_some_and(|fs| {
                // Check if the filesystem is the root filesystem.
                fs.mount_point
                    .as_ref()
                    .is_some_and(|mp| mp.path == mount_point.as_ref())
            })
        })
    }

    /// Returns the node idex and a reference to the node of the root
    /// filesystem.
    fn root_fs_node(&self) -> Option<(NodeIndex, &StorageGraphNode)> {
        self.filesystem_node_by_mount_point(ROOT_MOUNT_POINT_PATH)
    }

    /// Returns the node index and a reference to the node with the given block device id.
    fn node_by_id(&self, device_id: &BlockDeviceId) -> Option<(NodeIndex, &StorageGraphNode)> {
        self.inner
            .node_references()
            .find(|(_, node)| node.id() == Some(device_id))
    }

    /// Check if a volume is present and backed by an image.
    ///
    /// A volume is a file system on a specific mount point.
    pub(crate) fn get_volume_status(&self, mnt_point: impl AsRef<Path>) -> VolumeStatus {
        if let Some(filesystem) = self
            .inner
            .node_weights()
            .filter_map(|node| node.as_filesystem())
            .find(|fs| {
                fs.mount_point
                    .as_ref()
                    .map(|mp| mp.path == mnt_point.as_ref())
                    .unwrap_or_default()
            })
        {
            match filesystem.source {
                FileSystemSource::Image => VolumeStatus::PresentAndBackedByImage,
                FileSystemSource::Adopted(_) => VolumeStatus::PresentAndBackedByAdoptedFs,
                FileSystemSource::New(_) => VolumeStatus::PresentButNotBackedByImage,
            }
        } else {
            VolumeStatus::NotPresent
        }
    }

    /// Returns whether the root filesystem is on a verity device.
    pub fn root_fs_is_verity(&self) -> bool {
        let Some((rootfs_idx, _)) = self.root_fs_node() else {
            return false;
        };

        self.backing_verity_device(rootfs_idx).is_some()
    }

    /// Returns whether the usr filesystem is on a verity device.
    pub fn usr_fs_is_verity(&self) -> bool {
        let Some((usrfs_idx, _)) = self.filesystem_node_by_mount_point(USR_MOUNT_POINT_PATH) else {
            return false;
        };

        self.backing_verity_device(usrfs_idx).is_some()
    }

    /// Returns whether a block device is an adopted partition OR sits on top of an
    /// adopted partition.
    pub fn is_adopted(&self, node_id: &BlockDeviceId) -> Option<bool> {
        let (idx, _) = self.node_by_id(node_id)?;

        // Do a DFS starting on the node to check if it, or any of its
        // dependencies, are adopted partitions.
        Some(
            Dfs::new(&self.inner, idx)
                .iter(&self.inner)
                .any(|idx| self.inner[idx].device_kind() == BlkDevKind::AdoptedPartition),
        )
    }

    /// Returns the first verity device found in the graph under the given node
    /// index in a DFS traversal, if any. Because the graph rules make it
    /// impossible to have multiple verity devices under the same node, this
    /// function, in practice, is a way to tell if a node is using verity and if
    /// so, get the verity device.
    fn backing_verity_device(&self, node_idx: NodeIndex) -> Option<(NodeIndex, &VerityDevice)> {
        Dfs::new(&self.inner, node_idx)
            .iter(&self.inner)
            .filter_map(|idx| {
                // When the node is a verity device, extract it and return it.
                let StorageGraphNode::BlockDevice(BlockDevice {
                    host_config_ref: HostConfigBlockDevice::VerityDevice(dev),
                    ..
                }) = &self.inner[idx]
                else {
                    return None;
                };

                Some((idx, dev))
            })
            .next()
    }

    /// Returns whether the block device with the given ID has dependents.
    pub fn has_dependents(&self, id: &BlockDeviceId) -> Result<bool, Error> {
        // First, find the node index of the block device with the given ID.
        let (node_idx, _) = self
            .inner
            .node_references()
            .find(|(_, node)| node.id() == Some(id))
            .with_context(|| format!("Block device '{id}' not found"))?;

        // Then, get the count of incoming edges to the block device node. An
        // outgoing edge represents a dependency, so incoming edges represent
        // dependents.
        Ok(self
            .inner
            .neighbors_directed(node_idx, Direction::Incoming)
            .count()
            > 0)
    }

    /// Returns whether the existing node is an A/B volume, or is on top of an
    /// A/B volume, meaning that it is capable of A/B updates.
    pub fn has_ab_capabilities(&self, node_id: &BlockDeviceId) -> Option<bool> {
        let (idx, _) = self.node_by_id(node_id)?;
        // Do a DFS starting on the node to check if it, or any of its
        // dependencies, are A/B volumes.
        Some(
            Dfs::new(&self.inner, idx)
                .iter(&self.inner)
                .any(|idx| self.inner[idx].device_kind() == BlkDevKind::ABVolume),
        )
    }

    /// Returns the estimated storage size of a block device, when possible.
    pub fn block_device_size(&self, node_id: &BlockDeviceId) -> Option<u64> {
        let (idx, _) = self.node_by_id(node_id)?;
        block_device_size(&self.inner, idx)
    }

    /// Returns the filesystem places on this block device, if any.
    pub fn filesystem_on_device(&self, node_id: &BlockDeviceId) -> Option<&FileSystem> {
        let (idx, _) = self.node_by_id(node_id)?;

        // Create a "reversed" graph to explore the incoming edges, then get a
        // list of filesystems ON this device.
        let reversed = Reversed(&self.inner);
        let filesystems = Dfs::new(&reversed, idx)
            .iter(&reversed)
            .filter_map(|idx| self.inner[idx].as_filesystem())
            .collect::<Vec<_>>();

        // The list should really have at most one filesystem, so anything else
        // is a None for now.
        if filesystems.len() == 1 {
            Some(filesystems[0])
        } else {
            None
        }
    }

    /// Returns an iterator over all filesystems that are placed on verity devices.
    pub fn filesystems_on_verity(&self) -> impl Iterator<Item = &FileSystem> {
        self.inner.node_references().filter_map(|(idx, node)| {
            // Ensure this node is a filesystem
            let fs = node.as_filesystem()?;

            // Ensure this filesystem is on a verity device
            self.backing_verity_device(idx).and(Some(fs))
        })
    }

    /// Returns the verity device, if any, on which the given filesystem is placed.
    pub fn verity_device_for_filesystem(
        &self,
        mount_path: impl AsRef<Path>,
    ) -> Option<&VerityDevice> {
        let (fs_idx, _) = self.filesystem_node_by_mount_point(mount_path)?;
        self.backing_verity_device(fs_idx).map(|(_, dev)| dev)
    }
}

/// For a given NodeIndex, find the first outgoing edge with the given
/// special reference kind and return the target node index.
pub(super) fn find_special_reference(
    graph: &StoragePetgraph,
    node: NodeIndex,
    reference_kind: SpecialReferenceKind,
) -> Option<NodeIndex> {
    graph
        .edges_directed(node, Direction::Outgoing)
        .find_map(|edge| {
            edge.weight()
                .is_special_and(|kind| kind == reference_kind)
                .then(|| edge.target())
        })
}

/// Returns the estimated storage size of a block device, when possible.
fn block_device_size(graph: &StoragePetgraph, idx: NodeIndex) -> Option<u64> {
    let StorageGraphNode::BlockDevice(dev) = &graph[idx] else {
        // For non-block devices, we report None.
        return None;
    };

    match &dev.host_config_ref {
        // For partitions we report the size, when available.
        HostConfigBlockDevice::Partition(part) => part.size.to_bytes(),

        // For verity, we report the size of the data device.
        HostConfigBlockDevice::VerityDevice(_) => block_device_size(
            graph,
            find_special_reference(graph, idx, SpecialReferenceKind::VerityDataDevice)?,
        ),

        // For A/B volumes, we report the size of either volume, as they should be the same.
        HostConfigBlockDevice::ABVolume(_) => {
            let volume_a_node_idx = graph.neighbors_directed(idx, Direction::Outgoing).next()?;
            block_device_size(graph, volume_a_node_idx)
        }

        // For RAID arrays, we assume all members are fo the same size, then
        // we determine the resulting size depending on the level.
        HostConfigBlockDevice::RaidArray(array) => {
            let member_node_idx = graph.neighbors_directed(idx, Direction::Outgoing).next()?;
            // Let N be the number of members.
            let member_count = graph.neighbors_directed(idx, Direction::Outgoing).count() as u64;

            // Let S be the size of a single member. All members are assumed to be of the same size.
            // Use '?' to propagate None if the size of the member is
            // unknown. (should not happen in RAID, but still...).
            let member_size = block_device_size(graph, member_node_idx)?;

            // Resulting size is determined by the RAID level. The changing
            // factor per level is the space efficiency (E). In general
            // terms, the resulting size is:
            //
            // Rs = S * N * E
            //
            // Source for space efficiency values:
            // https://en.wikipedia.org/wiki/Standard_RAID_levels
            Some(match array.level {
                // Block-level striping, so the size is the sum of all
                // members. E = 1.
                RaidLevel::Raid0 => member_size * member_count,

                // RAID1 is a mirror, so the size is the size of a single
                // member. E = 1/N.
                RaidLevel::Raid1 => member_size,

                // Block-level striping with distributed parity. E = (N-1)/N.
                RaidLevel::Raid5 => member_size * (member_count - 1),

                // Block-level striping with double distributed parity. E = (N-2)/N.
                RaidLevel::Raid6 => member_size * (member_count - 2),

                // Block-level striping with parity. E = 1/2.
                // Raid 10 required even number of members, so the integer division
                // of the member count by 2 give the effective number of stripes.
                RaidLevel::Raid10 => member_size * (member_count / 2),
            })
        }

        // For encrypted devices, we report the size of the backing device
        // minus an estimated size for the LUKS header.
        HostConfigBlockDevice::EncryptedVolume(_) => {
            let backing_node_idx = graph
                .neighbors_directed(idx, Direction::Outgoing)
                .next()
                .unwrap();
            let backing_size = block_device_size(graph, backing_node_idx)?;

            // The LUKS header is defined in LUKS_HEADER_SIZE_IN_MIB.
            Some(backing_size - (LUKS_HEADER_SIZE_IN_MIB as u64 * 1024 * 1024))
        }

        // For disks, we report None, as we don't know the size.
        HostConfigBlockDevice::Disk(_) => None,

        // For adopted partitions, we report None, as we don't know the size.
        HostConfigBlockDevice::AdoptedPartition(_) => None,
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
    use std::{str::FromStr, vec};

    use super::*;

    use sysdefs::tpm2::Pcr;

    use crate::{
        config::{
            AbUpdate, AbVolumePair, AdoptedPartition, Disk, EncryptedVolume, Encryption,
            FileSystem, MountPoint, NewFileSystemType, Partition, PartitionSize, PartitionType,
            Raid, SoftwareRaidArray, Storage, VerityDevice,
        },
        storage_graph::{
            node::BlockDevice, references::SpecialReferenceKind, types::HostConfigBlockDevice,
        },
    };

    #[test]
    fn test_filesystems_on_verity() {
        let mut graph = StorageGraph::default();

        // Assert that nothing is found in an emptry graph.
        assert_eq!(graph.filesystems_on_verity().next(), None);

        // Add a filesystem node that is not on a verity device.
        let fs = FileSystem {
            device_id: Some("fs1".into()),
            mount_point: Some(MountPoint::from_str("/mnt/fs1").unwrap()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
        };
        let fs_node_idx = graph.inner.add_node((&fs).into());

        // Assert that nothing is found in an emptry graph.
        assert_eq!(graph.filesystems_on_verity().next(), None);

        // Add a verity device to back the filesystem.
        let verity_dev = VerityDevice {
            id: "fs1".into(),
            name: "myVerityDevice".into(),
            data_device_id: "data".into(),
            hash_device_id: "hash".into(),
            ..Default::default()
        };
        let backing_node_idx = graph.inner.add_node((&verity_dev).into());

        // Add an edge from the partition to the filesystem.
        graph
            .inner
            .add_edge(fs_node_idx, backing_node_idx, ReferenceKind::Regular);

        // Assert that the filesystem is found when it is on a verity device.
        assert_eq!(graph.filesystems_on_verity().collect::<Vec<_>>(), vec![&fs]);

        // Add a new filesystem node that is not on a verity device.
        let fs2 = FileSystem {
            device_id: Some("fs2".into()),
            mount_point: Some(MountPoint::from_str("/mnt/fs2").unwrap()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
        };
        let fs2_node_idx = graph.inner.add_node((&fs2).into());

        // Assert the result is the same.
        assert_eq!(graph.filesystems_on_verity().collect::<Vec<_>>(), vec![&fs]);

        // Add a verity device to back the new filesystem.
        let verity_dev2 = VerityDevice {
            id: "fs2".into(),
            name: "myVerityDevice2".into(),
            data_device_id: "data2".into(),
            hash_device_id: "hash2".into(),
            ..Default::default()
        };
        let backing_node_idx2 = graph.inner.add_node((&verity_dev2).into());
        // Add an edge from the partition to the filesystem.
        graph
            .inner
            .add_edge(fs2_node_idx, backing_node_idx2, ReferenceKind::Regular);

        // Assert that the filesystem is found when it is on a verity device.
        assert_eq!(
            graph.filesystems_on_verity().collect::<Vec<_>>(),
            vec![&fs, &fs2]
        );
    }

    #[test]
    fn test_root_fs_node() {
        let mut graph = StorageGraph::default();

        // Assert that the root filesystem is not found in an empty graph.
        assert_eq!(graph.root_fs_node(), None);

        // Add a filesystem node that is not the root filesystem.
        let fs_node = StorageGraphNode::FileSystem(FileSystem {
            device_id: Some("fs1".into()),
            mount_point: Some(MountPoint::from_str("/mnt/fs1").unwrap()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
        });
        let _ = graph.inner.add_node(fs_node);

        // Assert that the root filesystem is not found when the only filesystem
        // node is not the root filesystem.
        assert_eq!(graph.root_fs_node(), None);

        // Add a filesystem node that is the root filesystem.
        let root_fs_node = StorageGraphNode::FileSystem(FileSystem {
            device_id: Some("rootfs".into()),
            mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
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
    }

    #[test]
    fn test_root_fs_is_verity() {
        let graph = StorageGraph::default();

        // Assert that the root filesystem is not on a verity device in an empty graph.
        assert!(!graph.root_fs_is_verity());

        // ==== Verity Dev ====

        let mut graph = StorageGraph::default();
        let block_dev_id = "rootfs";

        // Add a root filesystem node.
        let root_fs_node = StorageGraphNode::FileSystem(FileSystem {
            device_id: Some(block_dev_id.into()),
            mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
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

    #[test]
    fn test_has_dependents() {
        let mut graph = StorageGraph::default();

        // Assert we get an error when the block device is not found.
        assert_eq!(
            graph
                .has_dependents(&"rootfs".into())
                .unwrap_err()
                .to_string(),
            "Block device 'rootfs' not found"
        );

        // Add a partition node.
        let dev_id = "myPartition";
        let partition_node_idx = graph
            .inner
            .add_node(StorageGraphNode::BlockDevice(BlockDevice {
                id: dev_id.into(),
                host_config_ref: HostConfigBlockDevice::Partition(Partition {
                    id: dev_id.into(),
                    partition_type: PartitionType::Root,
                    size: "1G".parse().unwrap(),
                }),
            }));

        // Assert that the partition has no dependents.
        assert!(!graph.has_dependents(&dev_id.into()).unwrap());

        // Add a filesystem node that depends on the partition.
        let fs_node_idx = graph
            .inner
            .add_node(StorageGraphNode::FileSystem(FileSystem {
                device_id: Some(dev_id.into()),
                mount_point: Some(MountPoint::from_str(ROOT_MOUNT_POINT_PATH).unwrap()),
                source: FileSystemSource::New(NewFileSystemType::Ext4),
            }));

        // Add an edge from the partition to the filesystem.
        graph
            .inner
            .add_edge(fs_node_idx, partition_node_idx, ReferenceKind::Regular);

        // Assert that the partition has dependents.
        assert!(graph.has_dependents(&dev_id.into()).unwrap());
    }

    #[test]
    fn test_has_ab_capabilities() {
        let mut graph = StorageGraph::default();

        // We should receive None when the node does not exist.
        assert_eq!(graph.has_ab_capabilities(&"non_existent".into()), None);

        // Add a verity device
        let verity_device_node = graph
            .inner
            .add_node(StorageGraphNode::BlockDevice(BlockDevice {
                id: "verity".into(),
                host_config_ref: HostConfigBlockDevice::VerityDevice(VerityDevice {
                    id: "verity".into(),
                    name: "myVerityDevice".into(),
                    data_device_id: "data".into(),
                    hash_device_id: "hash".into(),
                    ..Default::default()
                }),
            }));

        // The partition node should not have A/B capabilities.
        assert_eq!(graph.has_ab_capabilities(&"verity".into()), Some(false));

        // Add an A/B volume node.
        let ab_volume_node = graph
            .inner
            .add_node(StorageGraphNode::BlockDevice(BlockDevice {
                id: "ab".into(),
                host_config_ref: HostConfigBlockDevice::ABVolume(AbVolumePair {
                    id: "ab-volume".into(),
                    volume_a_id: "volume_a".into(),
                    volume_b_id: "volume_b".into(),
                }),
            }));

        // The A/B volume node should have A/B capabilities.
        assert_eq!(graph.has_ab_capabilities(&"ab".into()), Some(true));

        // The verity device node should not have A/B capabilities, yet.
        assert_eq!(graph.has_ab_capabilities(&"verity".into()), Some(false));

        // Add an edge from the verity device to the A/B volume.
        graph.inner.add_edge(
            verity_device_node,
            ab_volume_node,
            ReferenceKind::Special(SpecialReferenceKind::VerityDataDevice),
        );

        // The verity device node should have A/B capabilities now.
        assert_eq!(graph.has_ab_capabilities(&"verity".into()), Some(true));
    }

    #[test]
    fn test_block_device_size() {
        // 1 GiB
        let s1 = 1024 * 1024 * 1024;

        let mut storage = Storage {
            disks: vec![Disk {
                id: "disk".into(),
                device: "/dev/sda".into(),
                partitions: vec![
                    Partition {
                        id: "fixed-partition".into(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::from(s1),
                    },
                    Partition {
                        id: "grow-partition".into(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::Grow,
                    },
                    Partition {
                        id: "data".into(),
                        partition_type: PartitionType::Root,
                        size: PartitionSize::from(2 * s1),
                    },
                    Partition {
                        id: "hash".into(),
                        partition_type: PartitionType::RootVerity,
                        size: PartitionSize::from(s1),
                    },
                    Partition {
                        id: "volume-a".into(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::from(s1),
                    },
                    Partition {
                        id: "volume-b".into(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::from(s1),
                    },
                    Partition {
                        id: "raid-1".into(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::from(s1),
                    },
                    Partition {
                        id: "raid-2".into(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::from(s1),
                    },
                    Partition {
                        id: "raid-3".into(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::from(s1),
                    },
                    Partition {
                        id: "raid-4".into(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::from(s1),
                    },
                    Partition {
                        id: "encrypted-partition".into(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: PartitionSize::from(s1),
                    },
                ],
                adopted_partitions: vec![AdoptedPartition {
                    id: "adopted-partition".into(),
                    match_label: Some("adopted".into()),
                    match_uuid: None,
                }],
                ..Default::default()
            }],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![AbVolumePair {
                    id: "ab-volume".into(),
                    volume_a_id: "volume-a".into(),
                    volume_b_id: "volume-b".into(),
                }],
            }),
            raid: Raid {
                software: vec![SoftwareRaidArray {
                    id: "raid".into(),
                    level: RaidLevel::Raid0,
                    name: "raid".into(),
                    devices: (1..=4).map(|i| format!("raid-{i}")).collect(),
                }],
                ..Default::default()
            },
            verity: vec![VerityDevice {
                id: "verity".into(),
                name: "verity".into(),
                data_device_id: "data".into(),
                hash_device_id: "hash".into(),
                ..Default::default()
            }],
            encryption: Some(Encryption {
                volumes: vec![EncryptedVolume {
                    id: "encrypted".into(),
                    device_id: "encrypted-partition".into(),
                    device_name: "encrypted".into(),
                }],
                recovery_key_url: None,
                pcrs: vec![Pcr::Pcr7],
            }),
            ..Default::default()
        };

        let graph = storage.build_graph().unwrap();

        // Assert no size for non-existing node.
        assert_eq!(graph.block_device_size(&"non-existing".into()), None);

        // Assert size for fixed partition.
        assert_eq!(graph.block_device_size(&"fixed-partition".into()), Some(s1));

        // Assert no size for grow partition.
        assert_eq!(graph.block_device_size(&"grow-partition".into()), None);

        // Assert size of verity device is the size of the data device.
        assert_eq!(
            graph.block_device_size(&"verity".into()),
            graph.block_device_size(&"data".into())
        );

        // Assert size of A/B volume is the size of volume A.
        assert_eq!(
            graph.block_device_size(&"volume-a".into()),
            graph.block_device_size(&"ab-volume".into())
        );

        // Assert size of A/B volume is the size of volume B.
        assert_eq!(
            graph.block_device_size(&"ab-volume".into()),
            graph.block_device_size(&"volume-b".into())
        );

        // Assert size of encrypted volume is the size of the backing device minus the LUKS header.
        assert_eq!(
            graph.block_device_size(&"encrypted".into()),
            Some(s1 - (LUKS_HEADER_SIZE_IN_MIB as u64 * 1024 * 1024))
        );

        // Assert size of RAID0 array is the size of the sum of all members.
        assert_eq!(graph.block_device_size(&"raid".into()), Some(s1 * 4));

        // Assert size of RAID1 array is the size of a single member.
        storage.raid.software[0].level = RaidLevel::Raid1;
        let graph = storage.build_graph().unwrap();
        assert_eq!(graph.block_device_size(&"raid".into()), Some(s1));

        // Assert size of RAID5 array is the size of a single member times (N-1).
        storage.raid.software[0].level = RaidLevel::Raid5;
        let graph = storage.build_graph().unwrap();
        assert_eq!(graph.block_device_size(&"raid".into()), Some(s1 * 3));

        // Assert size of RAID6 array is the size of a single member times (N-2).
        storage.raid.software[0].level = RaidLevel::Raid6;
        let graph = storage.build_graph().unwrap();
        assert_eq!(graph.block_device_size(&"raid".into()), Some(s1 * 2));

        // Assert size of RAID10 array is the size of a single member times (N/2).
        storage.raid.software[0].level = RaidLevel::Raid10;
        let graph = storage.build_graph().unwrap();
        assert_eq!(graph.block_device_size(&"raid".into()), Some(s1 * 2));
    }

    #[test]
    fn test_is_adopted() {
        let mut graph = StorageGraph::default();

        // We should receive None when the node does not exist.
        assert_eq!(graph.is_adopted(&"non_existent".into()), None);

        // Add an adopted partition node.
        let adopted_partition_node = graph.inner.add_node(
            (&AdoptedPartition {
                id: "adopted".into(),
                match_label: Some("adopted".into()),
                match_uuid: None,
            })
                .into(),
        );

        // The adopted partition node should be considered adopted.
        assert_eq!(graph.is_adopted(&"adopted".into()), Some(true));

        // Add a node of arbitrary type.
        let block_device_node = graph.inner.add_node(
            (&AbVolumePair {
                id: "ab-volume".into(),
                volume_a_id: "volume-a".into(),
                volume_b_id: "volume-b".into(),
            })
                .into(),
        );

        // The block device node should not be considered adopted.
        assert_eq!(graph.is_adopted(&"ab-volume".into()), Some(false));

        // Add an edge from the adopted partition to the block device.
        // This is not allowed by the storage graph rules, but we try it for the sake of
        // the test.
        graph.inner.add_edge(
            block_device_node,
            adopted_partition_node,
            ReferenceKind::Regular,
        );

        // The block device node should now be considered adopted, as it is on top of an
        // adopted partition.
        assert_eq!(graph.is_adopted(&"ab-volume".into()), Some(true));
    }
}
