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

use anyhow::{bail, Context, Error, Ok};

use crate::{config::Partition, constants::SWAP_MOUNT_POINT};

use super::{
    cardinality::ValidCardinality,
    graph::BlockDeviceGraph,
    types::{
        BlkDevKindFlag, BlkDevNode, BlkDevReferrerKind, BlkDevReferrerKindFlag,
        HostConfigBlockDevice,
    },
};

mod encrypted;
mod raid;

/// This impl block contains validation rules for host-config objects
impl<'a> HostConfigBlockDevice<'a> {
    /// Checks basic context-free attributes of the block device
    ///
    /// Use this function to check attributes that do not depend on the graph,
    /// just simple rules & attributes that must be met for each block device
    /// kind.
    pub(super) fn basic_check(&self) -> Result<(), Error> {
        match self {
            HostConfigBlockDevice::Disk(_) => (),
            HostConfigBlockDevice::Partition(_) => (),
            HostConfigBlockDevice::AdoptedPartition => (),
            HostConfigBlockDevice::RaidArray(_) => (),
            HostConfigBlockDevice::ABVolume(_) => (),
            HostConfigBlockDevice::EncryptedVolume(_) => (),
        }

        Ok(())
    }
}

/// This impl block contains validation rules for block device referrers
impl BlkDevReferrerKind {
    /// Returns the valid target kinds for the referrer kind.
    ///
    /// This table shows the valid block device kinds that can be referenced by each referrer:
    ///
    /// | Referrer \ Target Kind | Disk | Partition | AdoptedPartition | RaidArray | ABVolume | EncryptedVolume |
    /// | ---------------------- | ---- | --------- | ---------------- | --------- | -------- | --------------- |
    /// | **Disk**               | N/A  | N/A       | N/A              | N/A       | N/A      | N/A             |
    /// | **Partition**          | N/A  | N/A       | N/A              | N/A       | N/A      | N/A             |
    /// | **AdoptedPartition**   | N/A  | N/A       | N/A              | N/A       | N/A      | N/A             |
    /// | **RaidArray**          | No   | Yes       | TBD              | No        | No       | No              |
    /// | **ABVolume**           | No   | Yes       | TBD              | Yes       | No       | Yes             |
    /// | **EncryptedVolume**    | No   | Yes       | TBD              | Yes       | No       | No              |
    /// | **Image**              | No   | Yes       | TBD              | Yes       | Yes      | Yes             |
    /// | **ImageSysupdate**     | No   | No        | TBD              | No        | Yes      | No              |
    /// | **MountPoint**         | No   | Yes       | TBD              | Yes       | Yes      | Yes             |
    pub(crate) fn valid_target_kinds(self) -> BlkDevKindFlag {
        match self {
            Self::None => BlkDevKindFlag::empty(),
            Self::RaidArray => BlkDevKindFlag::Partition,
            Self::ABVolume => {
                BlkDevKindFlag::Partition
                    | BlkDevKindFlag::RaidArray
                    | BlkDevKindFlag::EncryptedVolume
            }
            Self::EncryptedVolume => BlkDevKindFlag::Partition | BlkDevKindFlag::RaidArray,
            Self::Image => {
                BlkDevKindFlag::Partition
                    | BlkDevKindFlag::RaidArray
                    | BlkDevKindFlag::EncryptedVolume
                    | BlkDevKindFlag::ABVolume
            }
            Self::ImageSysupdate => BlkDevKindFlag::ABVolume,
            Self::MountPoint => {
                BlkDevKindFlag::Partition
                    | BlkDevKindFlag::AdoptedPartition
                    | BlkDevKindFlag::EncryptedVolume
                    | BlkDevKindFlag::ABVolume
                    | BlkDevKindFlag::RaidArray
            }
        }
    }

