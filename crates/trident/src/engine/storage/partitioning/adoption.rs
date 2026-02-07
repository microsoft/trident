use std::collections::BTreeMap;

use anyhow::{bail, ensure, Context, Error};
use log::{debug, trace};

use osutils::{
    block_devices::ResolvedDisk,
    repart::{RepartEmptyMode, RepartPartitionEntry, SystemdRepartInvoker},
    sfdisk::{SfDisk, SfPartition},
};
use trident_api::config::AdoptedPartition;

pub(super) struct PartitionAdopter<'a> {
    /// BTreeMap of all candidate partitions in the disk in logical order.
    /// (partition number, partition ref)
    candidates: BTreeMap<usize, SfPartition>,

    /// Map of matched partitions. (partition number,  adopted partition ref)
    matched: BTreeMap<usize, &'a AdoptedPartition>,
}

impl<'a> PartitionAdopter<'a> {
    /// Create a new PartitionAdopter from a disk info.
    pub(super) fn new(disk_info: &SfDisk) -> Self {
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
    pub(super) fn adopt(&mut self, adopted_part: &'a AdoptedPartition) -> Result<(), Error> {
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
    pub(super) fn get_unmatched_partitions(&self) -> impl Iterator<Item = &SfPartition> {
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

/// Adopt partitions on a disk.
///
/// This function will attempt to match the partitions on the disk with the
/// adopted partitions. If a partition is matched, it will be kept. If a
/// partition is not matched, it will be deleted. Matched partitions are saved
/// to engine context.
pub(super) fn adopt_partitions(
    disk: &ResolvedDisk,
    repart: &mut SystemdRepartInvoker,
) -> Result<(), Error> {
    if disk.spec.adopted_partitions.is_empty() {
        // Nothing to do :)
        return Ok(());
    }

    debug!(
        "Trying to adopt {} partitions on disk '{}'",
        disk.spec.adopted_partitions.len(),
        disk.id
    );

    let disk_info = SfDisk::get_info(&disk.dev_path).context(format!(
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

                // This UUID is NOT used for matching and will be entirely
                // ignored by repart regardless of value because this partition
                // already exists, so no need to pass it in. For convenience of
                // any future reader, we *try* to pass in the existing UUID of
                // this partition.
                //
                // > The UUID to assign to the partition if none is assigned
                // > yet. Note that this setting is not used for matching. It is
                // > also not used when a UUID is already set for an existing
                // > partition. It is thus only used when a partition is newly
                // > created or when an existing one had a all-zero UUID set.
                //
                // From:
                // https://www.freedesktop.org/software/systemd/man/latest/repart.d.html#UUID=
                uuid: part.id.as_uuid(),

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

#[cfg(test)]
mod tests {
    use super::*;

    use std::{path::PathBuf, str::FromStr};

    use anyhow::{bail, ensure, Context, Error};
    use log::{debug, error, info, trace};
    use uuid::Uuid;

    use osutils::{
        block_devices::{self, ResolvedDisk},
        lsblk,
        repart::{
            RepartActivity, RepartEmptyMode, RepartPartition, RepartPartitionEntry,
            SystemdRepartInvoker,
        },
        sfdisk::{SfDisk, SfDiskLabel, SfDiskUnit, SfPartition},
        udevadm,
    };
    use sysdefs::partition_types::DiscoverablePartitionType;
    use trident_api::{
        config::{
            AdoptedPartition, Disk, Partition, PartitionSize, PartitionTableType, PartitionType,
        },
        BlockDeviceId,
    };

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

    use std::{path::PathBuf, str::FromStr};

    use anyhow::{bail, ensure, Context, Error};
    use log::{debug, error, info, trace};
    use uuid::Uuid;

    use osutils::{
        block_devices::{self, ResolvedDisk},
        lsblk,
        repart::{
            RepartActivity, RepartEmptyMode, RepartPartition, RepartPartitionEntry,
            SystemdRepartInvoker,
        },
        sfdisk::{SfDisk, SfDiskLabel, SfDiskUnit, SfPartition},
        testutils::repart::TEST_DISK_DEVICE_PATH,
        udevadm, wipefs,
    };
    use pytest_gen::functional_test;
    use sysdefs::partition_types::DiscoverablePartitionType;
    use trident_api::{
        config::{
            AdoptedPartition, Disk, HostConfiguration, Partition, PartitionSize,
            PartitionTableType, PartitionType, Storage,
        },
        BlockDeviceId,
    };

    use crate::engine::{
        storage::partitioning::{self, repart_mode},
        EngineContext,
    };

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
                    uuid: None,
                    size_max_bytes: Some(10 * 1048576),
                    size_min_bytes: Some(10 * 1048576),
                },
                RepartPartitionEntry {
                    id: "part2".to_string(),
                    partition_type: DiscoverablePartitionType::Swap,
                    label: Some("part2".to_string()),
                    uuid: None,
                    size_max_bytes: Some(20 * 1048576),
                    size_min_bytes: Some(20 * 1048576),
                },
            ]);

        let output = repart.execute().unwrap();
        println!("Created partitions:\n{output:#?}");

        // Wait for the partitions to appear.
        for part in output.iter() {
            println!(
                "Waiting for partition symlink '{}': {}",
                part.id,
                part.path_by_uuid().display()
            );
            repart_mode::wait_for_part_symlink(part).unwrap();
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
                        uuid: None,
                        label: None,
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

        let mut ctx = EngineContext {
            spec: host_config.clone(),
            ..Default::default()
        };

        partitioning::create_partitions(&mut ctx).unwrap();

        assert_eq!(ctx.partition_paths.len(), 2);
        assert!(ctx.partition_paths.contains_key("part1"), "part1 not found");
        assert!(!ctx.partition_paths.contains_key("part2"), "part2 found");
        assert!(ctx.partition_paths.contains_key("part3"), "part3 not found");

        wipefs::all(TEST_DISK_DEVICE_PATH).unwrap();
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
                        uuid: None,
                        label: None,
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

        let mut ctx = EngineContext {
            spec: host_config.clone(),
            ..Default::default()
        };

        partitioning::create_partitions(&mut ctx).unwrap_err();

        wipefs::all(TEST_DISK_DEVICE_PATH).unwrap();
    }
}
