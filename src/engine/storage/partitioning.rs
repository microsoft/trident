use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, error, info, trace};

use osutils::{
    block_devices::{self, ResolvedDisk},
    lsblk,
    partition_types::DiscoverablePartitionType,
    repart::{
        RepartActivity, RepartEmptyMode, RepartPartition, RepartPartitionEntry,
        SystemdRepartInvoker,
    },
    sfdisk::{SfDisk, SfPartition},
    udevadm,
};
use trident_api::{
    config::{AdoptedPartition, Disk, HostConfiguration, PartitionSize, PartitionType, Storage},
    status::HostStatus,
    BlockDeviceId,
};

/// Given a host configuration, adopt and create partitions on the disks.
#[tracing::instrument(name = "partitions_creation", skip_all)]
pub fn create_partitions(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    // Resolve the disk paths to ensure that all disks in the configuration exist.
    let resolved_disks =
        block_devices::get_resolved_disks(host_config).context("Failed to resolve disk paths")?;

    // Do a non-destructive first pass of adoption to detect any issues before
    // we start making changes.
    partitioning_safety_check(&resolved_disks).context("Partitioning safety check failed")?;

    for disk in &resolved_disks {
        create_partitions_on_disk(host_status, host_config, disk)
            .with_context(|| format!("Failed to create partitions for disk '{}'", disk.id))?;
    }
    Ok(())
}

