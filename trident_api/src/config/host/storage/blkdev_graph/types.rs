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
    pub dependents: Option<BlockDeviceId>,
}

/// Enum for supported block device types
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum BlkDevReferrerKind {
    /// A kind that does not refer to other block devices
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
    #[cfg(feature = "sysupdate")]
    ImageSysupdate,

    /// A mount point
    MountPoint,
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
            dependents: None,
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
            dependents: None,
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
            Self::AdoptedPartition => write!(f, "adopted partition"),
            Self::RaidArray => write!(f, "RAID array"),
            Self::ABVolume => write!(f, "A/B volume"),
            Self::EncryptedVolume => write!(f, "encrypted volume"),
        }
    }
}

impl Display for BlkDevKindFlag {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.into_vec()
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<String>>()
                .join(" or ")
        )
    }
}

impl BlkDevKindFlag {
    /// Converts the flag to a vector of block device kinds
    pub(crate) fn into_vec(self) -> Vec<BlkDevKind> {
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
