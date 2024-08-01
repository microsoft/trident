use std::collections::HashMap;

use anyhow::{bail, Context, Error};

use log::{debug, info, warn};
use osutils::{
    block_devices::{get_resolved_disks, ResolvedDisk},
    osuuid::OsUuid,
    sfdisk::get_disk_uuid,
};

use trident_api::{config::HostConfiguration, status::HostStatus, BlockDeviceId};
use uuid::Uuid;

#[allow(unused)]
/// Gets the disks that need to be rebuilt.
fn get_disks_to_rebuild(
    old_disk_uuid_id_map: &HashMap<Uuid, BlockDeviceId>,
    resolved_disks: &[ResolvedDisk],
) -> Result<Vec<BlockDeviceId>, Error> {
    let mut disks_to_rebuild = Vec::new();
    for disk in resolved_disks {
        match get_disk_uuid(&disk.dev_path).context(format!(
            "Failed to get UUID for disk '{}'",
            disk.dev_path.display()
        ))? {
            Some(OsUuid::Uuid(uuid)) => {
                if !old_disk_uuid_id_map.contains_key(&uuid) {
                    debug!(
                        "New disk with partition information added: {}",
                        disk.dev_path.display()
                    );
                    disks_to_rebuild.push(disk.id.to_string());
                } else {
                    debug!("Disk {} with UUID {} is already present", disk.id, uuid);
                }
            }
            Some(OsUuid::Relaxed(uuid)) => {
                debug!("A disk {} with OsUuid::Relaxed {} is added", disk.id, uuid);
                disks_to_rebuild.push(disk.id.to_string());
            }
            None => {
                debug!(
                    "New disk without partition information is added: {}",
                    disk.dev_path.display()
                );
                disks_to_rebuild.push(disk.id.to_string());
            }
        }
    }
    Ok(disks_to_rebuild)
}

#[allow(unused)]
/// Checks if the host configuration is valid for a rebuild operation.
fn validate_rebuild(
    host_config: &HostConfiguration,
    host_status: &mut HostStatus,
) -> Result<(), Error> {
    validate_host_config_delta(host_config, host_status)
        .context("Failed to validate host config delta")?;

    let old_disk_uuid_id_map = &host_status.storage.disks_by_uuid;

    // Resolve the disk paths to ensure that all disks in the host configuration
    // exist.
    let resolved_disks = get_resolved_disks(host_config).context("Failed to resolve disk paths")?;

    let disks_to_rebuild = get_disks_to_rebuild(old_disk_uuid_id_map, &resolved_disks)
        .context("Failed to get disks that need to be rebuilt")?;

    if disks_to_rebuild.is_empty() {
        info!("No disks to rebuild");
        return Ok(());
    }

    validate_raid_recovery(host_config, &disks_to_rebuild)
        .context("Failed to validate raid recovery")?;

    // Fail validation if any of the disk partitions are not part of a raid
    // array or raw partition. Additionally, issue a warning if all partitions
    // on the disk to be rebuilt are raw partitions.
    for disk in disks_to_rebuild {
        let disk_info = host_config
            .storage
            .disks
            .iter()
            .find(|d| d.id == disk)
            .context(format!("Failed to find disk '{}'", disk))?;

        let partitions_len = disk_info.partitions.len();
        let mut raw_partitions = 0;

        // Build the graph of storage devices.
        let graph = host_config
            .storage
            .build_graph()
            .context("Failed to build storage graph")?;

        for partition in &disk_info.partitions {
            if !partition_is_backed_by_raid(&partition.id, host_config) {
                if !host_config
                    .storage
                    .is_raw_partition(&graph.nodes, &partition.id)
                {
                    bail!(
                        "Partition '{}' is neither backed by a RAID array nor a raw partition",
                        partition.id
                    );
                } else {
                    raw_partitions += 1;
                }
            }
        }

        if raw_partitions == partitions_len {
            warn!(
                "All partitions in disk '{}' are raw partitions. The disk has no RAID arrays",
                disk
            );
        }
    }

    Ok(())
}

