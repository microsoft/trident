//! #Constraint Declarations
//!
//! This module contains all the per-kind validation rules for block devices.
//! Generic rules that apply to all are covered directly in the build() function
//! of BlockDeviceGraphBuilder. (e.g. uniqueness of IDs)
//!
//! The rules declared in this section are used by BlockDeviceGraphBuilder to
//! validate specific
//!
//! The rules are declared in the order they are evaluated:
//! 1. Basic checks: Checks that do not depend on the graph.
//! 2. Target Kind validity: Can device of kind A refer to device of kind B?
//! 3. Member Count validity: How many members can a device of kind A have?
//! 4. Sharing: What referrers can refer to the same target as a given referrer
//!    at the same time?
//! 5. Field uniqueness: What field values must be unique across all devices of
//!    type A?
//! 6. Node Target validity: Are the targets of a given node valid? Do they meet
//!    all the required criteria?

use std::os::unix::ffi::OsStrExt;

use anyhow::{bail, ensure, Error};

use crate::config::{
    FileSystemType, HostConfigurationStaticValidationError, Partition, PartitionSize, PartitionType,
};

use super::{
    cardinality::ValidCardinality,
    graph::BlockDeviceGraph,
    mountpoints::ValidMountpoints,
    types::{
        AllowBlockList, BlkDevKind, BlkDevKindFlag, BlkDevNode, BlkDevReferrerKind,
        BlkDevReferrerKindFlag, FileSystemSourceKind, FileSystemSourceKindList,
        HostConfigBlockDevice,
    },
};

/// This impl block contains validation rules for host-config objects
impl<'a> HostConfigBlockDevice<'a> {
    /// Checks basic context-free attributes of the block device
    ///
    /// Use this function to check attributes that do not depend on the graph,
    /// just simple rules & attributes that must be met for each block device
    /// kind.
    pub(super) fn basic_check(&self) -> Result<(), Error> {
        match self {
            Self::Disk(disk) => {
                ensure!(
                    disk.device.is_absolute(),
                    HostConfigurationStaticValidationError::PathNotAbsolute {
                        path: disk.device.to_string_lossy().to_string(),
                    }
                );
            }
            Self::Partition(Partition {
                size: PartitionSize::Fixed(size),
                ..
            }) => {
                ensure!(
                    size.bytes() > 0 && size.bytes() % 4096 == 0,
                    "Partition size must be a non-zero multiple of 4096 bytes."
                );
            }
            Self::Partition(Partition {
                size: PartitionSize::Grow,
                ..
            }) => (),
            Self::AdoptedPartition(ap) => match (&ap.match_label, &ap.match_uuid) {
                (Some(_), Some(_)) => {
                    bail!("Adopted partitions cannot have both matchLabel and matchUUID");
                }
                (None, None) => {
                    bail!("Adopted partitions must have either matchLabel or matchUUID");
                }
                _ => (),
            },
            Self::RaidArray(_) => (),
            Self::ABVolume(_) => (),
            Self::EncryptedVolume(_) => (),
        }

        Ok(())
    }
}

/// This impl block contains validation rules for block device referrers
impl BlkDevReferrerKind {
    /// Returns a list of kinds compatible with the referrer kind.
    pub fn compatible_kinds(&self) -> BlkDevKindFlag {
        match self {
            Self::None => BlkDevKindFlag::empty(),
            Self::RaidArray => BlkDevKindFlag::Partition,
            Self::ABVolume => {
                BlkDevKindFlag::Partition
                    | BlkDevKindFlag::RaidArray
                    | BlkDevKindFlag::EncryptedVolume
            }
            Self::EncryptedVolume => BlkDevKindFlag::Partition | BlkDevKindFlag::RaidArray,
            Self::FileSystem | Self::FileSystemOsImage => {
                BlkDevKindFlag::Partition
                    | BlkDevKindFlag::RaidArray
                    | BlkDevKindFlag::EncryptedVolume
                    | BlkDevKindFlag::ABVolume
            }
            Self::FileSystemEsp => {
                BlkDevKindFlag::Partition
                    | BlkDevKindFlag::AdoptedPartition
                    | BlkDevKindFlag::RaidArray
            }
            Self::FileSystemAdopted => BlkDevKindFlag::AdoptedPartition,
            Self::FileSystemSysupdate => BlkDevKindFlag::ABVolume,
            Self::VerityFileSystemData | Self::VerityFileSystemHash => {
                BlkDevKindFlag::Partition | BlkDevKindFlag::RaidArray | BlkDevKindFlag::ABVolume
            }
        }
    }

