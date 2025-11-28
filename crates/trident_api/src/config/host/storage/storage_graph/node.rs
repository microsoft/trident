use serde::{Deserialize, Serialize};

use crate::{
    config::{FileSystem, Swap},
    BlockDeviceId,
};

use super::{
    references::{SpecialReferenceKind, StorageReference},
    types::{BlkDevKind, BlkDevReferrerKind, HostConfigBlockDevice},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDevice {
    pub id: BlockDeviceId,
    pub host_config_ref: HostConfigBlockDevice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StorageGraphNode {
    BlockDevice(BlockDevice),
    FileSystem(FileSystem),
    Swap(Swap),
}

impl StorageGraphNode {
    /// Creates a new block device node.
    pub fn new_block_device(id: BlockDeviceId, host_config_ref: HostConfigBlockDevice) -> Self {
        Self::BlockDevice(BlockDevice {
            id,
            host_config_ref,
        })
    }

    /// Creates a new filesystem node.
    pub fn new_filesystem(fs: FileSystem) -> Self {
        Self::FileSystem(fs)
    }

    /// Creates a new swap device node.
    pub fn new_swap(swap: Swap) -> Self {
        Self::Swap(swap)
    }

    /// Returns a user friendly identifier of the node.
    pub fn identifier(&self) -> NodeIdentifier {
        match self {
            Self::BlockDevice(dev) => NodeIdentifier::from(dev),
            Self::FileSystem(fs) => NodeIdentifier::from(fs),
            Self::Swap(swap) => NodeIdentifier::from(swap),
        }
    }

    /// Returns a user friendly description of the node suitable for logging.
    ///
    /// This function is best used for logging purposes and where there is
    /// little context about the node, since this will provide a reasonable
    /// non-minimal description.
    ///
    /// Output examples:
    ///
    /// - `block device 'sda'`
    /// - `filesystem [type:ext4 dev:sda1]`
    /// - `verity filesystem 'root'`
    pub fn describe(&self) -> String {
        match self {
            Self::BlockDevice(dev) => format!("{} '{}'", dev.kind(), dev.id),
            Self::FileSystem(fs) => format!("filesystem [{}]", fs.description()),
            Self::Swap(swap) => format!("swap on '{}'", swap.device_id),
        }
    }

    /// Returns the ID of the block device when applicable.
    pub fn id(&self) -> Option<&BlockDeviceId> {
        match self {
            Self::BlockDevice(dev) => Some(&dev.id),
            Self::FileSystem(_) => None,
            Self::Swap(_) => None,
        }
    }

    /// Returns the inner block device, if this node is a block device.
    #[allow(dead_code)]
    pub fn as_block_device(&self) -> Option<&BlockDevice> {
        match self {
            Self::BlockDevice(dev) => Some(dev),
            _ => None,
        }
    }

    /// Returns the inner filesystem, if this node is a filesystem.
    pub fn as_filesystem(&self) -> Option<&FileSystem> {
        match self {
            Self::FileSystem(fs) => Some(fs),
            _ => None,
        }
    }

    /// Returns the inner swap device, if this node is a swap device.
    #[allow(dead_code)]
    pub fn as_swap_device(&self) -> Option<&Swap> {
        match self {
            Self::Swap(swap) => Some(swap),
            _ => None,
        }
    }

    /// Returns the kind of block device this node represents.
    pub fn device_kind(&self) -> BlkDevKind {
        match self {
            Self::BlockDevice(dev) => dev.kind(),
            Self::FileSystem(_) | Self::Swap(_) => BlkDevKind::None,
        }
    }

    /// Returns the kind of referrer this node represents.
    pub fn referrer_kind(&self) -> BlkDevReferrerKind {
        match self {
            Self::BlockDevice(dev) => dev.host_config_ref.referrer_kind(),
            Self::FileSystem(fs) => (fs).into(),
            Self::Swap(_) => BlkDevReferrerKind::Swap,
        }
    }

    /// Returns a vector of references to other devices that this node references.
    pub fn references(&self) -> Vec<StorageReference<'_>> {
        match self {
            Self::BlockDevice(dev) => match &dev.host_config_ref {
                HostConfigBlockDevice::Disk(_) => vec![],
                HostConfigBlockDevice::Partition(_) => vec![],
                HostConfigBlockDevice::AdoptedPartition(_) => vec![],
                HostConfigBlockDevice::RaidArray(raid_array) => raid_array
                    .devices
                    .iter()
                    .map(StorageReference::new_regular)
                    .collect(),
                HostConfigBlockDevice::ABVolume(ab_volume) => {
                    vec![
                        StorageReference::new_regular(&ab_volume.volume_a_id),
                        StorageReference::new_regular(&ab_volume.volume_b_id),
                    ]
                }
                HostConfigBlockDevice::EncryptedVolume(encrypted_volume) => {
                    vec![StorageReference::new_regular(&encrypted_volume.device_id)]
                }
                HostConfigBlockDevice::VerityDevice(verity_device) => {
                    vec![
                        StorageReference::new_special(
                            SpecialReferenceKind::VerityDataDevice,
                            &verity_device.data_device_id,
                        ),
                        StorageReference::new_special(
                            SpecialReferenceKind::VerityHashDevice,
                            &verity_device.hash_device_id,
                        ),
                    ]
                }
            },
            Self::FileSystem(fs) => fs
                .device_id
                .as_ref()
                .map(StorageReference::new_regular)
                .into_iter()
                .collect(),
            Self::Swap(swap) => vec![StorageReference::new_regular(&swap.device_id)],
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum NodeIdentifier {
    BlockDevice(String),
    FileSystem(String),
    Swap(String),
}

impl From<&FileSystem> for NodeIdentifier {
    fn from(fs: &FileSystem) -> Self {
        Self::FileSystem(fs.description())
    }
}

impl From<&BlockDevice> for NodeIdentifier {
    fn from(dev: &BlockDevice) -> Self {
        Self::BlockDevice(dev.id.clone())
    }
}

impl From<&Swap> for NodeIdentifier {
    fn from(swap: &Swap) -> Self {
        Self::Swap(swap.device_id.to_string())
    }
}

#[cfg(test)]
impl NodeIdentifier {
    pub fn block_device(id: &str) -> Self {
        Self::BlockDevice(id.to_string())
    }

    pub fn filesystem(id: &str) -> Self {
        Self::FileSystem(id.to_string())
    }
}

impl BlockDevice {
    /// Returns the kind of block device this node represents.
    pub fn kind(&self) -> BlkDevKind {
        self.host_config_ref.kind()
    }

    /// Returns the kind of referrer this node represents.
    #[allow(dead_code)]
    pub fn referrer_kind(&self) -> BlkDevReferrerKind {
        self.host_config_ref.referrer_kind()
    }
}