pub fn create_partitions_on_disk(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
    disk: &ResolvedDisk,
) -> Result<(), Error> {
    let mut repart = SystemdRepartInvoker::new(&disk.bus_path, RepartEmptyMode::Force);

    // If the disk has adopted partitions we need to match them and delete the rest.
    adopt_partitions(disk, &mut repart)
        .with_context(|| format!("Failed to adopt partitions for disk '{}'", disk.id))?;

    // Populate repart with entries for partitions that are to be created.
    add_repart_entries(
        disk.spec,
        &generate_sysupdate_partlabels(&host_config.storage),
        &mut repart,
    );

    info!("Creating partitions for disk '{}'", disk.id);

    // Invoke repart to create the partitions.
    let repart_partitions = repart.execute().context(format!(
        "Failed to create partitions for disk '{}'",
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
        let success = block_devices::partx_update(&disk.bus_path)
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
    let disk_information = SfDisk::get_info(&disk.bus_path).context(format!(
        "Failed to retrieve information for disk '{}'",
        disk.id
    ))?;

    // Get disk UUID from osuuid
    match disk_information.id.as_uuid() {
        Some(disk_uuid) => {
            // Update the host status with disk UUID to disk ID mapping
            host_status.disks_by_uuid.insert(disk_uuid, disk.id.into());
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

        // Update host status with the partition metadata.
        trace!(
            "Updating host status with partition '{}':\n{:#?}",
            repart_partition.id,
            repart_partition
        );
        host_status
            .block_device_paths
            .insert(repart_partition.id.clone(), repart_partition.path_by_uuid());
    }
    Ok(())
}

/// Perform a runtime safety check.
///
/// This function will go through all requested disk changes to ensure that they
/// do not destroy partitions that are currently mounted.
fn partitioning_safety_check(disks: &Vec<ResolvedDisk>) -> Result<(), Error> {
    // Validation has already verified that any disk with adopted partitions will have
    // a GPT partition table, so we can safely assume that here.
    info!("Running partitioning safety check...");

    for disk in disks {
        debug!("Running partitioning safety check for disk '{}'", disk.id);

        let blkdev_info =
            lsblk::run(&disk.bus_path).context("Failed to retrieve partition table information")?;

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
        if blkdev_info.mountpoint.is_some() {
            bail!(
                "Disk '{}' is currently mounted at '{}', cannot proceed with partitioning.",
                disk.id,
                blkdev_info.mountpoint.unwrap().display()
            );
        }

        let disk_info = SfDisk::get_info(&disk.bus_path).context(format!(
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

                let part_info = osutils::lsblk::run(&part.node).with_context(|| {
                    format!(
                        "Failed to retrieve information for partition '{}' on disk '{}'.",
                        part.node.display(),
                        disk.id
                    )
                })?;

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

    info!("Partitioning safety check passed!");
    Ok(())
}

/// Adopt partitions on a disk.
///
/// This function will attempt to match the partitions on the disk with the
/// adopted partitions. If a partition is matched, it will be kept. If a
/// partition is not matched, it will be deleted. Matched partitions are saved
/// to host status.
fn adopt_partitions(disk: &ResolvedDisk, repart: &mut SystemdRepartInvoker) -> Result<(), Error> {
    if disk.spec.adopted_partitions.is_empty() {
        // Nothing to do :)
        return Ok(());
    }

    info!(
        "Trying to adopt {} partitions on disk '{}'",
        disk.spec.adopted_partitions.len(),
        disk.id
    );

    let disk_info = SfDisk::get_info(&disk.bus_path).context(format!(
        "Failed to retrieve information for disk '{}', the partition table could be missing or corrupted.",
        disk.id
    ))?;

    // We switch to refuse mode, meaning repart will require a partition
    // table to be present.
    repart.set_empty_mode(RepartEmptyMode::Refuse);

    ensure!(
        !disk_info.partitions.is_empty(),
        "Disk '{}' has adopted partitions configured but currently contains no partitions",
        disk.id
    );

    trace!("Disk '{}' before adoption:\n{:#?}", disk.id, disk_info);

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

    // Delete all partitions that were not matched.
    adopter
        .get_unmatched_partitions()
        .try_for_each(|part| {
            debug!(
                "Deleting unmatched partition '{}' on disk '{}'",
                part.node.display(),
                disk.id
            );
            part.delete().with_context(|| {
                format!(
                    "Failed to delete unmatched partition '{}' on disk '{}'",
                    part.node.display(),
                    disk.id
                )
            })
        })
        .context(format!(
            "Failed to delete unmatched partitions on disk '{}'",
            disk.id
        ))?;

    // Get the matched partitions to make necessary updates.
    adopter
        .get_matched_partitions()
        .for_each(|(part, adopted)| {
            trace!("Keeping adopted partition '{}':\n{:#?}", adopted.id, part);

            // We need to inform repart about the adopted partitions.
            repart.push_partition_entry(RepartPartitionEntry {
                // Store the BlockDeviceId in the id field.
                id: adopted.id.clone(),

                // Inform repart about the partition type to it can match it.
                partition_type: part.partition_type,

                // Keep the same label as the original partition.
                label: part.name.clone(),

                // Inform repart about the size of the partition to avoid resizes.
                size_max_bytes: Some(part.size),
                size_min_bytes: Some(part.size),
            });
        });

    trace!(
        "Disk '{}' after adoption:\n{:#?}",
        disk.id,
        SfDisk::get_info(&disk_info.device).context(format!(
            "Failed to retrieve information for disk '{}' after partition adoption.",
            disk.id
        ))?
    );

    Ok(())
}

/// Add repart entries for partitions that are to be created.
fn add_repart_entries(
    disk: &Disk,
    label_overrides: &HashMap<BlockDeviceId, String>,
    repart: &mut SystemdRepartInvoker,
) {
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

            // Use the label override if present, otherwise use the partition id.
            label: Some(
                label_overrides
                    .get(&partition.id)
                    .unwrap_or(&partition.id)
                    .clone(),
            ),

            // Inform repart about the size of the partition.
            size_max_bytes: size,
            size_min_bytes: size,
        })
    }
}

/// Generate a hash map of {key: partition_id, value: partlabel}, for all
/// members of AB Volumes so that sdrepart.rs can give initial "old-version"
/// labels, i.e. "_empty", to partitions that are inside any volume-pairs. This
/// is so that when sysupdate is invoked, it interprets PARTLABEL of the
/// partition to be updated as "old" enough.
fn generate_sysupdate_partlabels(storage: &Storage) -> HashMap<BlockDeviceId, String> {
    // Initialize an empty hash map, where key is BlockDeviceId,
    // value is the label of the partition.
    let mut partlabels: HashMap<BlockDeviceId, String> = HashMap::new();

    // TODO: Potentially, provide support for custom user-provided
    // PARTLABELs, if required by the users. Related ADO task:
    // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6125.

    // Iterate through host_status.storage.ab_update.volume_pairs. For each
    // volume_pair, add each partition_id to the hash map, where value for
    // volume-a-id (active) is "a" and value for volume-b-id (inactive) is
    // "_empty". On next run of sysupdate, "_empty" will be updated.
    if cfg!(feature = "sysupdate") {
        if let Some(ab_update) = &storage.ab_update {
            for volume_pair in &ab_update.volume_pairs {
                // For volume-a-id
                partlabels.insert(volume_pair.volume_a_id.clone(), "_empty".to_string());
                // For volume-b-id
                partlabels.insert(volume_pair.volume_b_id.clone(), "_empty".to_string());
            }
        }
    }

    partlabels
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
        PartitionType::Var => DiscoverablePartitionType::Var,
        PartitionType::Xbootldr => DiscoverablePartitionType::Xbootldr,
    }
}

/// Wait for a partition's path by partuuid to appear.
fn wait_for_part_symlink(repart_partition: &RepartPartition) -> Result<PathBuf, Error> {
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

struct PartitionAdopter<'a> {
    /// BTreeMap of all candidate partitions in the disk in logical order.
    /// (partition number, partition ref)
    candidates: BTreeMap<usize, SfPartition>,

    /// Map of matched partitions. (partition number,  adopted partition ref)
    matched: BTreeMap<usize, &'a AdoptedPartition>,
}

impl<'a> PartitionAdopter<'a> {
    /// Create a new PartitionAdopter from a disk info.
    fn new(disk_info: &SfDisk) -> Self {
        Self {
            candidates: disk_info
                .partitions
                .iter()
                .map(|p| (p.number, p.clone()))
                .collect(),
            matched: BTreeMap::new(),
        }
    }

    /// Get iterator of available (unmatched) candidate partitions in logical order.
    fn available_candidates_by_logical(&self) -> impl DoubleEndedIterator<Item = &SfPartition> {
        self.candidates
            .values()
            .filter(|cand| !self.has_match(cand))
    }

    /// Insert a match into the adopter.
    fn add_match(&mut self, number: usize, adopted_part: &'a AdoptedPartition) {
        self.matched.insert(number, adopted_part);
    }

    /// Check if a partition has been matched.
    fn has_match(&self, part: &'a SfPartition) -> bool {
        self.matched.contains_key(&part.number)
    }

    /// Adopt a partition based on the criteria.
    fn adopt(&mut self, adopted_part: &'a AdoptedPartition) -> Result<(), Error> {
        debug!("Attempting to adopt partition '{}'", adopted_part.id);
        let matched_candidate = match (&adopted_part.match_label, &adopted_part.match_uuid) {
            // Match by label
            (Some(label), None) => {
                // Find all partitions with the given label.
                let matching = self
                    .available_candidates_by_logical()
                    .filter(|cand| cand.name.as_deref() == Some(label))
                    .collect::<Vec<_>>();

                ensure!(
                    matching.len() == 1,
                    "Expected exactly one partition with label '{}', found {}",
                    label,
                    matching.len()
                );

                // Return the first matching partition.
                Some(matching[0])
            }

            // Match by UUID
            (None, Some(uuid)) => self
                .available_candidates_by_logical()
                .find(|cand| cand.id.match_uuid(uuid)),

            // Invalid match criteria
            _ => bail!(
                "Adopted partition '{}' must match with either a label xor a UUID",
                adopted_part.id
            ),
        };

        match matched_candidate {
            Some(candidate) => {
                debug!(
                    "Matched '{}' with candidate '{:#?}'",
                    adopted_part.id, candidate,
                );

                // This should generally not happen as only available partitions
                // are checked, but we want to ensure that we don't accidentally
                // adopt the same partition twice.
                ensure!(
                    !self.has_match(candidate),
                    "Partition {} was matched by adopted partition '{}' but it had already been adopted",
                    candidate.node.display(),
                    adopted_part.id
                );

                self.add_match(candidate.number, adopted_part);

                Ok(())
            }
            None => {
                bail!(
                    "No partition matched the criteria for adopted partition '{}'",
                    adopted_part.id
                );
            }
        }
    }

    /// Get iterator of partitions that were not matched.
    ///
    /// The partitions are in logical order.
    fn get_unmatched_partitions(&self) -> impl Iterator<Item = &SfPartition> {
        self.candidates
            .values()
            .filter(|cand| !self.has_match(cand))
    }

    /// Get iterator of partitions that were matched.
    ///
    /// The partitions are in logical order.
    fn get_matched_partitions(&self) -> impl Iterator<Item = (&SfPartition, &AdoptedPartition)> {
        // Because BTreeMap is ordered, we can iterate over the matched partitions in order.
        self.matched
            .iter()
            .map(|(number, adopted)| (&self.candidates[number], *adopted))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use uuid::Uuid;

    use osutils::sfdisk::{SfDiskLabel, SfDiskUnit};
    use trident_api::config::{AbUpdate, AbVolumePair, Partition, PartitionTableType};

    #[test]
    fn test_add_repart_entries() {
        let mut repart = SystemdRepartInvoker::new("/dev/sda", RepartEmptyMode::Force);

        let disk = Disk {
            id: "disk".to_string(),
            device: PathBuf::from("/dev/sda"),
            partitions: vec![
                Partition {
                    id: "part1".to_string(),
                    partition_type: PartitionType::Root,
                    size: 1024.into(),
                },
                Partition {
                    id: "part2".to_string(),
                    partition_type: PartitionType::Swap,
                    size: 2048.into(),
                },
                Partition {
                    id: "part3".to_string(),
                    partition_type: PartitionType::LinuxGeneric,
                    size: PartitionSize::Grow,
                },
            ],
            adopted_partitions: vec![],
            partition_table_type: PartitionTableType::Gpt,
        };

        let partlabels = maplit::hashmap! {
            "part2".to_string() => "part2_label".to_string(),
        };

        add_repart_entries(&disk, &partlabels, &mut repart);

        let entries = repart.partition_entries();
        assert_eq!(entries.len(), 3);

        let part1 = entries.first().unwrap();
        assert_eq!(part1.id, "part1");
        assert_eq!(part1.partition_type, DiscoverablePartitionType::Root);
        assert_eq!(part1.label, Some("part1".to_string()));
        assert_eq!(part1.size_max_bytes, Some(1024));
        assert_eq!(part1.size_min_bytes, Some(1024));

        let part2 = entries.get(1).unwrap();
        assert_eq!(part2.id, "part2");
        assert_eq!(part2.partition_type, DiscoverablePartitionType::Swap);
        assert_eq!(part2.label, Some("part2_label".to_string()));
        assert_eq!(part2.size_max_bytes, Some(2048));
        assert_eq!(part2.size_min_bytes, Some(2048));

        let part3 = entries.get(2).unwrap();
        assert_eq!(part3.id, "part3");
        assert_eq!(
            part3.partition_type,
            DiscoverablePartitionType::LinuxGeneric
        );
        assert_eq!(part3.label, Some("part3".to_string()));
        assert_eq!(part3.size_max_bytes, None);
        assert_eq!(part3.size_min_bytes, None);
    }

    #[test]
    fn test_generate_sysupdate_partlabels() {
        let storage = Storage {
            disks: vec![],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![AbVolumePair {
                    volume_a_id: "volume_a".to_string(),
                    volume_b_id: "volume_b".to_string(),
                    id: "pair".to_string(),
                }],
            }),
            ..Default::default()
        };

        let partlabels = generate_sysupdate_partlabels(&storage);

        if cfg!(feature = "sysupdate") {
            assert!(partlabels.len() == 2);
            assert_eq!(partlabels.get("volume_a").unwrap(), "_empty");
            assert_eq!(partlabels.get("volume_b").unwrap(), "_empty");
        } else {
            assert!(partlabels.is_empty());
        }
    }

    #[test]
    fn test_partition_adopter() {
        let disk_info = SfDisk {
            label: SfDiskLabel::Gpt,
            id: Uuid::parse_str("3E6494F9-91E1-426B-A25A-0A8101E464A4")
                .unwrap()
                .into(),
            device: PathBuf::from("/dev/sda"),
            unit: SfDiskUnit::Sectors,
            firstlba: 34,
            lastlba: 266338270,
            sectorsize: 512,
            capacity: 136_365_177_344,
            partitions: vec![
                SfPartition {
                    node: PathBuf::from("/dev/sda1"),
                    start: 2048,
                    size_sectors: 16_384,
                    size: 8_388_608,
                    partition_type: DiscoverablePartitionType::Esp,
                    id: Uuid::parse_str("F764E91F-9D15-4F6E-8508-0AFC1D0DF0B5")
                        .unwrap()
                        .into(),
                    name: Some("esp".to_string()),
                    parent: PathBuf::from("/dev/sda"),
                    number: 1,
                },
                SfPartition {
                    node: PathBuf::from("/dev/sda3"),
                    start: 20480,
                    size_sectors: 266_315_776,
                    size: 136_353_677_312,
                    partition_type: DiscoverablePartitionType::LinuxGeneric,
                    id: Uuid::parse_str("4D8C2A88-1411-4021-804D-EB8C40F054AA")
                        .unwrap()
                        .into(),
                    name: Some("rootfs".to_string()),
                    parent: PathBuf::from("/dev/sda"),
                    number: 3,
                },
            ],
        };

        let mut adopter = PartitionAdopter::new(&disk_info);

        // Try to adopt esp partition by label.
        let adopted_1 = AdoptedPartition {
            id: "esp".to_string(),
            match_label: Some("esp".to_string()),
            match_uuid: None,
        };
        adopter.adopt(&adopted_1).unwrap();

        // Check that we have a match.
        let matched = adopter.get_matched_partitions().next().unwrap();
        assert_eq!(matched.0, &disk_info.partitions[0]);
        assert_eq!(matched.1, &adopted_1);

        // There should be one unmatched partition, i.e. the rootfs partition.
        assert_eq!(
            adopter.get_unmatched_partitions().next().unwrap(),
            &disk_info.partitions[1]
        );

        // Try to adopt esp again, should fail.
        adopter.adopt(&adopted_1).unwrap_err();

        // Try to adopt rootfs partition by label AND UUID, should fail.
        let adopted_2 = AdoptedPartition {
            id: "rootfs".to_string(),
            match_label: Some("rootfs".to_string()),
            match_uuid: Some(Uuid::parse_str("4D8C2A88-1411-4021-804D-EB8C40F054AA").unwrap()),
        };
        adopter.adopt(&adopted_2).unwrap_err();

        // Try to adopt rootfs partition by UUID. Should succeed.
        let adopted_3 = AdoptedPartition {
            id: "rootfs".to_string(),
            match_label: None,
            match_uuid: Some(Uuid::parse_str("4D8C2A88-1411-4021-804D-EB8C40F054AA").unwrap()),
        };
        adopter.adopt(&adopted_3).unwrap();

        // Check that we have a match.
        let matched = adopter.get_matched_partitions().nth(1).unwrap();
        assert_eq!(matched.0, &disk_info.partitions[1]);
        assert_eq!(matched.1, &adopted_3);

        // There should be no unmatched partitions.
        // Using assert_eq! here so that in case of an error the remaining partition will get printed.
        assert_eq!(adopter.get_unmatched_partitions().next(), None);
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
    };
    use pytest_gen::functional_test;
    use trident_api::config::{Partition, PartitionTableType};

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

    #[functional_test]
    fn test_create_partitions() {
        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk".to_string(),
                    device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                    partitions: vec![
                        Partition {
                            id: "part1".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1M").unwrap(),
                        },
                        Partition {
                            id: "part2".to_string(),
                            partition_type: PartitionType::Swap,
                            size: PartitionSize::from_str("2M").unwrap(),
                        },
                        Partition {
                            id: "part3".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Grow,
                        },
                    ],
                    partition_table_type: PartitionTableType::Gpt,
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };

        create_partitions(&mut host_status, &host_config).unwrap();

        assert_eq!(host_status.block_device_paths.len(), 3);

        let check_part = |name: &str| {
            host_status
                .block_device_paths
                .get(name)
                .unwrap_or_else(|| panic!("Failed to find block device '{}' in status", name));
        };

        check_part("part1");
        check_part("part2");
        check_part("part3");

        osutils::wipefs::all(TEST_DISK_DEVICE_PATH).unwrap();
    }

    /// Create a test partition table on the test disk.
    /// The partition table will contain two partitions:
    /// - part1: 10 MiB, root partition, labeled "part1"
    /// - part2: 20 MiB, swap partition, labeled "part2"
    ///
    /// The partitions will be created with the force flag.
    fn create_test_partitions() {
        let repart = SystemdRepartInvoker::new(TEST_DISK_DEVICE_PATH, RepartEmptyMode::Force)
            .with_partition_entries(vec![
                RepartPartitionEntry {
                    id: "part1".to_string(),
                    partition_type: DiscoverablePartitionType::Root,
                    label: Some("part1".to_string()),
                    size_max_bytes: Some(10 * 1048576),
                    size_min_bytes: Some(10 * 1048576),
                },
                RepartPartitionEntry {
                    id: "part2".to_string(),
                    partition_type: DiscoverablePartitionType::Swap,
                    label: Some("part2".to_string()),
                    size_max_bytes: Some(20 * 1048576),
                    size_min_bytes: Some(20 * 1048576),
                },
            ]);

        let output = repart.execute().unwrap();
        println!("Created partitions:\n{:#?}", output);

        // Wait for the partitions to appear.
        for part in output.iter() {
            println!(
                "Waiting for partition symlink '{}': {}",
                part.id,
                part.path_by_uuid().display()
            );
            wait_for_part_symlink(part).unwrap();
        }
    }

    #[functional_test]
    fn test_adopt_partitions() {
        create_test_partitions();
        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk".to_string(),
                    device: PathBuf::from("/dev/sdb"),
                    partitions: vec![Partition {
                        id: "part3".to_string(),
                        partition_type: PartitionType::Root,
                        size: PartitionSize::from_str("1M").unwrap(),
                    }],
                    partition_table_type: PartitionTableType::Gpt,
                    adopted_partitions: vec![AdoptedPartition {
                        id: "part1".to_string(),
                        match_label: Some("part1".to_string()),
                        match_uuid: None,
                    }],
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };

        create_partitions(&mut host_status, &host_config).unwrap();

        assert_eq!(host_status.block_device_paths.len(), 2);
        assert!(
            host_status.block_device_paths.contains_key("part1"),
            "part1 not found"
        );
        assert!(
            !host_status.block_device_paths.contains_key("part2"),
            "part2 found"
        );
        assert!(
            host_status.block_device_paths.contains_key("part3"),
            "part3 not found"
        );

        osutils::wipefs::all(TEST_DISK_DEVICE_PATH).unwrap();
    }

    #[functional_test(negative = true)]
    fn test_adopt_bad_partitions() {
        create_test_partitions();

        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk".to_string(),
                    device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                    partitions: vec![Partition {
                        id: "part3".to_string(),
                        partition_type: PartitionType::Root,
                        size: PartitionSize::from_str("1M").unwrap(),
                    }],
                    partition_table_type: PartitionTableType::Gpt,
                    adopted_partitions: vec![AdoptedPartition {
                        id: "part4".to_string(),
                        match_label: Some("part4".to_string()),
                        match_uuid: None,
                    }],
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };

        create_partitions(&mut host_status, &host_config).unwrap_err();

        osutils::wipefs::all(TEST_DISK_DEVICE_PATH).unwrap();
    }
}
