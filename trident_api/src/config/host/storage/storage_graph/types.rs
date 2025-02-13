//! Basic types for the block device graph

#[cfg(feature = "documentation")]
use documented::{Documented, DocumentedVariants};

use std::fmt::Display;

use anyhow::{bail, Error};
use serde::{Deserialize, Serialize};

use crate::config::{
    host::storage::verity::VerityDevice, AbVolumePair, AdoptedPartition, Disk, EncryptedVolume,
    Partition, SoftwareRaidArray,
};

/// Enum for supported block device types
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(
    any(test, feature = "documentation"),
    derive(strum_macros::EnumIter, Documented, DocumentedVariants)
)]
pub enum BlkDevKind {
    /// Represents a 'null device' i.e. something that is not really a block device.
    None,

    /// A disk
    Disk,

    /// A new physical partition
    Partition,

    /// An existing physical partition that is being adopted
    AdoptedPartition,

    /// A RAID array
    RaidArray,

    /// An A/B volume
    ABVolume,

    /// An encrypted volume
    EncryptedVolume,

    /// A verity device
    VerityDevice,
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
        const VerityDevice = 1 << 6;
    }
}

/// Enum for holding HostConfiguration definitions
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostConfigBlockDevice {
    /// A disk
    Disk(Disk),

    /// A new physical partition
    Partition(Partition),

    /// An existing physical partition that is being adopted
    AdoptedPartition(AdoptedPartition),

    /// A RAID array
    RaidArray(SoftwareRaidArray),

    /// An A/B volume
    ABVolume(AbVolumePair),

    /// An encrypted volume
    EncryptedVolume(EncryptedVolume),

    /// A verity device
    VerityDevice(VerityDevice),
}

/// Enum for referrer kinds.
///
/// Referrers are config items that refer to other block devices.
#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
#[cfg_attr(
    feature = "documentation",
    derive(strum_macros::EnumIter, Documented, DocumentedVariants)
)]
pub enum BlkDevReferrerKind {
    /// Represents a 'null referrer', i.e. an entity that does not refer to any
    /// block device.
    ///
    /// E.g. Block devices that do not refer to any other block devices, such as
    /// disks, partitions, and adopted partitions.
    None,

    /// A RAID array
    RaidArray,

    /// An A/B volume
    ABVolume,

    /// An encrypted volume
    EncryptedVolume,

    /// A verity device
    VerityDevice,

    /// A regular filesystem
    FileSystem,

    /// A filesystem from an OS image
    FileSystemOsImage,

    /// An ESP/EFI filesystem
    FileSystemEsp,

    /// A verity filesystem
    FilesystemVerity,

    /// An adopted filesystem
    FileSystemAdopted,
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
        const VerityDevice = 1 << 3;
        const FileSystem = 1 << 4;
        const FileSystemOsImage = 1 << 5;
        const FileSystemEsp = 1 << 6;
        const FileSystemVerity = 1 << 7;
        const FileSystemAdopted = 1 << 8;

        // Groups:
        // Example:
        // const AnyImage = Self::Image.bits() | Self::ImageSysupdate.bits();
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
/// Enum for simple representation of a filesystem source
pub enum FileSystemSourceKind {
    /// Create a new file system.
    New,

    /// Use an existing file system from a partition image.
    Image,

    /// Filesystem from an adopted block device.
    Adopted,

    /// Use an existing file system from an ESP image.
    EspBundle,

    /// Use an existing file system from an OS Image.
    OsImage,
}

impl HostConfigBlockDevice {
    /// Returns the kind of the block device of this HostConfigBlockDevice.
    pub fn kind(&self) -> BlkDevKind {
        match self {
            Self::Disk(_) => BlkDevKind::Disk,
            Self::Partition(_) => BlkDevKind::Partition,
            Self::AdoptedPartition(_) => BlkDevKind::AdoptedPartition,
            Self::RaidArray(_) => BlkDevKind::RaidArray,
            Self::ABVolume(_) => BlkDevKind::ABVolume,
            Self::EncryptedVolume(_) => BlkDevKind::EncryptedVolume,
            Self::VerityDevice(_) => BlkDevKind::VerityDevice,
        }
    }

    /// Returns the kind of the referrer of this HostConfigBlockDevice.
    pub fn referrer_kind(&self) -> BlkDevReferrerKind {
        match self {
            Self::Disk(_) => BlkDevReferrerKind::None,
            Self::Partition(_) => BlkDevReferrerKind::None,
            Self::AdoptedPartition(_) => BlkDevReferrerKind::None,
            Self::RaidArray(_) => BlkDevReferrerKind::RaidArray,
            Self::ABVolume(_) => BlkDevReferrerKind::ABVolume,
            Self::EncryptedVolume(_) => BlkDevReferrerKind::EncryptedVolume,
            Self::VerityDevice(_) => BlkDevReferrerKind::VerityDevice,
        }
    }

