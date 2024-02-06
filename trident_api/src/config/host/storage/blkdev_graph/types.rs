//! Basic types for the block device graph

use std::fmt::Display;

use anyhow::{bail, Error};
use serde::{Deserialize, Serialize};

use crate::{
    config::{
        AbVolumePair, Disk, EncryptedVolume, Image, MountPoint, Partition, SoftwareRaidArray,
    },
    BlockDeviceId,
};

/// Enum for supported block device types
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum BlkDevKind {
    /// A disk
    Disk,

    /// A new physical partition
    Partition,

    /// An existing physical partition that is being adopted
    ///
    /// Not yet implemented!
    #[allow(dead_code)]
    AdoptedPartition,

    /// A RAID array
    RaidArray,

    /// An A/B volume
    ABVolume,

    /// An encrypted volume
    EncryptedVolume,
}

bitflags::bitflags! {
    /// Bitflags for supported block device types
    ///
    /// MUST MATCH THE CONTENTS OF BlkDevKind
    #[derive(Serialize, Deserialize, Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct BlkDevKindFlag: u32 {
        const Disk = 1;
        const Partition = 1 << 1;
        const AdoptedPartition = 1 << 2;
        const RaidArray = 1 << 3;
        const ABVolume = 1 << 4;
        const EncryptedVolume = 1 << 5;
    }
}

