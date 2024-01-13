use anyhow::{bail, Error};

use crate::config::{Partition, PartitionType};

/// Ensure that a partition is of a type that supports encryption.
///
/// Disallowed types are:
/// - esp
/// - root
/// - root-verity
pub(super) fn check_partition_type_supports_encryption(part: &Partition) -> Result<(), Error> {
    if matches!(
        part.partition_type,
        PartitionType::Esp | PartitionType::Root | PartitionType::RootVerity
    ) {
        bail!(
            "Partition '{}' is of unsupported type '{:?}'.",
            part.id,
            part.partition_type
        );
    }
    Ok(())
}
