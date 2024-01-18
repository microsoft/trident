use std::{collections::BTreeMap, path::Path};

use anyhow::{bail, Context, Error};

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

    /// Check that a mount point for a volume is present and that it is
    /// backed by an image. This is to make sure that Trident can detect the
    /// volume and the volume is initialized using customer provided
    /// image, not just an empty filesystem.
    pub(crate) fn validate_volume_presence(&self, mount_point_path: &Path) -> Result<(), Error> {
        let (_id, node) = self
            .nodes
            .iter()
            .find(|(_id, node)| {
                node.mount_points
                    .iter()
                    .any(|mp| mp.path == mount_point_path)
            })
            .context(format!(
                "'{}' mount point must be present",
                mount_point_path.display()
            ))?;

        if node.image.is_none() {
            bail!(format!(
                "'{}' mount point must be backed by an image",
                mount_point_path.display()
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::config::{
        host::storage::blkdev_graph::types::{BlkDevKind, HostConfigBlockDevice},
        Image, ImageFormat, ImageSha256, MountPoint, Partition, PartitionType,
    };

    use super::*;

    #[test]
    fn test_validate_volume_presence() {
        let mut nodes = BTreeMap::new();
        let partition = Partition {
            id: "foo".into(),
            partition_type: PartitionType::Root,
            size: crate::config::PartitionSize::Fixed(0),
        };
        let image = Image {
            url: "foo".into(),
            sha256: ImageSha256::Ignored,
            format: ImageFormat::RawZstd,
            target_id: "foo".into(),
        };
        let mut node = BlkDevNode {
            id: "foo".into(),
            kind: BlkDevKind::Partition,
            host_config_ref: HostConfigBlockDevice::Partition(&partition),
            mount_points: vec![],
            image: Some(&image),
            targets: vec![],
            dependents: None,
        };
        let mount_point = MountPoint {
            path: PathBuf::from("/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-123"),
            filesystem: "barfoo".into(),
            target_id: "foobar".into(),
            options: vec![],
        };
        node.mount_points.push(&mount_point);

        let mut node2 = BlkDevNode {
            id: "foo2".into(),
            kind: BlkDevKind::Partition,
            host_config_ref: HostConfigBlockDevice::Partition(&partition),
            mount_points: vec![],
            image: None,
            targets: vec![],
            dependents: None,
        };
        let mount_point2 = MountPoint {
            path: PathBuf::from("/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-456"),
            filesystem: "barfoo".into(),
            target_id: "foobar".into(),
            options: vec![],
        };
        node2.mount_points.push(&mount_point2);

        nodes.insert("sda".into(), node);
        nodes.insert("sdb".into(), node2);

        let graph = BlockDeviceGraph { nodes };

        graph
            .validate_volume_presence(Path::new(
                "/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-123",
            ))
            .unwrap();
        assert_eq!(
            graph
                .validate_volume_presence(Path::new(
                    "/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-456"
                ))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "'/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-456' mount point must be backed by an image"
        );
        assert_eq!(graph
            .validate_volume_presence(Path::new(
                "/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-789"
            ))
            .unwrap_err().root_cause().to_string(), "'/var/lib/kubelet/pods/123/volumes/kubernetes.io~csi/pvc-789' mount point must be present");
    }
}
