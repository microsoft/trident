//! Conversions from config types to BlkDevNode

use crate::config::{
    AbVolumePair, AdoptedPartition, Disk, EncryptedVolume, FileSystem, FileSystemSource,
    ImageFormat, Partition, SoftwareRaidArray,
};

use super::types::{BlkDevNode, BlkDevReferrerKind, FileSystemSourceKind, HostConfigBlockDevice};

/// Get a BlkDevNode from a Disk reference
impl<'a, 'b> From<&'a Disk> for BlkDevNode<'b>
where
    'a: 'b,
{
    fn from(disk: &'a Disk) -> Self {
        Self::new_base(disk.id.clone(), HostConfigBlockDevice::Disk(disk))
    }
}

/// Get a BlkDevNode from a Partition reference
impl<'a, 'b> From<&'a Partition> for BlkDevNode<'b>
where
    'a: 'b,
{
    fn from(partition: &'a Partition) -> Self {
        Self::new_base(
            partition.id.clone(),
            HostConfigBlockDevice::Partition(partition),
        )
    }
}

/// Get a BlkDevNode from an AdoptedPartition reference
impl<'a, 'b> From<&'a AdoptedPartition> for BlkDevNode<'b>
where
    'a: 'b,
{
    fn from(partition: &'a AdoptedPartition) -> Self {
        Self::new_base(
            partition.id.clone(),
            HostConfigBlockDevice::AdoptedPartition(partition),
        )
    }
}

/// Get a BlkDevNode from a AbVolumePair reference
impl<'a, 'b> From<&'a AbVolumePair> for BlkDevNode<'b>
where
    'a: 'b,
{
    fn from(ab_volume_pair: &'a AbVolumePair) -> Self {
        Self::new_composite(
            ab_volume_pair.id.clone(),
            HostConfigBlockDevice::ABVolume(ab_volume_pair),
            [&ab_volume_pair.volume_a_id, &ab_volume_pair.volume_b_id],
        )
    }
}

/// Get a BlkDevNode from a SoftwareRaidArray reference
impl<'a, 'b> From<&'a SoftwareRaidArray> for BlkDevNode<'b>
where
    'a: 'b,
{
    fn from(raid_array: &'a SoftwareRaidArray) -> Self {
        Self::new_composite(
            raid_array.id.clone(),
            HostConfigBlockDevice::RaidArray(raid_array),
            raid_array.devices.iter(),
        )
    }
}

/// Get a BlkDevNode from an EncryptedVolume reference
impl<'a, 'b> From<&'a EncryptedVolume> for BlkDevNode<'b>
where
    'a: 'b,
{
    fn from(volume: &'a EncryptedVolume) -> Self {
        Self::new_composite(
            volume.id.clone(),
            HostConfigBlockDevice::EncryptedVolume(volume),
            [&volume.device_id],
        )
    }
}

/// Get a BlkDevReferrerKind from a FileSystem reference
impl From<&FileSystem> for BlkDevReferrerKind {
    fn from(fs: &FileSystem) -> Self {
        if fs.fs_type.requires_block_device_id() {
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

                // If it's an image, then it depends on the image format.
                FileSystemSource::Image(img) => match img.format {
                    // If we're creating the FS from a zst image, then it's a
                    // filesystem referrer.
                    ImageFormat::RawZst => BlkDevReferrerKind::FileSystem,

                    // If we're creating the FS from a lzma image, then it's a
                    // sysupdate referrer.
                    #[cfg(feature = "sysupdate")]
                    ImageFormat::RawLzma => BlkDevReferrerKind::FileSystemSysupdate,
                },

                FileSystemSource::OsImage => BlkDevReferrerKind::FileSystemOsImage,
            }
        } else {
            // Filesystems that do not require a block device are not referrers.
            BlkDevReferrerKind::None
        }
    }
}

/// Get a FileSystemSourceKind from a FileSystemSource reference
impl From<&FileSystemSource> for FileSystemSourceKind {
    fn from(source: &FileSystemSource) -> Self {
        match source {
            FileSystemSource::Create => FileSystemSourceKind::Create,
            FileSystemSource::Image(_) => FileSystemSourceKind::Image,
            FileSystemSource::Adopted => FileSystemSourceKind::Adopted,
            FileSystemSource::EspImage(_) => FileSystemSourceKind::EspBundle,
            FileSystemSource::OsImage => FileSystemSourceKind::OsImage,
        }
    }
}