    /// Returns the valid number of members for the referrer kind.
    ///
    /// This table shows the valid number of members for each referrer:
    pub fn valid_target_count(self) -> ValidCardinality {
        match self {
            Self::None => ValidCardinality::new_zero(),
            Self::RaidArray => ValidCardinality::new_at_least(2),
            Self::ABVolume => ValidCardinality::new_exact(2),
            Self::EncryptedVolume => ValidCardinality::new_exact(1),

            // These are not really used, but we define them for
            // completeness
            Self::FileSystem => ValidCardinality::new_at_most(1),
            Self::FileSystemOsImage
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FileSystemSysupdate
            | Self::VerityFileSystemData
            | Self::VerityFileSystemHash => ValidCardinality::new_exact(1),
        }
    }

    /// Returns a bitset of other referrers that may also refer to the same
    /// targets as this referrer kind at the same time.
    ///
    /// In other words, what other referrers can share the same targets as the
    /// current referrer?
    ///
    /// Returning an empty bitset means that a kind is claiming *exclusive*
    /// access over its targets. Nothing else can refer to them.
    ///
    /// This is useful for cases when we want a node to be shareable between two
    /// (or more) referrers or the same or other kind.
    ///
    /// IMPORTANT: Sharing goes both ways! Both referrers must be in each
    /// other's valid_sharing_peers() bitset for it to work!
    ///
    /// NOTE: Filesystems are special referrers that follow additional rules not
    /// covered here:
    /// 1. Filesystems can never share with any other filesystem kind. (Only one
    ///    filesystem slot!)
    ///
    /// The reason for this is that filesystems are not block devices, so they
    /// get their own respective fields in the BlkDevNode object.
    /// - #1 above is enforced by the node struct having only an `Option` to
    ///   store the image associated with it.
    /// - #2 follows from the node struct using a `Vec` to store mount points.
    pub fn valid_sharing_peers(self) -> BlkDevReferrerKindFlag {
        match self {
            Self::None => BlkDevReferrerKindFlag::empty(),
            Self::RaidArray => BlkDevReferrerKindFlag::empty(),
            Self::ABVolume => BlkDevReferrerKindFlag::empty(),
            Self::EncryptedVolume => BlkDevReferrerKindFlag::empty(),
            Self::FileSystem
            | Self::FileSystemOsImage
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FileSystemSysupdate
            | Self::VerityFileSystemData
            | Self::VerityFileSystemHash => BlkDevReferrerKindFlag::empty(),
        }
    }
}

impl FileSystemType {
    /// Returns whether a filesystem type requires a block device ID.
    pub fn requires_block_device_id(&self) -> bool {
        match self {
            Self::Ext4
            | Self::Xfs
            | Self::Vfat
            | Self::Ntfs
            | Self::Iso9660
            | Self::Swap
            | Self::Auto
            | Self::Other => true,
            Self::Tmpfs | Self::Overlay => false,
        }
    }

    /// Returns the valid sources for a filesystem type.
    pub fn valid_sources(&self) -> FileSystemSourceKindList {
        match self {
            Self::Ext4 | Self::Xfs | Self::Ntfs => FileSystemSourceKindList(vec![
                FileSystemSourceKind::Create,
                FileSystemSourceKind::Image,
                FileSystemSourceKind::Adopted,
                FileSystemSourceKind::OsImage,
            ]),
            Self::Vfat => FileSystemSourceKindList(vec![
                FileSystemSourceKind::Create,
                FileSystemSourceKind::Image,
                FileSystemSourceKind::Adopted,
                FileSystemSourceKind::EspBundle,
                FileSystemSourceKind::OsImage,
            ]),
            Self::Other => FileSystemSourceKindList(vec![
                FileSystemSourceKind::Image,
                FileSystemSourceKind::OsImage,
            ]),
            Self::Iso9660 | Self::Auto => {
                FileSystemSourceKindList(vec![FileSystemSourceKind::Adopted])
            }
            Self::Swap | Self::Tmpfs | Self::Overlay => {
                FileSystemSourceKindList(vec![FileSystemSourceKind::Create])
            }
        }
    }

    /// Returns whether a filesystem type can have a mountpoint.
    pub fn can_have_mountpoint(&self) -> bool {
        match self {
            Self::Ext4
            | Self::Xfs
            | Self::Vfat
            | Self::Ntfs
            | Self::Iso9660
            | Self::Tmpfs
            | Self::Overlay
            | Self::Auto => true,
            Self::Swap | Self::Other => false,
        }
    }

