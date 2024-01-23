use anyhow::{bail, ensure, Context, Error};

use crate::config::{Partition, PartitionSize};

pub(super) fn check_partition_size_equals(partitions: &[&Partition]) -> Result<(), Error> {
    // Get the size of each partition, while ensuring that all partitions have a fixed size
    let sizes = partitions
        .iter()
        .map(|part| {
            if let PartitionSize::Fixed(size) = part.size {
                Ok(size)
            } else {
                bail!(
                    "RAID array references partition '{}', which does not have a fixed size.",
                    part.id
                );
            }
        })
        .collect::<Result<Vec<u64>, Error>>()
        .context("Not all members have fixed sizes.")?;

    // Ensure that all partitions have the same size
    //
    // Get the size of the first partition, then ensure that all other partitions have
    // the same size.
    let first_size = *sizes
        .first()
        .context("Failed to get first partition size.")?;

    ensure!(
        sizes.into_iter().all(|size| size == first_size),
        "RAID array references partitions with different sizes."
    );

    Ok(())
}
