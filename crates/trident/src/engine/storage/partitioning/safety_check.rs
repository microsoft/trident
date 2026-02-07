
use anyhow::{bail, ensure, Context, Error};
use log::{debug, info};

use osutils::{
    block_devices::ResolvedDisk,
    lsblk,
    sfdisk::SfDisk,
};

use super::adoption::PartitionAdopter;

/// Perform a runtime safety check.
///
/// This function will go through all requested disk changes to ensure that they
/// do not destroy partitions that are currently mounted.
pub(super) fn partitioning_safety_check(disks: &Vec<ResolvedDisk>) -> Result<(), Error> {
    // Validation has already verified that any disk with adopted partitions will have
    // a GPT partition table, so we can safely assume that here.
    debug!("Running partitioning safety check");

    for disk in disks {
        debug!("Running partitioning safety check for disk '{}'", disk.id);

        let blkdev_info =
            lsblk::get(&disk.dev_path).context("Failed to retrieve partition table information")?;

        // Figure out if anything in the disk is mounted.
        if blkdev_info.get_all_mountpoints_recursive().is_empty() {
            // Nothing is mounted, we can safely proceed.
            debug!("Disk '{}' has no mount points, proceeding...", disk.id);
            continue;
        }

        // We have mountpoints, so we can only proceed if the disk uses GPT partitioning.
        if blkdev_info.partition_table_type != Some(lsblk::PartitionTableType::Gpt) {
            // If the disk has mount points, but does not use GPT partitioning, we cannot proceed.
            bail!(
                "Disk '{}' has mount points, but does not use GPT partitioning [{:?}]. Refusing to proceed with partitioning.",
                disk.id,
                blkdev_info.partition_table_type,
            );
        }

        // If the disk itself is mounted we cannot proceed because we can only adopt partitions.
        if let Some(m) = blkdev_info.mountpoint {
            bail!(
                "Disk '{}' is currently mounted at '{}', cannot proceed with partitioning.",
                disk.id,
                m.display()
            );
        }

        let disk_info = SfDisk::get_info(&disk.dev_path).context(format!(
            "Failed to retrieve information for disk '{}', the partition table could be missing or corrupted.",
            disk.id
        ))?;

        let mut adopter = PartitionAdopter::new(&disk_info);

        // Try to perform matching for all adopted partitions.
        disk.spec
            .adopted_partitions
            .iter()
            .try_for_each(|adopted_part| {
                adopter
                    .adopt(adopted_part)
                    .context(format!("Failed to adopt partition '{}'", adopted_part.id))
            })?;

        // Ensure that none of the unmatched partitions or their children are mounted.
        adopter
            .get_unmatched_partitions()
            .try_for_each(|part| {
                debug!(
                    "Checking unmatched partition '{}' on disk '{}'",
                    part.node.display(),
                    disk.id
                );

                let Some(part_info) = lsblk::try_get(&part.node).with_context(|| {
                    format!(
                        "Failed to retrieve information for partition '{}' on disk '{}'.",
                        part.node.display(),
                        disk.id
                    )
                })? else {
                    // The kernel is not aware of this device, therefore it
                    // cannot have mount points. We can safely skip.
                    return Ok(());
                };

                // Check if the partition or its children are mounted.
                let mnt_points = part_info.get_all_mountpoints_recursive();
                ensure!(
                    mnt_points.is_empty(),
                    "Partition '{}' on disk '{}' was not adopted, but it and its children have mount points: {}",
                    part.node.display(),
                    disk.id,
                    mnt_points.iter().map(|mnt| mnt.to_string_lossy()).collect::<Vec<_>>().join(", "),
                );

                Ok(())
            })
            .context("Currently mounted partitions would be deleted by re-partitioning.")?;
    }

    info!("Partitioning safety check passed");
    Ok(())
}