#[allow(unused)]
/// Checks if recovery is possible given the disks to rebuild. Ensures that each
/// RAID array has at least one partition that is not part of the disks marked
/// for rebuild.
fn validate_raid_recovery(
    host_config: &HostConfiguration,
    disks_to_rebuild: &[BlockDeviceId],
) -> Result<(), Error> {
    let mut disks_to_rebuild_partitions = vec![];

    // Collect all partitions from disks to rebuild
    for disk_id in disks_to_rebuild {
        let disk = host_config
            .storage
            .disks
            .iter()
            .find(|d| d.id == *disk_id)
            .context(format!("Failed to find disk '{}'", disk_id))?;

        disks_to_rebuild_partitions.extend(disk.partitions.iter().map(|p| p.id.clone()));
    }

    for raid in host_config.storage.raid.software.iter() {
        let raid_devices = raid.devices.iter().collect::<Vec<_>>();

        if raid_devices
            .iter()
            .all(|p| disks_to_rebuild_partitions.contains(p))
        {
            bail!("Recovery is not possible as all the partitions in  array '{}' are in the disks to rebuild", raid.id);
        }
    }

    Ok(())
}

#[allow(unused)]
/// Validates the host config delta between the host config and host status.
/// Currently only checks if the host status spec and host config are same.
fn validate_host_config_delta(
    host_config: &HostConfiguration,
    host_status: &HostStatus,
) -> Result<(), Error> {
    // Compare the host status spec and host_config.
    if host_status.spec != *host_config {
        bail!("The host status spec and host config are not same");
    }
    Ok(())
}