    /// Returns the valid number of members for the referrer kind
    ///
    /// This table shows the valid number of members for each referrer:
    ///
    /// | Referrer Type        | Min | Max |
    /// | -------------------- | --- | --- |
    /// | **Disk**             | 0   | 0   |
    /// | **Partition**        | 0   | 0   |
    /// | **AdoptedPartition** | 0   | 0   |
    /// | **RaidArray**        | 2   | ∞   |
    /// | **ABVolume**         | 2   | 2   |
    /// | **EncryptedVolume**  | 1   | 1   |
    /// | **Image**            | 1   | 1   |
    /// | **MountPoint**       | 1   | 1   |
    ///
    /// (Above ranges are inclusive)
    pub(crate) fn valid_target_count(self) -> ValidCardinality {
        match self {
            Self::None => ValidCardinality::new_zero(),
            Self::RaidArray => ValidCardinality::new_at_least(2),
            Self::ABVolume => ValidCardinality::new_exact(2),
            Self::EncryptedVolume => ValidCardinality::new_exact(1),
            // These two are not really used, but we define them for
            // completeness
            Self::Image => ValidCardinality::new_exact(1),
            Self::ImageSysupdate => ValidCardinality::new_exact(1),
            Self::MountPoint => ValidCardinality::new_exact(1),
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
    /// NOTE: Images and mount points are special referrers that follow
    /// additional rules not covered here:
    /// 1. Image kinds can never share with any other image kind. (Only one
    ///    image slot!)
    /// 2. Mount points can always share with any number of other mount points
    ///   **implicitly**!
    ///
    /// The reason for this is that images and mount points are not block
    /// devices, so they get their own respective fields in the BlkDevNode
    /// object.
    /// - #1 above is enforced by the node struct having only an `Option` to
    ///   store the image associated with it.
    /// - #2 follows from the node struct using a `Vec` to store mount points.
    pub(crate) fn valid_sharing_peers(self) -> BlkDevReferrerKindFlag {
        match self {
            Self::None => BlkDevReferrerKindFlag::empty(),
            Self::RaidArray => BlkDevReferrerKindFlag::empty(),
            Self::ABVolume => BlkDevReferrerKindFlag::empty(),
            Self::EncryptedVolume => BlkDevReferrerKindFlag::empty(),
            Self::Image => BlkDevReferrerKindFlag::MountPoint,
            Self::ImageSysupdate => BlkDevReferrerKindFlag::MountPoint,
            Self::MountPoint => BlkDevReferrerKindFlag::AnyImage,
        }
    }
}

/// Mount points generally should refer to paths, however, there are certain exceptions, which are listed here.
pub(super) const VALID_NON_PATH_MOUNT_POINTS: [&str; 1] = [SWAP_MOUNT_POINT];

/// This impl block contains validation rules for host-config objects
impl<'a> HostConfigBlockDevice<'a> {
    /// Return information about fields that must be unique.
    ///
    /// Some block devices define fields that must be unique across all block devices of the same
    /// kind. This function returns a tuple of the field name, and field value (as bytes) for each
    /// field that must be unique.
    ///
    /// The caller will collect all these tuples and ensure the uniqueness of each field
    pub(super) fn uniqueness_constraints(&self) -> Option<Vec<(&'static str, &[u8])>> {
        match self {
            Self::Disk(disk) => Some(vec![("device", disk.device.as_os_str().as_bytes())]),
            HostConfigBlockDevice::Partition(_) => None,
            // Will be implemented in the future
            HostConfigBlockDevice::AdoptedPartition => todo!("AdoptedPartition unique fields"),
            HostConfigBlockDevice::RaidArray(raid_array) => {
                Some(vec![("name", raid_array.name.as_bytes())])
            }
            HostConfigBlockDevice::ABVolume(_) => None,
            HostConfigBlockDevice::EncryptedVolume(enc_vol) => {
                Some(vec![("deviceName", enc_vol.device_name.as_bytes())])
            }
        }
    }
}

impl BlkDevReferrerKind {
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
        targets: &[&BlkDevNode],
        graph: &BlockDeviceGraph,
    ) -> Result<(), Error> {
        match self {
            Self::None => (),
            Self::RaidArray => raid::check_partition_size_equals(
                &targets
                    .iter()
                    // Assumption: all targets are partitions
                    .map(|target| target.host_config_ref.unwrap_partition())
                    .collect::<Result<Vec<&Partition>, Error>>()
                    .context("Failed to get partitions for RAID array.")?,
            )?,
            Self::ABVolume => (),
            Self::EncryptedVolume => {
                // Assumption: just one target exists.
                // We already validated that targets.len() == 1
                let target = targets[0];
                match target.host_config_ref {
                    // If the target is a partition, ensure it is of an
                    // acceptable type
                    HostConfigBlockDevice::Partition(part) => {
                        encrypted::check_partition_type_supports_encryption(part)?;
                    }
                    // If the target is a RAID array, ensure all its underlying
                    // partitions are of an acceptable type
                    HostConfigBlockDevice::RaidArray(_) => {
                        graph
                            .targets(&target.id)
                            .context(format!(
                                "Failed to get targets for RAID array '{}'.",
                                target.id
                            ))?
                            .iter()
                            // Assumption: all targets are partitions
                            .map(|target| target.host_config_ref.unwrap_partition())
                            .collect::<Result<Vec<&Partition>, Error>>()
                            .context(format!(
                                "Failed to get partitions for RAID array '{}'.",
                                target.id
                            ))?
                            .into_iter()
                            .try_for_each(encrypted::check_partition_type_supports_encryption)
                            .context("Encrypted volume references invalid RAID array.")?;
                    }

                    // Assumption: all other types are invalid
                    _ => bail!(
                        "Encrypted volume references block device of invalid kind '{}'.",
                        target.id
                    ),
                }
            }
            Self::Image => (),
            Self::ImageSysupdate => (),
            Self::MountPoint => (),
        }

        Ok(())
    }
}
