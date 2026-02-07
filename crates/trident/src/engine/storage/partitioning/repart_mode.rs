use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

use anyhow::{bail, ensure, Context, Error};
use gpt::disk;
use log::{debug, error, info, trace};

use osutils::{
    block_devices::{self, ResolvedDisk},
    lsblk,
    repart::{
        RepartActivity, RepartEmptyMode, RepartPartition, RepartPartitionEntry,
        SystemdRepartInvoker,
    },
    sfdisk::{SfDisk, SfPartition},
    udevadm,
};
use sysdefs::partition_types::DiscoverablePartitionType;
use trident_api::{
    config::{AdoptedPartition, Disk, PartitionSize, PartitionType},
    constants::internal_params::RAW_COSI_STORAGE,
    BlockDeviceId,
};
use uuid::Uuid;

use crate::engine::EngineContext;

use super::adoption::{self, PartitionAdopter};

pub fn create_partitions_on_disk(
    disk: &ResolvedDisk,
    partition_paths: &mut BTreeMap<BlockDeviceId, PathBuf>,
    disk_uuids: &mut HashMap<BlockDeviceId, Uuid>,
) -> Result<(), Error> {
    let mut repart = SystemdRepartInvoker::new(&disk.dev_path, RepartEmptyMode::Force);

    // If the disk has adopted partitions we need to match them and delete the rest.
    adoption::adopt_partitions(disk, &mut repart)
        .with_context(|| format!("Failed to adopt partitions for disk '{}'", disk.id))?;

    // Populate repart with entries for partitions that are to be created.
    add_repart_entries(&disk.spec, &mut repart);

    info!("Initializing '{}': creating disk partitions", disk.id);

    // Invoke repart to create the partitions.
    let repart_partitions = repart.execute().context(format!(
        "Failed to execute systemd-repart to create partitions for disk '{}'",
        disk.id
    ))?;

    // Check how many partitions were adopted by repart.
    let adopted_partition_count = repart_partitions
        .iter()
        .filter(|rp| rp.activity != RepartActivity::Create)
        .count();

    ensure!(
        adopted_partition_count == disk.spec.adopted_partitions.len(),
        "Expected {} partitions to be adopted, but {} were adopted",
        disk.spec.adopted_partitions.len(),
        adopted_partition_count
    );

    // Fix for #7911. When we adopt partitions, force kernel to re-read
    // the partition table. Bug #7911 is limited to adoption only, it never
    // reproduces on full repartitioning, so we only attempt it when we have
    // adopted partitions. `partx --update` requires the disk to have a
    // partition table and for it to have at least one partition. By
    // limiting this to adopted partitions > 0, we ensure that these
    // conditions are met.
    if adopted_partition_count > 0 {
        debug!(
            "Partitions were adopted, re-reading partition table for disk '{}'",
            disk.id
        );
        // If we fail to re-read the partition table, we log an error but
        // continue with the rest of the operation.
        let success = block_devices::partx_update(&disk.dev_path)
            .map_err(|e| {
                error!(
                    "Failed to re-read partition table for disk '{}': {:?}",
                    disk.id, e
                );
            })
            .is_ok();

        tracing::info!(metric_name = "partx_update_executed", value = success);
    }

    // Get the updated disk information.
    let disk_information = SfDisk::get_info(&disk.dev_path).context(format!(
        "Failed to retrieve information for disk '{}'",
        disk.id
    ))?;

    // Get disk UUID from osuuid
    match disk_information.id.as_uuid() {
        Some(disk_uuid) => {
            // Update the engine context with disk UUID to disk ID mapping
            disk_uuids.insert(disk.id.clone(), disk_uuid);
        }
        None => {
            debug!(
                "Expected UUID but found Osuuid::Relaxed {} for disk ID {}",
                disk_information.id, disk.id,
            );
        }
    }

    // Perform checks for all partitions.
    for repart_partition in repart_partitions.iter() {
        // Check that the expected partition symlinks exist.
        wait_for_part_symlink(repart_partition).with_context(|| {
            format!(
                "Could not find symlinks for partition '{}'",
                repart_partition.id
            )
        })?;

        // Update engine context with the partition metadata.
        trace!(
            "Updating engine context with partition '{}':\n{:#?}",
            repart_partition.id,
            repart_partition
        );
        partition_paths.insert(repart_partition.id.clone(), repart_partition.path_by_uuid());
    }
    Ok(())
}