    /// Returns whether a filesystem type can be used with verity.
    pub fn supports_verity(&self) -> bool {
        match self {
            Self::Ext4 | Self::Xfs => true,
            Self::Vfat
            | Self::Iso9660
            | Self::Swap
            | Self::Ntfs
            | Self::Tmpfs
            | Self::Overlay
            | Self::Auto
            | Self::Other => false,
        }
    }
}

/// A function that extracts the value of a specific field from a block device.
type UniqueValueExtractor =
    Box<dyn for<'a> FnOnce(&'a HostConfigBlockDevice) -> Result<Option<&'a [u8]>, Error>>;

/// This impl block contains validation rules for specific block device kinds
impl BlkDevKind {
    /// Return information about fields that must be unique across all block
    /// devices of a type.
    ///
    /// Some block devices define fields that must be unique across all block
    /// devices of the same kind. This function returns a tuple of the field
    /// name, and field value (as bytes) for each field that must be unique.
    ///
    /// The caller will collect all these tuples and ensure the uniqueness of
    /// each field.
    ///
    /// The returned function expects to be called with the associated
    /// HostConfigBlockDevice variant, it will return an error if it gets called
    /// with the wrong variant.
    ///
    /// The returned function will return None if the field is not set on the
    /// block device. It will return Some(bytes) if the field is set.
    pub fn uniqueness_constraints(&self) -> Option<Vec<(&'static str, UniqueValueExtractor)>> {
        match self {
            Self::Disk => Some(vec![(
                "device",
                Box::new(|blkdev: &HostConfigBlockDevice| {
                    Ok(Some(blkdev.unwrap_disk()?.device.as_os_str().as_bytes()))
                }),
            )]),
            Self::Partition => None,
            Self::AdoptedPartition => Some(vec![
                (
                    "matchLabel",
                    Box::new(|blkdev: &HostConfigBlockDevice| {
                        Ok(blkdev
                            .unwrap_adopted_partition()?
                            .match_label
                            .as_ref()
                            .map(|s| s.as_bytes()))
                    }),
                ),
                (
                    "matchUuid",
                    Box::new(|blkdev: &HostConfigBlockDevice| {
                        Ok(blkdev
                            .unwrap_adopted_partition()?
                            .match_uuid
                            .as_ref()
                            .map(|u| u.as_bytes().as_slice()))
                    }),
                ),
            ]),
            Self::RaidArray => Some(vec![(
                "name",
                Box::new(|blkdev: &HostConfigBlockDevice| {
                    Ok(Some(blkdev.unwrap_raid_array()?.name.as_bytes()))
                }),
            )]),
            Self::ABVolume => None,
            Self::EncryptedVolume => Some(vec![(
                "deviceName",
                Box::new(|blkdev: &HostConfigBlockDevice| {
                    Ok(Some(
                        blkdev.unwrap_encrypted_volume()?.device_name.as_bytes(),
                    ))
                }),
            )]),
        }
    }
}

impl BlkDevKind {
    /// Returns whether a block device kind can have a partition type.
    ///
    /// This is used to avoid checking partition types for block device nodes
    /// that can't have them.
    pub(super) fn has_partition_type(self) -> bool {
        match self {
            Self::Disk | Self::AdoptedPartition => false,
            Self::Partition | Self::RaidArray | Self::ABVolume | Self::EncryptedVolume => true,
        }
    }
}

impl BlkDevReferrerKind {
    /// Returns whether to enforce homogeneous reference kinds for a given referrer
    /// kind.
    ///
    /// In other words, can a referrer kind refer to multiple target
    /// kinds? e.g a partition and a raid array?
    pub fn enforce_homogeneous_reference_kinds(&self) -> bool {
        match self {
            // Nothing to do.
            Self::None => false,

            // These should always have homogeneous reference kinds.
            Self::RaidArray | Self::ABVolume | Self::EncryptedVolume => true,

            // These only have one target, so enforcing this is meaningless.
            Self::FileSystem
            | Self::FileSystemOsImage
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FileSystemSysupdate => false,

            // Only enforce it for the data partition, which will check both.
            // Do not enforce for the hash partition to avoid checking twice.
            Self::VerityFileSystemData => true,
            Self::VerityFileSystemHash => false,
        }
    }

