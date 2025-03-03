//! # Rule Declarations
//!
//! This module contains all the per-kind validation rules for block devices &
//! nodes. Generic rules that apply to all are covered directly in the build()
//! function of StorageGraphBuilder. (e.g. uniqueness of IDs)
//!
//! The rules declared in this section are used by StorageGraphBuilder to
//! validate particular constraints and requirements associated with specific
//! devices, filesystems, referrers, etc.
//!
//! The rules are declared roughly in the order they are evaluated.

use std::{
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Error};

use crate::{
    config::{
        FileSystemType, HostConfigurationStaticValidationError, Partition, PartitionSize,
        PartitionType, RaidLevel,
    },
    constants::ESP_MOUNT_POINT_PATH,
};

use super::{
    cardinality::ValidCardinality,
    containers::{AllowBlockList, ItemList},
    graph::{NodeIndex, StoragePetgraph},
    node::StorageGraphNode,
    references::SpecialReferenceKind,
    types::{
        BlkDevKind, BlkDevKindFlag, BlkDevReferrerKind, BlkDevReferrerKindFlag,
        FileSystemSourceKind, HostConfigBlockDevice,
    },
};

// TODO for the future: Add a trait for reference rules or some better way to
// organize rules.

// trait ReferenceRule<T> {
//     fn name(&self) -> &'static str;
//     fn description(&self) -> &'static str;
//     fn definition(kind: BlkDevReferrerKind) -> T;
//     fn special_definition(kind: SpecialReferenceKind) -> Option<T>;
// }

/// This impl block contains validation rules for host-config objects
impl HostConfigBlockDevice {
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
            Self::VerityDevice(_) => (),
        }

        Ok(())
    }
}

impl FileSystemType {
    /// Returns whether a filesystem type expects a block device ID.
    ///
    /// If true, the filesystem type must have a block device ID.
    /// If false, the filesystem type must not have a block device ID.
    pub fn expects_block_device_id(&self) -> bool {
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

    /// Returns whether a filesystem type must have a mountpoint.
    pub fn must_have_mountpoint(&self) -> bool {
        // Something that does not have a block device and can have a
        // mountpoint, must have a mountpoint. Today this covers only Tmpfs and
        // Overlay. These NEED to be mounted otherwise they don't really exist.
        !self.expects_block_device_id() && self.can_have_mountpoint()
    }

    /// Returns the valid sources for a filesystem type.
    pub fn valid_sources(&self) -> ItemList<FileSystemSourceKind> {
        match self {
            Self::Ext4 | Self::Xfs | Self::Ntfs => ItemList(vec![
                FileSystemSourceKind::New,
                FileSystemSourceKind::Image,
                FileSystemSourceKind::Adopted,
                FileSystemSourceKind::OsImage,
            ]),
            Self::Vfat => ItemList(vec![
                FileSystemSourceKind::New,
                FileSystemSourceKind::Image,
                FileSystemSourceKind::Adopted,
                FileSystemSourceKind::EspBundle,
                FileSystemSourceKind::OsImage,
            ]),
            Self::Other => ItemList(vec![
                FileSystemSourceKind::Image,
                FileSystemSourceKind::OsImage,
            ]),
            Self::Iso9660 | Self::Auto => ItemList(vec![FileSystemSourceKind::Adopted]),
            Self::Swap | Self::Tmpfs | Self::Overlay => ItemList(vec![FileSystemSourceKind::New]),
        }
    }

    /// Returns whether a filesystem type can be used with verity.
    pub fn supports_verity(&self) -> bool {
        // If a filesystem cannot be mounted, by default it cannot be used with
        // verity.
        if !self.can_have_mountpoint() {
            return false;
        }

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

/// This impl block contains validation rules for block device referrers
impl BlkDevReferrerKind {
    /// Returns the valid number of members for the referrer kind.
    ///
    /// This table shows the valid number of members for each referrer:
    pub fn valid_target_count(self) -> ValidCardinality {
        match self {
            Self::None => ValidCardinality::new_zero(),
            Self::RaidArray => ValidCardinality::new_at_least(2),
            Self::ABVolume => ValidCardinality::new_exact(2),
            Self::EncryptedVolume => ValidCardinality::new_exact(1),
            Self::VerityDevice => ValidCardinality::new_exact(2),
            Self::FileSystem => ValidCardinality::new_at_most(1),
            Self::FileSystemEsp => ValidCardinality::new_exact(1),
            Self::FileSystemAdopted => ValidCardinality::new_exact(1),
            Self::FilesystemVerity => ValidCardinality::new_exact(2),
            Self::FileSystemOsImage => ValidCardinality::new_exact(1),
        }
    }

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
                    | BlkDevKindFlag::VerityDevice
            }
            Self::FileSystemEsp => {
                BlkDevKindFlag::Partition
                    | BlkDevKindFlag::AdoptedPartition
                    | BlkDevKindFlag::RaidArray
            }
            Self::FileSystemAdopted => BlkDevKindFlag::AdoptedPartition,
            Self::VerityDevice | Self::FilesystemVerity => {
                BlkDevKindFlag::Partition | BlkDevKindFlag::RaidArray | BlkDevKindFlag::ABVolume
            }
        }
    }
}