#[allow(unused)]
/// Checks if the partition is part of any raid array.
fn partition_is_backed_by_raid(
    partition_id: &BlockDeviceId,
    host_config: &HostConfiguration,
) -> bool {
    // Check if the partition ID is present in any RAID array in the host
    // configuration.
    host_config
        .storage
        .raid
        .software
        .iter()
        .any(|raid| raid.devices.contains(&partition_id.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use osutils::testutils::repart::TEST_DISK_DEVICE_PATH;
    use std::path::PathBuf;
    use std::str::FromStr;
    use trident_api::config::{Disk, Partition, PartitionSize, PartitionType, RaidLevel, Storage};

    fn get_host_config() -> HostConfiguration {
        HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk1".to_string(),
                        device: PathBuf::from("/dev/sda"),
                        partitions: vec![
                            Partition {
                                id: "disk1part1".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1M").unwrap(),
                            },
                            Partition {
                                id: "disk1part2".to_string(),
                                partition_type: PartitionType::Swap,
                                size: PartitionSize::from_str("2M").unwrap(),
                            },
                        ],
                        ..Default::default()
                    },
                    Disk {
                        id: "disk2".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "disk2part1".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1M").unwrap(),
                            },
                            Partition {
                                id: "disk2part2".to_string(),
                                partition_type: PartitionType::Swap,
                                size: PartitionSize::from_str("2M").unwrap(),
                            },
                        ],
                        ..Default::default()
                    },
                ],
                raid: trident_api::config::Raid {
                    software: vec![trident_api::config::SoftwareRaidArray {
                        name: "raid1".to_string(),
                        id: "raid1".to_string(),
                        level: RaidLevel::Raid1,
                        devices: vec!["disk1part1".to_string(), "disk2part1".to_string()],
                    }],
                    ..Default::default()
                },

                ..Default::default()
            },
            ..Default::default()
        }
    }
    #[test]
    fn test_is_partition_is_backed_by_raid() {
        let host_config = get_host_config();
        let result = partition_is_backed_by_raid(&String::from("disk1part1"), &host_config);
        assert!(result);
        let result = partition_is_backed_by_raid(&String::from("disk2part1"), &host_config);
        assert!(result);
        let result = partition_is_backed_by_raid(&String::from("disk1part2"), &host_config);
        assert!(!result);
    }

    #[test]
    fn test_validate_host_config_delta() {
        let mut host_config = get_host_config();
        let host_status = HostStatus {
            spec: host_config.clone(),
            storage: Default::default(),
            ..Default::default()
        };

        assert!(validate_host_config_delta(&host_config, &host_status).is_ok());

        host_config.storage.disks.push(Disk {
            id: "disk3".to_string(),
            device: PathBuf::from("/dev/sdb"),
            partitions: vec![
                Partition {
                    id: "disk3part1".to_string(),
                    partition_type: PartitionType::Root,
                    size: PartitionSize::from_str("1M").unwrap(),
                },
                Partition {
                    id: "disk3part2".to_string(),
                    partition_type: PartitionType::Swap,
                    size: PartitionSize::from_str("2M").unwrap(),
                },
            ],
            ..Default::default()
        });

        assert!(validate_host_config_delta(&host_config, &host_status).is_err());
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use pytest_gen::functional_test;

    use super::*;

    use osutils::testutils::repart::TEST_DISK_DEVICE_PATH;
    use std::path::Path;
    use std::path::PathBuf;
    use std::str::FromStr;
    use trident_api::config::Disk;
    use trident_api::config::Partition;
    use trident_api::config::PartitionSize;
    use trident_api::config::PartitionType;
    use trident_api::config::RaidLevel;
    use trident_api::config::Storage;

    fn get_host_config() -> HostConfiguration {
        HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk1".to_string(),
                        device: PathBuf::from("/dev/sda"),
                        partitions: vec![
                            Partition {
                                id: "disk1part1".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1M").unwrap(),
                            },
                            Partition {
                                id: "disk1part2".to_string(),
                                partition_type: PartitionType::Swap,
                                size: PartitionSize::from_str("2M").unwrap(),
                            },
                        ],
                        ..Default::default()
                    },
                    Disk {
                        id: "disk2".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "disk2part1".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1M").unwrap(),
                            },
                            Partition {
                                id: "disk2part2".to_string(),
                                partition_type: PartitionType::Swap,
                                size: PartitionSize::from_str("2M").unwrap(),
                            },
                        ],
                        ..Default::default()
                    },
                ],
                raid: trident_api::config::Raid {
                    software: vec![trident_api::config::SoftwareRaidArray {
                        name: "raid1".to_string(),
                        id: "raid1".to_string(),
                        level: RaidLevel::Raid1,
                        devices: vec!["disk1part1".to_string(), "disk2part1".to_string()],
                    }],
                    ..Default::default()
                },

                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[functional_test(feature = "helpers")]
    fn test_validate_rebuild_success() {
        let host_config = get_host_config();
        let mut host_status = HostStatus {
            spec: host_config.clone(),
            storage: Default::default(),
            ..Default::default()
        };

        // Get disk uuid of /dev/sda.
        let disk1_osuuid = get_disk_uuid(Path::new("/dev/sda")).unwrap().unwrap();
        let disk1_uuid = disk1_osuuid.as_uuid().unwrap();
        // Create a new (disk1_uuid, disk1) and set it to
        // host_status.storage.disk_uuid_id_map.
        let mut new_disk_uuid_id_map: HashMap<Uuid, BlockDeviceId> = HashMap::new();

        new_disk_uuid_id_map.insert(disk1_uuid, "disk1".to_string());
        host_status.storage.disks_by_uuid = new_disk_uuid_id_map.clone();

        let result = validate_rebuild(&host_config, &mut host_status);

        assert!(result.is_ok());
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_validate_rebuild_failure_no_disks_to_rebuild() {
        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk1".to_string(),
                    device: PathBuf::from("/dev/sda"),
                    partitions: vec![
                        Partition {
                            id: "disk1part1".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1M").unwrap(),
                        },
                        Partition {
                            id: "disk1part2".to_string(),
                            partition_type: PartitionType::Swap,
                            size: PartitionSize::from_str("2M").unwrap(),
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let mut host_status = HostStatus {
            spec: host_config.clone(),
            storage: Default::default(),
            ..Default::default()
        };

        let disk1_osuuid = get_disk_uuid(Path::new("/dev/sda")).unwrap().unwrap();
        let disk1_uuid = disk1_osuuid.as_uuid().unwrap();
        // Update the host_status.storage.disk_uuid_id_map with the same disk
        // uuid, disk id so that there are no disks to rebuild.
        let mut new_disk_uuid_id_map = HashMap::new();
        new_disk_uuid_id_map.insert(disk1_uuid, "disk1".to_string());
        host_status.storage.disks_by_uuid = new_disk_uuid_id_map.clone();

        let result = validate_rebuild(&host_config, &mut host_status);

        assert!(result.is_ok());
    }
}