    /// Returns whether to enforce homogeneous partition sizes for a given referrer kind.
    pub fn enforce_homogeneous_partition_sizes(&self) -> bool {
        // If the referrer can't have multiple targets, there's nothing to enforce.
        if !self.valid_target_count().can_be_multiple() {
            return false;
        }

        match self {
            Self::RaidArray => true,
            Self::None | Self::ABVolume | Self::EncryptedVolume => false,

            // Filesystems are special referrers because they are not nodes.
            // These rules are not checked, but included here for completeness.
            Self::FileSystem
            | Self::FileSystemOsImage
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FileSystemSysupdate
            | Self::VerityFileSystemData
            | Self::VerityFileSystemHash => false,
        }
    }

    /// Returns whether to enforce homogeneous partition types for a given referrer kind.
    pub fn enforce_homogeneous_partition_types(&self) -> bool {
        // If the referrer can't have multiple targets, there's nothing to enforce.
        if !self.valid_target_count().can_be_multiple() {
            return false;
        }

        match self {
            Self::RaidArray | Self::ABVolume => true,
            Self::None | Self::EncryptedVolume => false,
            // Filesystems should always have homogeneous partition types.
            Self::FileSystem
            | Self::FileSystemOsImage
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FileSystemSysupdate
            | Self::VerityFileSystemData
            | Self::VerityFileSystemHash => true,
        }
    }

    /// Returns the valid partition types for a given referrer kind.
    pub fn allowed_partition_types(&self) -> AllowBlockList<PartitionType> {
        match self {
            Self::None => AllowBlockList::Any,
            Self::RaidArray => AllowBlockList::Any,
            Self::ABVolume => AllowBlockList::Any,
            Self::EncryptedVolume => AllowBlockList::Block(vec![
                PartitionType::Esp,
                PartitionType::Root,
                PartitionType::RootVerity,
            ]),
            Self::FileSystem | Self::FileSystemAdopted | Self::FileSystemSysupdate => {
                AllowBlockList::Block(vec![PartitionType::Esp])
            }
            Self::FileSystemOsImage => AllowBlockList::Any,
            Self::FileSystemEsp => AllowBlockList::Allow(vec![PartitionType::Esp]),
            Self::VerityFileSystemData => {
                // TODO: add Usr when it's supported
                AllowBlockList::Allow(vec![PartitionType::Root])
            }
            Self::VerityFileSystemHash => {
                // TODO: add UsrVerity when it's supported
                AllowBlockList::Allow(vec![PartitionType::RootVerity])
            }
        }
    }

    /// Checks for the targets of a given referrer kind.
    ///
    /// THESE CHECKS ARE NOT VERY DECLARATIVE, TRY TO MINIMIZE THEIR USE AND
    /// CHECK THEM IF ANY OF THE RULES ABOVE CHANGE.
    ///
    /// TRY TO KEEP ALL ASSUMPTIONS IN THIS FILE SO THEY ARE EASIER TO FIND AND
    /// CROSS-VALIDATE.
    ///
    /// This function checks that the targets of a given referrer kind are valid
    /// beyond the basic kind and count checks. You can assume these checks have
    /// already been performed.
    ///
    /// For example, here we check that the partition sizes of all targets of a
    /// RAID array are the same.
    pub(super) fn check_targets(
        &self,
        _node: &BlkDevNode,
        _targets: &[&BlkDevNode],
        _graph: &BlockDeviceGraph,
    ) -> Result<(), Error> {
        match self {
            Self::None => (),

            Self::RaidArray => (),

            Self::ABVolume => (),

            Self::EncryptedVolume => (),

            // Filesystems are special referrers because they are not nodes.
            // Because of that, they have their own set of rules that are not
            // covered here, and this section is unreachable.
            Self::FileSystem
            | Self::FileSystemOsImage
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FileSystemSysupdate
            | Self::VerityFileSystemData
            | Self::VerityFileSystemHash => (),
        }

        Ok(())
    }
}

impl PartitionType {
    /// Return known-valid and expected mountpoints for a partition type.
    pub fn valid_mountpoints(&self) -> ValidMountpoints {
        match self {
            Self::Esp => ValidMountpoints::new(&["/boot", "/efi", "/boot/efi"]),
            Self::Home => ValidMountpoints::new(&["/home"]),
            Self::LinuxGeneric => ValidMountpoints::Any,
            Self::Root => ValidMountpoints::new(&["/"]),
            Self::RootVerity => ValidMountpoints::None,
            Self::Srv => ValidMountpoints::new(&["/srv"]),
            Self::Swap => ValidMountpoints::None,
            Self::Tmp => ValidMountpoints::new(&["/var/tmp"]),
            Self::Usr => ValidMountpoints::new(&["/usr"]),
            Self::Var => ValidMountpoints::new(&["/var"]),
            Self::Xbootldr => ValidMountpoints::new(&["/boot"]),
        }
    }
}