impl SpecialReferenceKind {
    /// Optionally returns a list of further restrictions on compatible kinds
    /// enforced by a special reference kind.
    pub fn compatible_kinds(&self) -> Option<BlkDevKindFlag> {
        match self {
            // Verity data/hash do not impose any additional restrictions.
            Self::VerityDataDevice => None,
            Self::VerityHashDevice => None,
        }
    }
}

impl BlkDevReferrerKind {
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
    pub fn valid_sharing_peers(self) -> BlkDevReferrerKindFlag {
        match self {
            Self::None
            | Self::RaidArray
            | Self::ABVolume
            | Self::EncryptedVolume
            | Self::VerityDevice
            | Self::FileSystem
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FilesystemVerity
            | Self::FileSystemOsImage => BlkDevReferrerKindFlag::empty(),
        }
    }

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
            Self::RaidArray
            | Self::ABVolume
            | Self::EncryptedVolume
            | Self::VerityDevice
            | Self::FilesystemVerity => true,

            // These only have one target, so enforcing this is meaningless.
            Self::FileSystem
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FileSystemOsImage => false,
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
            Self::None => None,
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
            Self::VerityDevice => Some(vec![(
                "name",
                Box::new(|blkdev: &HostConfigBlockDevice| {
                    Ok(Some(blkdev.unwrap_verity_device()?.name.as_bytes()))
                }),
            )]),
        }
    }
}

impl BlkDevReferrerKind {
    /// Returns whether to enforce homogeneous partition sizes for a given referrer kind.
    ///
    /// **NOTE:** this check is performed transitively. The graph is followed as a tree to discover
    /// all underlying partition types.
    pub fn enforce_homogeneous_partition_sizes(&self) -> bool {
        match self {
            // None referrer does not have any requirements.
            Self::None => false,

            // RAID arrays need all members to have the same size.
            Self::RaidArray => true,

            // AB volumes need all members to have the same size.
            Self::ABVolume => true,

            // Verity allows for data and hash devices to have different sizes.
            Self::VerityDevice | Self::FilesystemVerity => false,

            // These don't really care about partition sizes.
            Self::EncryptedVolume
            | Self::FileSystem
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FileSystemOsImage => false,
        }
    }

    /// Returns whether to enforce homogeneous partition types for a given referrer kind.
    ///
    /// **NOTE:** this check is performed transitively. The graph is followed as a tree to discover
    /// all underlying partition types.
    pub fn enforce_homogeneous_partition_types(&self) -> bool {
        match self {
            // None referrer does not have any requirements.
            Self::None => false,

            // These need all members to be basically the same.
            Self::RaidArray | Self::ABVolume => true,

            // Verity devices *expect* heterogeneous partition types.
            Self::FilesystemVerity | Self::VerityDevice => false,

            // These care about having all underlying partitions be of the same
            // type.
            Self::EncryptedVolume
            | Self::FileSystem
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FileSystemOsImage => true,
        }
    }
}

impl SpecialReferenceKind {
    /// Returns whether to enforce homogeneous partition types for a given special relationship kind.
    ///
    /// **NOTE:** this check is performed transitively. The graph is followed as a tree to discover
    /// all underlying partition types.
    pub fn enforce_homogeneous_partition_types(&self) -> Option<bool> {
        match self {
            Self::VerityDataDevice => Some(true),
            Self::VerityHashDevice => Some(true),
        }
    }
}

