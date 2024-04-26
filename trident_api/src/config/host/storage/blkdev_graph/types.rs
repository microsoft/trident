//! Basic types for the block device graph

use std::fmt::Display;

use anyhow::{bail, Error};
use serde::{Deserialize, Serialize};

use crate::{
    config::{
        AbVolumePair, Disk, EncryptedVolume, FileSystem, FileSystemSource, FileSystemType,
        MountPoint, Partition, SoftwareRaidArray, VerityFileSystem,
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
    /// Represents an 'null referrer' i.e. an entity that does not refer to any
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

    /// A regular filesystem
    FileSystem,

    /// A filesystem for sysupdate
    FileSystemSysupdate,

    /// A Verity filesystem
    VerityFileSystemData,

    /// A Verity filesystem
    VerityFileSystemHash,
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
        const FileSystem = 1 << 3;
        const FileSystemSysupdate = 1 << 4;
        const VerityFileSystemData = 1 << 5;
        const VerityFileSystemHash = 1 << 6;

        // Groups:
        // Example:
        // const AnyImage = Self::Image.bits() | Self::ImageSysupdate.bits();
    }
}

/// File system relationships for a node.
#[derive(Debug, Clone, PartialEq, Eq, Copy)]
pub enum NodeFileSystem<'a> {
    /// Regular filesystem is associated with the node
    Regular(&'a FileSystem),

    /// Verity filesystem is associated with the node as data device
    VerityData(&'a VerityFileSystem),

    /// Verity filesystem is associated with the node as hash device
    VerityHash(&'a VerityFileSystem),
}

impl<'a> NodeFileSystem<'a> {
    /// Get the filesystem type
    pub fn fs_type(&self) -> FileSystemType {
        match self {
            NodeFileSystem::Regular(fs) => fs.fs_type,
            NodeFileSystem::VerityData(vfs) | NodeFileSystem::VerityHash(vfs) => vfs.fs_type,
        }
    }

    /// Get Mountpoint
    pub fn mountpoint(&self) -> Option<&MountPoint> {
        match self {
            NodeFileSystem::Regular(fs) => fs.mount_point.as_ref(),
            NodeFileSystem::VerityData(vfs) => Some(&vfs.mount_point),
            NodeFileSystem::VerityHash(_) => None,
        }
    }

    /// Return whether this filesystem is backed by an image
    pub fn is_image_backed(&self) -> bool {
        match self {
            NodeFileSystem::Regular(fs) => matches!(fs.source, FileSystemSource::Image(_)),
            // Verity filesystems are always image backed
            // This code should break if this ever changes :)
            NodeFileSystem::VerityData(vfs) => !vfs.data_image.url.is_empty(),
            NodeFileSystem::VerityHash(vfs) => !vfs.hash_image.url.is_empty(),
        }
    }

    pub fn targets(&self) -> Vec<BlockDeviceId> {
        match self {
            NodeFileSystem::Regular(fs) => fs.device_id.iter().cloned().collect(),
            NodeFileSystem::VerityData(vfs) => {
                vec![vfs.data_device_id.clone(), vfs.hash_device_id.clone()]
            }
            NodeFileSystem::VerityHash(vfs) => {
                vec![vfs.data_device_id.clone(), vfs.hash_device_id.clone()]
            }
        }
    }

    pub fn identity(&self) -> String {
        match self {
            NodeFileSystem::Regular(fs) => {
                let mut out = format!("{} filesystem", fs.fs_type);
                if let Some(mntp) = fs.mount_point.as_ref() {
                    out.push_str(" mounted at ");
                    out.push_str(&mntp.path.to_string_lossy());
                } else if let Some(blkdevid) = fs.device_id.as_ref() {
                    out.push_str(" on block device ");
                    out.push_str(blkdevid);
                }

                out
            }
            NodeFileSystem::VerityData(vfs) | NodeFileSystem::VerityHash(vfs) => vfs.name.clone(),
        }
    }
}

/// Small helper to get the referrer kind from a NodeFileSystem
impl From<NodeFileSystem<'_>> for BlkDevReferrerKind {
    fn from(fs: NodeFileSystem) -> Self {
        match fs {
            NodeFileSystem::Regular(fs) => fs.into(),
            NodeFileSystem::VerityData(_) => Self::VerityFileSystemData,
            NodeFileSystem::VerityHash(_) => Self::VerityFileSystemHash,
        }
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

    /// The file system associated with the block device
    pub filesystem: Option<NodeFileSystem<'a>>,

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
            Self::None => BlkDevReferrerKindFlag::empty(),
            Self::RaidArray => BlkDevReferrerKindFlag::RaidArray,
            Self::ABVolume => BlkDevReferrerKindFlag::ABVolume,
            Self::EncryptedVolume => BlkDevReferrerKindFlag::EncryptedVolume,
            Self::FileSystem => BlkDevReferrerKindFlag::FileSystem,
            Self::FileSystemSysupdate => BlkDevReferrerKindFlag::FileSystemSysupdate,
            Self::VerityFileSystemData => BlkDevReferrerKindFlag::VerityFileSystemData,
            Self::VerityFileSystemHash => BlkDevReferrerKindFlag::VerityFileSystemHash,
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
            filesystem: None,
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
            filesystem: None,
            targets: members.into_iter().cloned().collect(),
            dependents: Vec::new(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
/// Enum for simple representation of a filesystem source
pub enum FileSystemSourceKind {
    /// Create a new file system.
    Create,

    /// Use an existing file system from a partition image.
    Image,

    /// Filesystem from an adopted block device.
    Adopted,
}

/// Wrapper for a list of FileSystemSourceKind
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct FileSystemSourceKindList(pub Vec<FileSystemSourceKind>);

impl FileSystemSourceKindList {
    pub(crate) fn contains(&self, fs_src_kind: FileSystemSourceKind) -> bool {
        self.0.contains(&fs_src_kind)
    }
}

// * * * * * * * * * * * * * * * * * * * * * *
// * Other convenience Trait implementations *
// * * * * * * * * * * * * * * * * * * * * * *

impl Display for FileSystemSourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileSystemSourceKind::Create => write!(f, "create"),
            FileSystemSourceKind::Image => write!(f, "image"),
            FileSystemSourceKind::Adopted => write!(f, "adopted"),
        }
    }
}

impl Display for FileSystemSourceKindList {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.0
                .iter()
                .map(|kind| kind.to_string())
                .collect::<Vec<String>>()
                .join(" or ")
        )
    }
}

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
            BlkDevReferrerKind::FileSystem => write!(f, "filesystem"),
            BlkDevReferrerKind::FileSystemSysupdate => write!(f, "filesystem-sysupdate"),
            BlkDevReferrerKind::VerityFileSystemData => write!(f, "verity-filesystem-data"),
            BlkDevReferrerKind::VerityFileSystemHash => write!(f, "verity-filesystem-hash"),
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
                BlkDevReferrerKindFlag::FileSystem => BlkDevReferrerKind::FileSystem,
                BlkDevReferrerKindFlag::FileSystemSysupdate => {
                    BlkDevReferrerKind::FileSystemSysupdate
                }
                BlkDevReferrerKindFlag::VerityFileSystemData => {
                    BlkDevReferrerKind::VerityFileSystemData
                }
                BlkDevReferrerKindFlag::VerityFileSystemHash => {
                    BlkDevReferrerKind::VerityFileSystemHash
                }
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
