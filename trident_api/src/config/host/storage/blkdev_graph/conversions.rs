//! Conversions from config types to BlkDevNode

use crate::config::{
    AbVolumePair, Disk, EncryptedVolume, Image, ImageFormat, Partition, SoftwareRaidArray,
};

use super::types::{BlkDevNode, BlkDevReferrerKind, HostConfigBlockDevice};

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
            [&volume.target_id],
        )
    }
}

impl From<&Image> for BlkDevReferrerKind {
    fn from(i: &Image) -> Self {
        match i.format {
            ImageFormat::RawZst => BlkDevReferrerKind::Image,
            #[cfg(feature = "sysupdate")]
            ImageFormat::RawLzma => BlkDevReferrerKind::ImageSysupdate,
        }
    }
}
