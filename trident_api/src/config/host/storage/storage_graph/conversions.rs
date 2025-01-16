// //! Conversions from config types to BlkDevNode

use crate::config::{
    AbVolumePair, AdoptedPartition, Disk, EncryptedVolume, FileSystem, FileSystemSource, Partition,
    SoftwareRaidArray, VerityFileSystem,
};

use super::{
    node::StorageGraphNode,
    types::{BlkDevReferrerKind, FileSystemSourceKind, HostConfigBlockDevice},
};

/// Get a FileSystemSourceKind from a FileSystemSource reference
impl From<&FileSystemSource> for FileSystemSourceKind {
    fn from(source: &FileSystemSource) -> Self {
        match source {
            FileSystemSource::Create => Self::Create,
            FileSystemSource::Image(_) => Self::Image,
            FileSystemSource::Adopted => Self::Adopted,
            FileSystemSource::EspImage(_) => Self::EspBundle,
            FileSystemSource::OsImage => Self::OsImage,
        }
    }
}

/// Get a StorageGraphNode from a Disk reference.
impl From<&Disk> for StorageGraphNode {
    fn from(disk: &Disk) -> Self {
        Self::new_block_device(disk.id.clone(), HostConfigBlockDevice::Disk(disk.clone()))
    }
}

/// Get a StorageGraphNode from a Partition reference.
impl From<&Partition> for StorageGraphNode {
    fn from(partition: &Partition) -> Self {
        Self::new_block_device(
            partition.id.clone(),
            HostConfigBlockDevice::Partition(partition.clone()),
        )
    }
}

/// Get a StorageGraphNode from an AdoptedPartition reference.
impl From<&AdoptedPartition> for StorageGraphNode {
    fn from(partition: &AdoptedPartition) -> Self {
        Self::new_block_device(
            partition.id.clone(),
            HostConfigBlockDevice::AdoptedPartition(partition.clone()),
        )
    }
}

/// Get a StorageGraphNode from an AbVolumePair reference.
impl From<&AbVolumePair> for StorageGraphNode {
    fn from(ab_volume_pair: &AbVolumePair) -> Self {
        Self::new_block_device(
            ab_volume_pair.id.clone(),
            HostConfigBlockDevice::ABVolume(ab_volume_pair.clone()),
        )
    }
}

/// Get a StorageGraphNode from a SoftwareRaidArray reference.
impl From<&SoftwareRaidArray> for StorageGraphNode {
    fn from(raid_array: &SoftwareRaidArray) -> Self {
        Self::new_block_device(
            raid_array.id.clone(),
            HostConfigBlockDevice::RaidArray(raid_array.clone()),
        )
    }
}

/// Get a StorageGraphNode from an EncryptedVolume reference.
impl From<&EncryptedVolume> for StorageGraphNode {
    fn from(volume: &EncryptedVolume) -> Self {
        Self::new_block_device(
            volume.id.clone(),
            HostConfigBlockDevice::EncryptedVolume(volume.clone()),
        )
    }
}

/// Get a StorageGraphNode from a Filesystem reference.
impl From<&FileSystem> for StorageGraphNode {
    fn from(fs: &FileSystem) -> Self {
        Self::new_filesystem(fs.clone())
    }
}

/// Get a StorageGraphNode from a verity Filesystem reference.
impl From<&VerityFileSystem> for StorageGraphNode {
    fn from(vfs: &VerityFileSystem) -> Self {
        Self::new_verity_filesystem(vfs.clone())
    }
}

/// Get a BlkDevReferrerKind from a FileSystem reference.
impl From<&FileSystem> for BlkDevReferrerKind {
    fn from(fs: &FileSystem) -> Self {
        if fs.fs_type.expects_block_device_id() {
            // Filesystems that require a block device are filesystem referrers.
            match &fs.source {
                // If we're creating a filesystem, then it's a regular
                // filesystem referrer.
                FileSystemSource::Create => BlkDevReferrerKind::FileSystem,

                // If we're adopting a filesystem, then it's an adopted
                // filesystem referrer.
                FileSystemSource::Adopted => BlkDevReferrerKind::FileSystemAdopted,

                // If we're creating a filesystem from an ESP bundle, then it's
                // an ESP filesystem referrer.
                FileSystemSource::EspImage(_) => BlkDevReferrerKind::FileSystemEsp,

                // If it's an image, then it is a filesystem referrer.
                FileSystemSource::Image(_) => BlkDevReferrerKind::FileSystem,

                // If it's an OS image, then it is an OS image filesystem referrer.
                FileSystemSource::OsImage => BlkDevReferrerKind::FileSystemOsImage,
            }
        } else {
            // Filesystems that do not require a block device are not referrers.
            BlkDevReferrerKind::None
        }
    }
}