/// Add repart entries for partitions that are to be created.
fn add_repart_entries(disk: &Disk, repart: &mut SystemdRepartInvoker) {
    for partition in &disk.partitions {
        let size = match partition.size {
            PartitionSize::Grow => None,
            PartitionSize::Fixed(s) => Some(s.bytes()),
        };

        repart.push_partition_entry(RepartPartitionEntry {
            // Store the BlockDeviceId in the id field.
            id: partition.id.clone(),

            // Inform repart about the partition type.
            partition_type: config_part_type_into_discoverable(partition.partition_type),

            // Use the configured label if present, otherwise use the partition id.
            label: Some(partition.label.as_ref().unwrap_or(&partition.id).clone()),

            // Copy over the Option<Uuid> from the partition config.
            uuid: partition.uuid,

            // Inform repart about the size of the partition.
            size_max_bytes: size,
            size_min_bytes: size,
        })
    }
}

fn config_part_type_into_discoverable(part_type: PartitionType) -> DiscoverablePartitionType {
    match part_type {
        PartitionType::Esp => DiscoverablePartitionType::Esp,
        PartitionType::Home => DiscoverablePartitionType::Home,
        PartitionType::LinuxGeneric => DiscoverablePartitionType::LinuxGeneric,
        PartitionType::Root => DiscoverablePartitionType::Root,
        PartitionType::RootVerity => DiscoverablePartitionType::RootVerity,
        PartitionType::Srv => DiscoverablePartitionType::Srv,
        PartitionType::Swap => DiscoverablePartitionType::Swap,
        PartitionType::Tmp => DiscoverablePartitionType::Tmp,
        PartitionType::Usr => DiscoverablePartitionType::Usr,
        PartitionType::UsrVerity => DiscoverablePartitionType::UsrVerity,
        PartitionType::Var => DiscoverablePartitionType::Var,
        PartitionType::Xbootldr => DiscoverablePartitionType::Xbootldr,
        PartitionType::Unknown(uuid) => DiscoverablePartitionType::from_uuid(&uuid),
    }
}

