use std::{
    collections::{BTreeMap, HashMap},
    fs::OpenOptions,
};

use anyhow::{Context, Error};
use gpt::GptConfig;

use osutils::block_devices::{self, ResolvedDisk};

use crate::engine::EngineContext;

/// This function handles the partitioning logic for raw COSI storage mode. In
/// this mode, we expect exactly one disk with no partitions, and we create a
/// single partition that takes up the entire disk. This is a simplified flow
/// that is only used for raw COSI storage, and it allows us to use the same
/// underlying partitioning code while bypassing the usual validation and
/// adoption logic that is not relevant in this mode.
pub(super) fn create_partitions_for_raw_cosi_storage(
    ctx: &mut EngineContext,
    disk: &ResolvedDisk,
) -> Result<(), Error> {
    let raw_gpt = {
        ctx.image
            .as_mut()
            .context("An image is needed for raw partitioning mode")?
            .gpt()
            .context("Failed to get GPT data from image for raw partitioning mode")?
            .context("Image does not provide raw GPT data")?
    };

    // Generate a mapping from partition UUID to partition ID for the disk in the Host Configuration.
    let device_id_by_part_uuid = disk
        .spec
        .partitions
        .iter()
        .filter_map(|part| part.uuid.as_ref().map(|uuid| (*uuid, &part.id)))
        .collect::<BTreeMap<_, _>>();

    // Before we actually touch the disk, stage the disk and partition
    // information we will add to EngineContext, so that we may catch
    // correspondence issues early. Note: we won't store these into the
    // EngineContext until after we've successfully created the GPT on disk,
    // since that's the point of no return for making changes to the disk.

    // First, the disk DeviceId -> UUID mapping.
    let staged_disk = [(disk.id.clone(), *raw_gpt.guid())];

    // Then, the partition DeviceId -> disk by partition UUID mapping.
    let staged_partitions = {
        let mut tmp = HashMap::new();
        for raw_part in raw_gpt.partitions().values() {
            let part_device_id = device_id_by_part_uuid
                .get(&raw_part.part_guid)
                .with_context(|| {
                    format!(
                        "Partition with UUID '{}' from raw GPT does not match any partition UUID in the Host Configuration",
                        raw_part.part_guid
                    )
                })?;

            tmp.insert(
                part_device_id.to_owned().to_owned(),
                block_devices::part_uuid_path(raw_part.part_guid),
            );
        }

        tmp
    };

    // Now let's try to open the disk as a file!
    let mut disk_device = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&disk.dev_path)
        .with_context(|| {
            format!(
                "Failed to open disk device at path '{}' for repartitioning",
                disk.dev_path.display()
            )
        })?;

    // Create the new GPT on the disk using the raw GPT data from the image.
    // This will overwrite any existing partitions on the disk.
    let mut new_gpt = GptConfig::new()
        .writable(true)
        .change_partition_count(true)
        .logical_block_size(*raw_gpt.logical_block_size())
        .create_from_device(&mut disk_device, Some(*raw_gpt.guid()))
        .context("Failed to create GPT from disk device in raw partitioning mode")?;

    // Now start replicating partitions!
    new_gpt
        .update_partitions(raw_gpt.partitions().clone())
        .context("Failed to update partitions in raw partitioning mode")?;

    new_gpt
        .write()
        .context("Failed to write new GPT to disk in raw partitioning mode")?;

    disk_device
        .sync_all()
        .context("Failed to sync disk device after writing GPT in raw partitioning mode")?;

    // If we got here, then the GPT has been successfully written to disk, so we
    // can now commit the staged disk and partition information to the
    // EngineContext.
    ctx.partition_paths.extend(staged_partitions);
    ctx.disk_uuids.extend(staged_disk);

    Ok(())
}