impl BlkDevReferrerKind {
    /// Returns the valid partition types for a given referrer kind.
    ///
    /// **NOTE:** this check is performed transitively. The graph is followed as a tree to discover
    /// all underlying partition types.
    pub fn allowed_partition_types(&self) -> AllowBlockList<PartitionType> {
        match self {
            Self::None => AllowBlockList::Any,
            Self::RaidArray => AllowBlockList::Any,
            Self::ABVolume => AllowBlockList::Any,
            Self::EncryptedVolume => AllowBlockList::Block(vec![
                PartitionType::Esp,
                PartitionType::Root,
                PartitionType::RootVerity,
                // Blocking the home partition type is a temporary
                // workaround for a conflict between
                // systemd-gpt-auto-generator and
                // systemd-cryptsetup-generator, where the former will
                // generate a faulty systemd unit file for the encrypted
                // volume if it detects that a home partition is
                // encrypted. Remove this when
                // https://dev.azure.com/mariner-org/ECF/_workitems/edit/9752
                // is completed.
                PartitionType::Home,
            ]),
            Self::FileSystem | Self::FileSystemAdopted => {
                AllowBlockList::Block(vec![PartitionType::Esp])
            }
            Self::FileSystemEsp => AllowBlockList::Allow(vec![PartitionType::Esp]),
            Self::FilesystemVerity | Self::VerityDevice => {
                // TODO: Add usr when it's supported.
                AllowBlockList::Allow(vec![
                    PartitionType::Root,
                    PartitionType::RootVerity,
                    PartitionType::LinuxGeneric,
                ])
            }
            Self::FileSystemOsImage => AllowBlockList::Any,
        }
    }
}

impl SpecialReferenceKind {
    /// Returns the valid partition types for a given special relationship kind.
    ///
    /// **NOTE:** this check is performed transitively. The graph is followed as a tree to discover
    /// all underlying partition types.
    pub fn allowed_partition_types(&self) -> Option<AllowBlockList<PartitionType>> {
        match self {
            Self::VerityDataDevice => Some(AllowBlockList::Allow(vec![
                PartitionType::Root,
                PartitionType::LinuxGeneric,
            ])),
            Self::VerityHashDevice => Some(AllowBlockList::Allow(vec![
                PartitionType::RootVerity,
                PartitionType::LinuxGeneric,
            ])),
        }
    }
}

/// Returns the expected partition type for a given mount point, if any.
pub fn expected_partition_type(mount_point: &Path) -> AllowBlockList<PartitionType> {
    if mount_point == Path::new(ESP_MOUNT_POINT_PATH) {
        return AllowBlockList::new_allow([PartitionType::Esp]);
    }

    AllowBlockList::Any
}

impl PartitionType {
    /// Return known-valid and expected mountpoints for a partition type.
    pub fn valid_mountpoints(&self) -> AllowBlockList<PathBuf> {
        match self {
            Self::Esp => AllowBlockList::new_allow(["/boot", "/efi", "/boot/efi"]),
            Self::Home => AllowBlockList::new_allow(["/home"]),
            Self::LinuxGeneric => AllowBlockList::Any,
            Self::Root => AllowBlockList::new_allow(["/"]),
            Self::RootVerity => AllowBlockList::None,
            Self::Srv => AllowBlockList::new_allow(["/srv"]),
            Self::Swap => AllowBlockList::None,
            Self::Tmp => AllowBlockList::new_allow(["/var/tmp"]),
            Self::Usr => AllowBlockList::new_allow(["/usr"]),
            Self::Var => AllowBlockList::new_allow(["/var"]),
            Self::Xbootldr => AllowBlockList::new_allow(["/boot"]),
            Self::Unknown(_) => AllowBlockList::Any,
        }
    }
}

impl BlkDevReferrerKind {
    /// Returns the valid RAID levels for a given referrer kind.
    ///
    /// **NOTE:** this check is not performed transitively. It only checks direct references.
    pub fn allowed_raid_levels(&self) -> Option<AllowBlockList<RaidLevel>> {
        if !self.compatible_kinds().contains(BlkDevKindFlag::RaidArray) {
            return None;
        }

        Some(match self {
            // ESP can only use RAID1
            Self::FileSystemEsp => AllowBlockList::Allow(vec![RaidLevel::Raid1]),

            // All other referrers that allow RAID can use any level.
            _ => AllowBlockList::Any,
        })
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
        _node_idx: NodeIndex,
        _node: &StorageGraphNode,
        _graph: &StoragePetgraph,
    ) -> Result<(), Error> {
        match self {
            Self::None
            | Self::RaidArray
            | Self::ABVolume
            | Self::EncryptedVolume
            | Self::VerityDevice
            | Self::FileSystem
            | Self::FileSystemEsp
            | Self::FileSystemAdopted
            | Self::FilesystemVerity
            | Self::FileSystemOsImage => (),
        }

        Ok(())
    }
}