/// Wait for a partition's path by partuuid to appear.
pub(super) fn wait_for_part_symlink(repart_partition: &RepartPartition) -> Result<PathBuf, Error> {
    let part_path = repart_partition.path_by_uuid();
    udevadm::wait(&part_path).context(format!(
        "Failed waiting for '{}' to appear",
        part_path.display()
    ))?;

    ensure!(
        part_path.exists(),
        "Partition '{}' symlink '{}' does not exist",
        repart_partition.id,
        part_path.display()
    );

    Ok(part_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    use uuid::Uuid;

    use osutils::sfdisk::{SfDiskLabel, SfDiskUnit};
    use trident_api::config::{Partition, PartitionTableType};

    #[test]
    fn test_add_repart_entries() {
        let mut repart = SystemdRepartInvoker::new("/dev/sda", RepartEmptyMode::Force);

        let uuid = Uuid::parse_str("123e4567-e89b-12d3-a456-426614174000").unwrap();

        let disk = Disk {
            id: "disk".to_string(),
            device: PathBuf::from("/dev/sda"),
            partitions: vec![
                Partition {
                    id: "part1".to_string(),
                    partition_type: PartitionType::Root,
                    size: 1024.into(),
                    uuid: None,
                    label: None,
                },
                Partition {
                    id: "part2".to_string(),
                    partition_type: PartitionType::Swap,
                    size: 2048.into(),
                    uuid: None,
                    label: Some("".to_string()),
                },
                Partition {
                    id: "part3".to_string(),
                    partition_type: PartitionType::LinuxGeneric,
                    size: PartitionSize::Grow,
                    uuid: Some(uuid),
                    label: Some("my-super-part-label".to_string()),
                },
            ],
            adopted_partitions: vec![],
            partition_table_type: PartitionTableType::Gpt,
        };

        add_repart_entries(&disk, &mut repart);

        let entries = repart.partition_entries();
        assert_eq!(entries.len(), 3);

        let part1 = entries.first().unwrap();
        assert_eq!(part1.id, "part1");
        assert_eq!(part1.partition_type, DiscoverablePartitionType::Root);
        assert_eq!(part1.label, Some("part1".to_string()));
        assert_eq!(part1.uuid, None);
        assert_eq!(part1.size_max_bytes, Some(1024));
        assert_eq!(part1.size_min_bytes, Some(1024));

        let part2 = entries.get(1).unwrap();
        assert_eq!(part2.id, "part2");
        assert_eq!(part2.partition_type, DiscoverablePartitionType::Swap);
        assert_eq!(part2.label, Some("".to_string()));
        assert_eq!(part2.uuid, None);
        assert_eq!(part2.size_max_bytes, Some(2048));
        assert_eq!(part2.size_min_bytes, Some(2048));

        let part3 = entries.get(2).unwrap();
        assert_eq!(part3.id, "part3");
        assert_eq!(
            part3.partition_type,
            DiscoverablePartitionType::LinuxGeneric
        );
        assert_eq!(part3.label, Some("my-super-part-label".to_string()));
        assert_eq!(part3.uuid, Some(uuid));
        assert_eq!(part3.size_max_bytes, None);
        assert_eq!(part3.size_min_bytes, None);
    }

    #[test]
    fn test_partitioning_using_uuid() {
        let mut repart = SystemdRepartInvoker::new("/dev/sda", RepartEmptyMode::Force);

        let disk = Disk {
            id: "disk".to_string(),
            device: PathBuf::from("/dev/sda"),
            partitions: vec![
                Partition {
                    id: "part1".to_string(),
                    // UUID for ESP Partition
                    partition_type: PartitionType::Unknown(
                        Uuid::parse_str("c12a7328f81f11d2ba4b00a0c93ec93b").unwrap(),
                    ),
                    size: 1024.into(),
                    uuid: None,
                    label: None,
                },
                Partition {
                    id: "part2".to_string(),
                    // UUID for LinuxGeneric Partition
                    partition_type: PartitionType::Unknown(
                        Uuid::parse_str("0fc63daf848347728e793d69d8477de4").unwrap(),
                    ),
                    size: PartitionSize::Grow,
                    uuid: None,
                    label: None,
                },
            ],
            adopted_partitions: vec![],
            partition_table_type: PartitionTableType::Gpt,
        };

        add_repart_entries(&disk, &mut repart);

        let entries = repart.partition_entries();
        assert_eq!(entries.len(), 2);

        let part1 = entries.first().unwrap();
        assert_eq!(part1.partition_type, DiscoverablePartitionType::Esp);

        let part2 = entries.get(1).unwrap();
        assert_eq!(
            part2.partition_type,
            DiscoverablePartitionType::LinuxGeneric
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::str::FromStr;

    use osutils::{
        repart::RepartActivity,
        testutils::repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH},
        wipefs,
    };
    use pytest_gen::functional_test;
    use trident_api::config::{HostConfiguration, Partition, PartitionTableType, Storage};

    #[functional_test]
    fn test_wait_for_part_symlink() {
        // Get the first partition from /dev/sda.
        let demo_part = SfDisk::get_info(OS_DISK_DEVICE_PATH)
            .unwrap()
            .partitions
            .pop()
            .unwrap();
        let repart = RepartPartition {
            id: "part1".to_string(),
            partition_type: demo_part.partition_type,
            label: demo_part.name.clone(),
            uuid: demo_part.id.as_uuid().unwrap(),
            file: PathBuf::from("/some/file"),
            node: demo_part.node.clone(),
            start: demo_part.start,
            size: demo_part.size,
            activity: RepartActivity::Unchanged,
        };

        let res = wait_for_part_symlink(&repart).unwrap();

        assert_eq!(res, demo_part.path_by_uuid());
    }
}