/// Enum for holding HostConfiguration definitions
#[derive(Debug, Clone, PartialEq)]
pub enum HostConfigBlockDevice<'a> {
    /// A disk
    Disk(&'a Disk),

    /// A new physical partition
    Partition(&'a Partition),

    /// An existing physical partition that is being adopted
    ///
    /// Not yet implemented!
    #[allow(dead_code)]
    AdoptedPartition,

    /// A RAID array
    RaidArray(&'a SoftwareRaidArray),

    /// An A/B volume
    ABVolume(&'a AbVolumePair),

    /// An encrypted volume
    EncryptedVolume(&'a EncryptedVolume),
}

/// Enum for referrer kinds.
///
/// Referrers are config items that refer to other block devices.
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlkDevReferrerKind {
    /// Represents an 'null referrer' i.e. a block device that does not refer to other block devices
    ///
    /// Used to aggregate disks, partitions, and adopted partitions.
    None,

    /// A RAID array
    RaidArray,

    /// An A/B volume
    ABVolume,

    /// An encrypted volume
    EncryptedVolume,

    /// A regular image
    Image,

    /// A LZMA image for systemd-sysupdate
    ImageSysupdate,

    /// A mount point
    MountPoint,
}

bitflags::bitflags! {
    /// Bitflags for supported referrer kinds
    ///
    /// MUST MATCH THE CONTENTS OF BlkDevReferrerKind
    #[derive(Serialize, Deserialize, Default, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    pub struct BlkDevReferrerKindFlag: u32 {
        // Simple types:
        const RaidArray = 1 << 0;
        const ABVolume = 1 << 1;
        const EncryptedVolume = 1 << 2;
        const MountPoint = 1 << 3;
        const Image = 1 << 4;
        const ImageSysupdate = 1 << 5;

        // Groups:
        const AnyImage = Self::Image.bits() | Self::ImageSysupdate.bits();
    }
}

/// Node representing a block device in the graph
#[derive(Debug, Clone, PartialEq)]
pub struct BlkDevNode<'a> {
    /// The ID of the block device
    pub id: BlockDeviceId,

    /// The kind of block device
    pub kind: BlkDevKind,

    /// A reference to the original object in the host configuration
    pub host_config_ref: HostConfigBlockDevice<'a>,

    /// Any mount points associated with the block device
    pub mount_points: Vec<&'a MountPoint>,

    /// The image associated with the block device
    pub image: Option<&'a Image>,

    /// The block devices that this block device depends on
    pub targets: Vec<BlockDeviceId>,

    /// The block device, if any, that depend on this block device
    pub dependents: Vec<BlockDeviceId>,
}

impl HostConfigBlockDevice<'_> {
    /// Get the kind of block device
    pub(super) fn kind(&self) -> BlkDevKind {
        match self {
            HostConfigBlockDevice::Disk(_) => BlkDevKind::Disk,
            HostConfigBlockDevice::Partition(_) => BlkDevKind::Partition,
            HostConfigBlockDevice::AdoptedPartition => BlkDevKind::AdoptedPartition,
            HostConfigBlockDevice::RaidArray(_) => BlkDevKind::RaidArray,
            HostConfigBlockDevice::ABVolume(_) => BlkDevKind::ABVolume,
            HostConfigBlockDevice::EncryptedVolume(_) => BlkDevKind::EncryptedVolume,
        }
    }

    #[allow(dead_code)]
    pub(super) fn unwrap_encrypted_volume(&self) -> Result<&EncryptedVolume, Error> {
        if let HostConfigBlockDevice::EncryptedVolume(volume) = self {
            Ok(volume)
        } else {
            bail!("Block device is not an encrypted volume")
        }
    }

    #[allow(dead_code)]
    pub(super) fn unwrap_ab_volume(&self) -> Result<&AbVolumePair, Error> {
        if let HostConfigBlockDevice::ABVolume(volume) = self {
            Ok(volume)
        } else {
            bail!("Block device is not an A/B volume")
        }
    }

    #[allow(dead_code)]
    pub(super) fn unwrap_raid_array(&self) -> Result<&SoftwareRaidArray, Error> {
        if let HostConfigBlockDevice::RaidArray(raid_array) = self {
            Ok(raid_array)
        } else {
            bail!("Block device is not a RAID array")
        }
    }

    pub(super) fn unwrap_partition(&self) -> Result<&Partition, Error> {
        if let HostConfigBlockDevice::Partition(partition) = self {
            Ok(partition)
        } else {
            bail!("Block device is not a partition")
        }
    }

    #[allow(dead_code)]
    pub(super) fn unwrap_disk(&self) -> Result<&Disk, Error> {
        if let HostConfigBlockDevice::Disk(disk) = self {
            Ok(disk)
        } else {
            bail!("Block device is not a disk")
        }
    }
}

/// Conversion from BlkDevKind to BlkDevKindFlag
impl BlkDevKind {
    /// Returns the flag associated with the block device kind
    pub(crate) fn as_flag(&self) -> BlkDevKindFlag {
        match self {
            BlkDevKind::Disk => BlkDevKindFlag::Disk,
            BlkDevKind::Partition => BlkDevKindFlag::Partition,
            BlkDevKind::AdoptedPartition => BlkDevKindFlag::AdoptedPartition,
            BlkDevKind::RaidArray => BlkDevKindFlag::RaidArray,
            BlkDevKind::ABVolume => BlkDevKindFlag::ABVolume,
            BlkDevKind::EncryptedVolume => BlkDevKindFlag::EncryptedVolume,
        }
    }

    pub(crate) fn as_blkdev_referrer(&self) -> BlkDevReferrerKind {
        match self {
            BlkDevKind::Disk | BlkDevKind::Partition | BlkDevKind::AdoptedPartition => {
                BlkDevReferrerKind::None
            }
            BlkDevKind::RaidArray => BlkDevReferrerKind::RaidArray,
            BlkDevKind::ABVolume => BlkDevReferrerKind::ABVolume,
            BlkDevKind::EncryptedVolume => BlkDevReferrerKind::EncryptedVolume,
        }
    }
}

impl BlkDevReferrerKind {
    /// Returns the flag associated with the block device kind
    pub(crate) fn as_flag(&self) -> BlkDevReferrerKindFlag {
        match self {
            BlkDevReferrerKind::None => BlkDevReferrerKindFlag::empty(),
            BlkDevReferrerKind::RaidArray => BlkDevReferrerKindFlag::RaidArray,
            BlkDevReferrerKind::ABVolume => BlkDevReferrerKindFlag::ABVolume,
            BlkDevReferrerKind::EncryptedVolume => BlkDevReferrerKindFlag::EncryptedVolume,
            BlkDevReferrerKind::Image => BlkDevReferrerKindFlag::Image,
            BlkDevReferrerKind::ImageSysupdate => BlkDevReferrerKindFlag::ImageSysupdate,
            BlkDevReferrerKind::MountPoint => BlkDevReferrerKindFlag::MountPoint,
        }
    }
}

impl<'a> BlkDevNode<'a> {
    /// Creates a new block device node from a basic type i.e. has no members (disk, partition, etc.)
    pub(super) fn new_base(id: BlockDeviceId, hc_ref: HostConfigBlockDevice<'a>) -> Self {
        Self {
            id,
            kind: hc_ref.kind(),
            host_config_ref: hc_ref,
            mount_points: Vec::new(),
            image: None,
            targets: Vec::new(),
            dependents: Vec::new(),
        }
    }

    /// Creates a new block device node from a composite type i.e. has underlying members (ABVolume, EncryptedVolume, etc.)
    pub(super) fn new_composite<'b, S>(
        id: BlockDeviceId,
        hc_ref: HostConfigBlockDevice<'a>,
        members: S,
    ) -> Self
    where
        S: IntoIterator<Item = &'b BlockDeviceId>,
    {
        Self {
            id,
            kind: hc_ref.kind(),
            host_config_ref: hc_ref,
            mount_points: Vec::new(),
            image: None,
            targets: members.into_iter().cloned().collect(),
            dependents: Vec::new(),
        }
    }
}

// * * * * * * * * * * * * * * * * * * * * * *
// * Other convenience Trait implementations *
// * * * * * * * * * * * * * * * * * * * * * *