    pub(super) fn unwrap_adopted_partition(&self) -> Result<&AdoptedPartition, Error> {
        if let HostConfigBlockDevice::AdoptedPartition(partition) = self {
            Ok(partition)
        } else {
            bail!("Block device is not an adopted partition")
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

    #[allow(dead_code)]
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

    pub(super) fn unwrap_verity_device(&self) -> Result<&VerityDevice, Error> {
        if let HostConfigBlockDevice::VerityDevice(verity_device) = self {
            Ok(verity_device)
        } else {
            bail!("Block device is not a verity device")
        }
    }
}

/// Conversion from BlkDevKind to BlkDevKindFlag
impl BlkDevKind {
    /// Returns the flag associated with the block device kind
    pub fn as_flag(&self) -> BlkDevKindFlag {
        match self {
            Self::None => BlkDevKindFlag::empty(),
            Self::Disk => BlkDevKindFlag::Disk,
            Self::Partition => BlkDevKindFlag::Partition,
            Self::AdoptedPartition => BlkDevKindFlag::AdoptedPartition,
            Self::RaidArray => BlkDevKindFlag::RaidArray,
            Self::ABVolume => BlkDevKindFlag::ABVolume,
            Self::EncryptedVolume => BlkDevKindFlag::EncryptedVolume,
            Self::VerityDevice => BlkDevKindFlag::VerityDevice,
        }
    }
}

impl BlkDevReferrerKind {
    /// Returns the flag associated with the block device kind
    pub fn as_flag(&self) -> BlkDevReferrerKindFlag {
        match self {
            Self::None => BlkDevReferrerKindFlag::empty(),
            Self::RaidArray => BlkDevReferrerKindFlag::RaidArray,
            Self::ABVolume => BlkDevReferrerKindFlag::ABVolume,
            Self::EncryptedVolume => BlkDevReferrerKindFlag::EncryptedVolume,
            Self::VerityDevice => BlkDevReferrerKindFlag::VerityDevice,
            Self::FileSystem => BlkDevReferrerKindFlag::FileSystem,
            Self::FileSystemEsp => BlkDevReferrerKindFlag::FileSystemEsp,
            Self::FileSystemAdopted => BlkDevReferrerKindFlag::FileSystemAdopted,
            Self::FilesystemVerity => BlkDevReferrerKindFlag::FileSystemVerity,
            Self::FileSystemOsImage => BlkDevReferrerKindFlag::FileSystemOsImage,
        }
    }
}

pub(super) trait BitFlagsBackingEnumVec<T>: bitflags::Flags
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

/// Convert a BlkDevKindFlag to a vector of BlkDevKind.
impl BitFlagsBackingEnumVec<BlkDevKind> for BlkDevKindFlag {
    /// Converts the flag to a vector of block device kinds.
    fn backing_enum_vec(self) -> Vec<BlkDevKind> {
        self.into_iter()
            .map(|kind| match kind {
                BlkDevKindFlag::Disk => BlkDevKind::Disk,
                BlkDevKindFlag::Partition => BlkDevKind::Partition,
                BlkDevKindFlag::AdoptedPartition => BlkDevKind::AdoptedPartition,
                BlkDevKindFlag::RaidArray => BlkDevKind::RaidArray,
                BlkDevKindFlag::ABVolume => BlkDevKind::ABVolume,
                BlkDevKindFlag::EncryptedVolume => BlkDevKind::EncryptedVolume,
                BlkDevKindFlag::VerityDevice => BlkDevKind::VerityDevice,
                _ => unreachable!("Invalid block device kind flag: {:?}", kind),
            })
            .collect()
    }
}

/// Convert a BlkDevReferrerKindFlag to a vector of BlkDevReferrerKind.
impl BitFlagsBackingEnumVec<BlkDevReferrerKind> for BlkDevReferrerKindFlag {
    /// Converts the flag to a vector of block device kinds.
    fn backing_enum_vec(self) -> Vec<BlkDevReferrerKind> {
        self.into_iter()
            .map(|kind| match kind {
                BlkDevReferrerKindFlag::RaidArray => BlkDevReferrerKind::RaidArray,
                BlkDevReferrerKindFlag::ABVolume => BlkDevReferrerKind::ABVolume,
                BlkDevReferrerKindFlag::VerityDevice => BlkDevReferrerKind::VerityDevice,
                BlkDevReferrerKindFlag::EncryptedVolume => BlkDevReferrerKind::EncryptedVolume,
                BlkDevReferrerKindFlag::FileSystem => BlkDevReferrerKind::FileSystem,
                BlkDevReferrerKindFlag::FileSystemEsp => BlkDevReferrerKind::FileSystemEsp,
                BlkDevReferrerKindFlag::FileSystemAdopted => BlkDevReferrerKind::FileSystemAdopted,
                BlkDevReferrerKindFlag::FileSystemVerity => BlkDevReferrerKind::FilesystemVerity,
                BlkDevReferrerKindFlag::FileSystemOsImage => BlkDevReferrerKind::FileSystemOsImage,
                _ => unreachable!("Invalid referrer kind flag: {:?}", kind),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