impl Display for BlkDevKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disk => write!(f, "disk"),
            Self::Partition => write!(f, "partition"),
            Self::AdoptedPartition => write!(f, "adopted-partition"),
            Self::RaidArray => write!(f, "raid-array"),
            Self::ABVolume => write!(f, "ab-volume"),
            Self::EncryptedVolume => write!(f, "encrypted-volume"),
        }
    }
}

impl Display for BlkDevReferrerKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BlkDevReferrerKind::None => write!(f, "none"),
            BlkDevReferrerKind::RaidArray => write!(f, "raid-array"),
            BlkDevReferrerKind::ABVolume => write!(f, "ab-volume"),
            BlkDevReferrerKind::EncryptedVolume => write!(f, "encrypted-volume"),
            BlkDevReferrerKind::Image => write!(f, "image"),
            BlkDevReferrerKind::ImageSysupdate => write!(f, "image-sysupdate"),
            BlkDevReferrerKind::MountPoint => write!(f, "mount-point"),
        }
    }
}

// * * * * * * * * * * * * * * * * * * * * * *
// * Bitflag display stuff                   *
// * * * * * * * * * * * * * * * * * * * * * *

/// Trait to turn turning bitflags into vectors of displayable items, one for
/// each active flag.
trait BitFlagsBackingEnumVec<T>: bitflags::Flags
where
    T: Display,
{
    fn backing_enum_vec(self) -> Vec<T>;

    fn user_readable(self) -> String {
        if self.is_empty() {
            return "(none)".into();
        }

        self.backing_enum_vec()
            .iter()
            .map(|kind| kind.to_string())
            .collect::<Vec<String>>()
            .join(" or ")
    }
}

/// Convert a BlkDevKindFlag to a vector of BlkDevKind
impl BitFlagsBackingEnumVec<BlkDevKind> for BlkDevKindFlag {
    /// Converts the flag to a vector of block device kinds
    fn backing_enum_vec(self) -> Vec<BlkDevKind> {
        self.into_iter()
            .map(|kind| match kind {
                BlkDevKindFlag::Disk => BlkDevKind::Disk,
                BlkDevKindFlag::Partition => BlkDevKind::Partition,
                BlkDevKindFlag::AdoptedPartition => BlkDevKind::AdoptedPartition,
                BlkDevKindFlag::RaidArray => BlkDevKind::RaidArray,
                BlkDevKindFlag::ABVolume => BlkDevKind::ABVolume,
                BlkDevKindFlag::EncryptedVolume => BlkDevKind::EncryptedVolume,
                _ => unreachable!(),
            })
            .collect()
    }
}

/// Convert a BlkDevReferrerKindFlag to a vector of BlkDevReferrerKind
impl BitFlagsBackingEnumVec<BlkDevReferrerKind> for BlkDevReferrerKindFlag {
    /// Converts the flag to a vector of block device kinds
    fn backing_enum_vec(self) -> Vec<BlkDevReferrerKind> {
        self.into_iter()
            .map(|kind| match kind {
                BlkDevReferrerKindFlag::RaidArray => BlkDevReferrerKind::RaidArray,
                BlkDevReferrerKindFlag::ABVolume => BlkDevReferrerKind::ABVolume,
                BlkDevReferrerKindFlag::EncryptedVolume => BlkDevReferrerKind::EncryptedVolume,
                BlkDevReferrerKindFlag::Image => BlkDevReferrerKind::Image,
                BlkDevReferrerKindFlag::ImageSysupdate => BlkDevReferrerKind::ImageSysupdate,
                BlkDevReferrerKindFlag::MountPoint => BlkDevReferrerKind::MountPoint,
                _ => unreachable!("Invalid referrer kind flag: {:?}", kind),
            })
            .collect()
    }
}

impl Display for BlkDevKindFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.user_readable())
    }
}

impl Display for BlkDevReferrerKindFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.user_readable())
    }
}

#[cfg(test)]
mod test {
    use super::{BitFlagsBackingEnumVec, BlkDevKindFlag, BlkDevReferrerKindFlag};

    #[test]
    fn test_backing_enum_block_dev_kind() {
        BlkDevKindFlag::all().iter().for_each(|flag| {
            let flag_vec = flag.backing_enum_vec();
            assert_eq!(
                flag_vec.len(),
                1,
                "Flag '{:?}' could not be converted to enum",
                flag
            );
        });
    }

    #[test]
    fn test_backing_enum_block_dev_referrer_kind() {
        BlkDevReferrerKindFlag::all().iter().for_each(|flag| {
            let flag_vec = flag.backing_enum_vec();
            assert_eq!(
                flag_vec.len(),
                1,
                "Flag '{:?}' could not be converted to enum",
                flag
            );
        });
    }
}
