use std::{collections::HashMap, path::PathBuf};

use anyhow::{bail, Context, Error, Ok};
use log::{debug, info, warn};
use uuid::Uuid;

use osutils::{
    block_devices::{self, ResolvedDisk},
    mdadm,
    osuuid::OsUuid,
    sfdisk,
};
use trident_api::{
    config::HostConfiguration,
    status::{HostStatus, ServicingType},
    BlockDeviceId,
};

use crate::engine::storage::partitioning;

/// Rebuilds the RAID array i.e adds the new disks partitions for the
/// given RAID array.
fn rebuild_raid_array(
    raid_id: &BlockDeviceId,
    disks_to_rebuild: &[BlockDeviceId],
    host_status: &HostStatus,
) -> Result<(), Error> {
    debug!(
        "Rebuilding RAID array '{}' with disks {:?}",
        raid_id, disks_to_rebuild
    );

    // Get the partitions to rebuild
    let partitions_to_rebuild: Vec<_> = disks_to_rebuild
        .iter()
        .flat_map(|disk_id| {
            host_status
                .spec
                .storage
                .disks
                .iter()
                .find(|d| d.id == *disk_id)
                .context(format!(
                    "Failed to find configuration for disk '{}' in host status spec",
                    disk_id
                ))
                .map(|disk| disk.partitions.iter().map(|p| p.id.clone()))
        })
        .flatten()
        .collect();

    // Get the RAID array and collect the devices that need to be rebuilt
    let raid_array = host_status
        .spec
        .storage
        .raid
        .software
        .iter()
        .find(|raid| raid.id == *raid_id)
        .context(format!(
            "Failed to find configuration for RAID array '{}' in host status spec",
            raid_id
        ))?;

    let rebuild_partitions: Vec<_> = raid_array
        .devices
        .iter()
        .filter(|device| partitions_to_rebuild.contains(device))
        .cloned()
        .collect();

    let rebuild_partition_paths: Result<Vec<PathBuf>, Error> = rebuild_partitions
        .iter()
        .map(|device| {
            host_status
                .block_device_paths
                .get(device)
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Failed to find block device path for RAID partition {:?}",
                        device
                    )
                })
        })
        .collect();
    let rebuild_partition_paths: Vec<PathBuf> = rebuild_partition_paths
        .context("Failed to get rebuild partition paths from host status spec")?;

    info!(
        "Rebuilding RAID array '{}' with partitions {:?}",
        raid_id, rebuild_partition_paths
    );

    // Get the RAID path
    let raid_path = host_status
        .spec
        .storage
        .raid
        .software
        .iter()
        .find(|&r| &r.id == raid_id)
        .context(format!(
            "Failed to find block device path for RAID array'{raid_id}'",
        ))?
        .device_path();

    // Add the new disk partitions in the RAID array
    for partition_path in rebuild_partition_paths {
        debug!(
            "Adding partition '{}' to RAID array '{}'",
            partition_path.display(),
            raid_path.display()
        );

        mdadm::add(raid_path.clone(), partition_path.clone()).context(format!(
            "Failed to add disk partition '{}' to rebuild RAID array '{}'",
            partition_path.display(),
            raid_id
        ))?;
    }
    Ok(())
}

/// Gets the disks that need to be rebuilt.
fn get_disks_to_rebuild(
    old_disk_uuid_id_map: &HashMap<Uuid, BlockDeviceId>,
    resolved_disks: &[ResolvedDisk],
) -> Result<Vec<BlockDeviceId>, Error> {
    let mut disks_to_rebuild = Vec::new();
    for disk in resolved_disks {
        match sfdisk::get_disk_uuid(&disk.dev_path).context(format!(
            "Failed to get UUID for disk '{}'",
            disk.dev_path.display()
        ))? {
            Some(OsUuid::Uuid(uuid)) => {
                if !old_disk_uuid_id_map.contains_key(&uuid) {
                    debug!(
                        "New disk {} with partition information added",
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
                    "New disk {} without partition information is added",
                    disk.dev_path.display()
                );
                disks_to_rebuild.push(disk.id.to_string());
            }
        }
    }
    Ok(disks_to_rebuild)
}

/// This function verifies if the rebuild-raid operation can be initiated by
/// validating the host configuration changes and determining whether the RAID
/// can be successfully recovered.
fn validate_rebuild_raid(
    host_config: &HostConfiguration,
    host_status: &mut HostStatus,
    disks_to_rebuild: &[BlockDeviceId],
) -> Result<(), Error> {
    validate_host_config_delta(host_config, &host_status.spec)
        .context("Failed to validate host configuration delta for rebuild-raid operation")?;

    if host_status.servicing_type == ServicingType::CleanInstall {
        bail!(
            "rebuild-raid command is not allowed when servicing type is {:?}",
            host_status.servicing_type
        );
    }

    validate_raid_recovery(host_config, disks_to_rebuild)
        .context("Failed to validate RAID recovery")?;

    // Fail validation if any of the disk partitions are not unformatted partitions or
    // part of a RAID array. Additionally, issue a warning if all partitions on
    // the disk to be rebuilt are unformatted partitions.
    for disk in disks_to_rebuild {
        let disk_info = host_config
            .storage
            .disks
            .iter()
            .find(|d| d.id.as_str() == disk)
            .context(format!(
                "Failed to find configuration for disk '{}' in host config",
                disk
            ))?;

        let partitions_len = disk_info.partitions.len();
        if partitions_len == 0 {
            continue;
        }
        let mut unformatted_partitions = 0;

        // Build the graph of storage devices.
        let graph = host_config
            .storage
            .build_graph()
            .context("Failed to build storage graph for host config")?;

        for partition in &disk_info.partitions {
            if !partition_is_raid_member(&partition.id, host_config) {
                if !host_config
                    .storage
                    .is_unformatted_partition(&graph.nodes, &partition.id)
                {
                    bail!(
                        "Partition '{}' is neither a member of a software RAID array nor an unformatted partition, refusing to rebuild",
                        partition.id
                    );
                } else {
                    unformatted_partitions += 1;
                }
            }
        }

        if unformatted_partitions == partitions_len {
            warn!(
                "All partitions in disk '{}' are unformatted partitions. The disk has no RAID arrays",
                disk
            );
        }
    }

    Ok(())
}

/// Validates the host configuration and rebuilds the RAID arrays.
pub(crate) fn validate_and_rebuild_raid(
    host_config: &HostConfiguration,
    host_status: &mut HostStatus,
) -> Result<(), Error> {
    let resolved_disks = block_devices::get_resolved_disks(host_config)
        .context("Failed to resolve disks to device paths")?;
    let disks_to_rebuild = get_disks_to_rebuild(&host_status.disks_by_uuid, &resolved_disks)
        .context("Failed to get disks to rebuild from host status")?;

    if disks_to_rebuild.is_empty() {
        info!("No disks to rebuild to perform RAID recovery");
        return Ok(());
    }

    validate_rebuild_raid(host_config, host_status, &disks_to_rebuild)
        .context("Trident rebuild-raid validation failed or could not be performed")?;

    debug!(
        "Rebuilding RAID arrays, Disks to rebuild {:?}",
        disks_to_rebuild
    );

    for disk in &disks_to_rebuild {
        // Get resolved disk for the disk to rebuild
        let resolved_disk = resolved_disks
            .iter()
            .find(|rd| rd.id == *disk)
            .context(format!("Failed to find resolved disk for disk '{}'", disk))?;

        // Create Partitions on the new disk
        partitioning::create_partitions_on_disk(
            host_config,
            resolved_disk,
            &mut host_status.block_device_paths,
            &mut host_status.disks_by_uuid,
        )
        .context(format!(
            "Failed to create partitions on disk '{}'",
            resolved_disk.id
        ))?;
    }

    let raid_disks_to_rebuild_map =
        get_raid_disks_to_rebuild_map(host_config, &disks_to_rebuild)
            .context("Failed to get the mapping of RAID arrays to disks to rebuild")?;

    // Rebuild RAID Arrays
    for (raid_array, disks) in raid_disks_to_rebuild_map {
        info!(
            "Rebuilding RAID array '{}' with disks {:?}",
            raid_array, disks
        );
        rebuild_raid_array(&raid_array, &disks, host_status)
            .context(format!("Failed to rebuild RAID array '{}'", raid_array))?;
    }
    Ok(())
}

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
            .context(format!(
                "Failed to find configuration for disk '{}' in host config",
                disk_id
            ))?;

        disks_to_rebuild_partitions.extend(disk.partitions.iter().map(|p| p.id.clone()));
    }

    if disks_to_rebuild_partitions.is_empty() {
        info!("No partitions to rebuild in disks {:?}", disks_to_rebuild);
        return Ok(());
    }

    for raid in host_config.storage.raid.software.iter() {
        let raid_devices = raid.devices.iter().collect::<Vec<_>>();

        if raid_devices
            .iter()
            .all(|p| disks_to_rebuild_partitions.contains(p))
        {
            bail!("Recovery is not possible as all the partitions in array '{}' are in the disks to rebuild", raid.id);
        }
    }

    Ok(())
}

/// Gets the RAID disks to rebuild map i.e. a map of RAID id and the associated
/// disks to rebuild.
fn get_raid_disks_to_rebuild_map(
    host_config: &HostConfiguration,
    disks_to_rebuild: &[BlockDeviceId],
) -> Result<HashMap<BlockDeviceId, Vec<BlockDeviceId>>, Error> {
    let mut raid_disks_to_rebuild_map: HashMap<BlockDeviceId, Vec<BlockDeviceId>> = HashMap::new();
    for raid in host_config.storage.raid.software.iter() {
        let raid_devices = raid.devices.iter().collect::<Vec<_>>();
        // Verify if any RAID device partitions are among the disk partitions to
        // rebuild, and create a map of RAID IDs to their corresponding disks to
        // rebuild.
        for disk in disks_to_rebuild {
            // Get partitions of the disk
            let partitions = host_config
                .storage
                .disks
                .iter()
                .find(|d| d.id == *disk)
                .context(format!(
                    "Failed to find configuration for disk '{}' in host config",
                    disk
                ))?
                .partitions
                .iter()
                .map(|p| p.id.clone())
                .collect::<Vec<_>>();
            // Check if any of the RAID devices is in the partitions of the disk
            if raid_devices.iter().any(|p| partitions.contains(p)) {
                raid_disks_to_rebuild_map
                    .entry(raid.id.clone())
                    .or_default()
                    .push(disk.clone());
            }
        }
    }
    Ok(raid_disks_to_rebuild_map)
}

/// Validates the difference between the host configuration used to trigger a
/// rebuild and the initial host configuration that is saved as host status spec.
/// Currently, it only checks if the host status specification and host
/// configuration are identical.
fn validate_host_config_delta(
    host_config: &HostConfiguration,
    host_status_spec: &HostConfiguration,
) -> Result<(), Error> {
    // Compare the host status spec and host_config.
    let mut host_status_spec = host_status_spec.clone();
    let mut host_config_to_compare = host_config.clone();

    // Skip checking the Trident field as Trident fields gets populated only on
    // host status spec.
    host_status_spec.trident = Default::default();
    host_config_to_compare.trident = Default::default();

    // Skip checking the old API for mount points and internal verity devices as
    // they haven't been populated in the host configuration for rebuild-raid.
    host_status_spec.storage.internal_mount_points = Default::default();
    host_config_to_compare.storage.internal_mount_points = Default::default();

    host_status_spec.storage.internal_verity = Default::default();
    host_config_to_compare.storage.internal_verity = Default::default();

    if host_status_spec != host_config_to_compare {
        bail!("We do not support the updated host configuration for the Trident rebuild-raid process. The configuration must match the original host configuration used during host provisioning.");
    }
    Ok(())
}

/// Checks if the partition is part of any RAID array.
fn partition_is_raid_member(partition_id: &BlockDeviceId, host_config: &HostConfiguration) -> bool {
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

    use std::{path::PathBuf, str::FromStr};

    use osutils::testutils::repart::TEST_DISK_DEVICE_PATH;
    use trident_api::{
        config::{
            Disk, FileSystemSource, FileSystemType, Image, ImageFormat, ImageSha256, MountOptions,
            MountPoint, Partition, PartitionSize, PartitionType, RaidLevel, Storage,
        },
        status::ServicingState,
    };

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
    fn test_partition_is_raid_member() {
        let host_config = get_host_config();
        let result = partition_is_raid_member(&String::from("disk1part1"), &host_config);
        assert!(result);
        let result = partition_is_raid_member(&String::from("disk2part1"), &host_config);
        assert!(result);
        let result = partition_is_raid_member(&String::from("disk1part2"), &host_config);
        assert!(!result);
    }

    #[test]
    fn test_validate_host_config_delta() {
        let mut host_config = get_host_config();
        let host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };

        validate_host_config_delta(&host_config, &host_status.spec).unwrap();

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

        assert_eq!(validate_host_config_delta(&host_config, &host_status.spec).unwrap_err().to_string(), "We do not support the updated host configuration for the Trident rebuild-raid process. The configuration must match the original host configuration used during host provisioning.");
    }

    #[test]
    fn test_get_raid_disks_to_rebuild_map() {
        let host_config = get_host_config();

        // RAID array is on the disk to rebuild.
        let disks_to_rebuild = vec!["disk2".to_string()];
        let result = get_raid_disks_to_rebuild_map(&host_config, &disks_to_rebuild).unwrap();
        let mut expected = HashMap::new();
        expected.insert("raid1".to_string(), vec!["disk2".to_string()]);
        assert_eq!(result, expected);

        // Append a new disk to the host configuration.
        let mut host_config = host_config;
        host_config.storage.disks.push(Disk {
            id: "disk3".to_string(),
            device: PathBuf::from("/dev/sdc"),
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

        // RAID array is not on the disk to rebuild.
        let disks_to_rebuild = vec!["disk3".to_string()];
        let result = get_raid_disks_to_rebuild_map(&host_config, &disks_to_rebuild).unwrap();
        let expected = HashMap::new();
        assert_eq!(result, expected);

        // Disks to rebuild is empty.
        let disks_to_rebuild = vec![];
        let result = get_raid_disks_to_rebuild_map(&host_config, &disks_to_rebuild).unwrap();
        let expected = HashMap::new();
        assert_eq!(result, expected);

        // Disk to rebuild is not part of the host configuration.
        let disks_to_rebuild = vec!["doesnotexist".to_string()];
        let result = get_raid_disks_to_rebuild_map(&host_config, &disks_to_rebuild);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Failed to find configuration for disk 'doesnotexist' in host config"
        );
    }

    #[test]
    fn test_validate_raid_recovery() {
        let host_config = get_host_config();

        // RAID array is recoverable.
        let disks_to_rebuild = vec!["disk2".to_string()];
        let result = validate_raid_recovery(&host_config, &disks_to_rebuild);
        result.unwrap();

        // Append a new raid array to the host configuration.
        let mut host_config = host_config;
        host_config
            .storage
            .raid
            .software
            .push(trident_api::config::SoftwareRaidArray {
                name: "raid2".to_string(),
                id: "raid2".to_string(),
                level: RaidLevel::Raid1,
                devices: vec!["disk1part2".to_string(), "disk1part1".to_string()],
            });

        // RAID array raid2 is not recoverable.
        let disks_to_rebuild = vec!["disk1".to_string()];
        let result = validate_raid_recovery(&host_config, &disks_to_rebuild);
        assert_eq!(result.unwrap_err().to_string(), "Recovery is not possible as all the partitions in array 'raid2' are in the disks to rebuild");

        // RAID arrays are not recoverable.
        let disks_to_rebuild = vec!["disk1".to_string(), "disk2".to_string()];
        let result = validate_raid_recovery(&host_config, &disks_to_rebuild);
        assert_eq!(result.unwrap_err().to_string(), "Recovery is not possible as all the partitions in array 'raid1' are in the disks to rebuild");

        // Disks to rebuild is empty.
        let disks_to_rebuild = vec![];
        let result = validate_raid_recovery(&host_config, &disks_to_rebuild);
        result.unwrap();

        // Disk to rebuild does not exist in the host configuration.
        let disks_to_rebuild = vec!["doesnotexist".to_string()];
        let result = validate_raid_recovery(&host_config, &disks_to_rebuild);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Failed to find configuration for disk 'doesnotexist' in host config"
        );
    }

    #[test]
    fn test_validate_rebuild_raid_success() {
        let host_config = get_host_config();
        let mut host_status = HostStatus {
            servicing_state: ServicingState::Provisioned,
            spec: host_config.clone(),
            ..Default::default()
        };

        let disks_to_rebuild = vec!["disk2".to_string()];
        let result = validate_rebuild_raid(&host_config, &mut host_status, &disks_to_rebuild);

        result.unwrap();
    }

    #[test]
    fn test_validate_rebuild_raid_failure_no_disks_to_rebuild() {
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
            ..Default::default()
        };

        let disks_to_rebuild = vec![];
        let result = validate_rebuild_raid(&host_config, &mut host_status, &disks_to_rebuild);

        result.unwrap();
    }

    #[test]
    fn test_validate_rebuild_raid_servicing_type_failure() {
        let host_config = get_host_config();

        let mut host_status = HostStatus {
            servicing_type: ServicingType::CleanInstall,
            spec: host_config.clone(),
            ..Default::default()
        };

        let disks_to_rebuild = vec!["disk2".to_string()];
        let result = validate_rebuild_raid(&host_config, &mut host_status, &disks_to_rebuild);

        assert_eq!(
            result.unwrap_err().to_string(),
            "rebuild-raid command is not allowed when servicing type is CleanInstall"
        );
    }

    #[test]
    fn test_validate_rebuild_raid_host_config_delta_failure() {
        let host_config = get_host_config();
        // Append a new disk and create a new host configuration.
        let mut host_config1 = host_config.clone();
        host_config1.storage.disks.push(Disk {
            id: "disk3".to_string(),
            device: PathBuf::from("/dev/sdc"),
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

        let mut host_status = HostStatus {
            spec: host_config,
            ..Default::default()
        };

        let disks_to_rebuild = vec!["disk2".to_string()];
        let result = validate_rebuild_raid(&host_config1, &mut host_status, &disks_to_rebuild);

        assert_eq!(
            result.unwrap_err().to_string(),
            "Failed to validate host configuration delta for rebuild-raid operation"
        );
    }

    #[test]
    fn test_validate_rebuild_raid_raid_recovery_failure() {
        let host_config = get_host_config();
        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };

        // RAID array is not recoverable.
        let disks_to_rebuild = vec!["disk1".to_string(), "disk2".to_string()];
        let result = validate_rebuild_raid(&host_config, &mut host_status, &disks_to_rebuild);

        assert_eq!(
            result.unwrap_err().to_string(),
            "Failed to validate RAID recovery"
        );
    }

    #[test]
    fn test_validate_rebuild_raid_storage_graph_failure() {
        let host_config = get_host_config();
        let mut host_config = host_config;
        host_config
            .storage
            .filesystems
            .push(trident_api::config::FileSystem {
                device_id: Some("disk2part1".to_string()),
                fs_type: FileSystemType::Ext4,
                source: FileSystemSource::Image(Image {
                    url: "http://example.com/root_1.img".to_string(),
                    sha256: ImageSha256::Checksum("root_sha256_1".to_string()),
                    format: ImageFormat::RawZst,
                }),
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/"),
                    options: MountOptions::empty(),
                }),
            });

        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };

        let disks_to_rebuild = vec!["disk2".to_string()];
        let result = validate_rebuild_raid(&host_config, &mut host_status, &disks_to_rebuild);

        // Failed to build storage graph for host config as disk2part1 has a filesystem and is also a RAID member.
        assert_eq!(
            result.unwrap_err().to_string(),
            "Failed to build storage graph for host config"
        );
    }

    #[test]
    fn test_validate_rebuild_raid_all_unformatted_partitions_warning() {
        let host_config = get_host_config();
        let mut host_config = host_config;
        // Clear raid in the host configuration.
        host_config.storage.raid.software.clear();

        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };

        let disks_to_rebuild = vec!["disk2".to_string()];
        let result = validate_rebuild_raid(&host_config, &mut host_status, &disks_to_rebuild);
        result.unwrap();

        let disks_to_rebuild = vec!["disk1".to_string(), "disk2".to_string()];
        let result = validate_rebuild_raid(&host_config, &mut host_status, &disks_to_rebuild);
        result.unwrap();
    }

    #[test]
    fn test_validate_rebuild_raid_invalid_hostconfig_for_rebuild_failure() {
        let host_config = get_host_config();
        let mut host_config = host_config;
        // Append esp partition to disk2.
        host_config.storage.disks[1].partitions.push(Partition {
            id: "disk2part3".to_string(),
            partition_type: PartitionType::Esp,
            size: PartitionSize::from_str("1M").unwrap(),
        });

        host_config
            .storage
            .filesystems
            .push(trident_api::config::FileSystem {
                device_id: Some("disk2part3".to_string()),
                fs_type: FileSystemType::Vfat,
                source: FileSystemSource::EspImage(Image {
                    url: "http://example.com/esp_2.img".to_string(),
                    sha256: ImageSha256::Checksum("esp_sha256_2".to_string()),
                    format: ImageFormat::RawZst,
                }),
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/esp"),
                    options: MountOptions::empty(),
                }),
            });

        let mut host_status = HostStatus {
            spec: host_config.clone(),
            ..Default::default()
        };

        // Disk1 can be rebuild.
        let disks_to_rebuild = vec!["disk1".to_string()];
        let result = validate_rebuild_raid(&host_config, &mut host_status, &disks_to_rebuild);

        result.unwrap();

        // Disk2 cannot be rebuild as it has a ESP partition.
        let disks_to_rebuild = vec!["disk2".to_string()];
        let result = validate_rebuild_raid(&host_config, &mut host_status, &disks_to_rebuild);

        assert_eq!(
            result.unwrap_err().to_string(),
            "Partition 'disk2part3' is neither a member of a software RAID array nor an unformatted partition, refusing to rebuild"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use crate::engine::{storage, EngineContext};

    use super::*;
    use pytest_gen::functional_test;

    use osutils::{
        testutils::{raid, repart::TEST_DISK_DEVICE_PATH},
        udevadm,
    };
    use std::{process::Command, str::FromStr};
    use trident_api::config::{
        AdoptedPartition, Disk, HostConfiguration, Partition, PartitionSize, PartitionTableType,
        PartitionType, RaidLevel, Storage,
    };
    use trident_api::status::ServicingState::Provisioned;

    /// Returns the host configuration and host status.
    fn get_hostconfig_and_hoststatus() -> (HostConfiguration, trident_api::status::HostStatus) {
        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk".to_string(),
                        device: PathBuf::from("/dev/sda"),
                        partitions: vec![Partition {
                            id: "raidpart1".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        }],
                        partition_table_type: PartitionTableType::Gpt,
                        adopted_partitions: vec![
                            AdoptedPartition {
                                id: "esp".to_string(),
                                match_label: Some("esp".to_string()),
                                match_uuid: None,
                            },
                            AdoptedPartition {
                                id: "root-a".to_string(),
                                match_label: Some("root-a".to_string()),
                                match_uuid: None,
                            },
                            AdoptedPartition {
                                id: "root-b".to_string(),
                                match_label: Some("root-b".to_string()),
                                match_uuid: None,
                            },
                            AdoptedPartition {
                                id: "swap".to_string(),
                                match_label: Some("swap".to_string()),
                                match_uuid: None,
                            },
                            AdoptedPartition {
                                id: "trident".to_string(),
                                match_label: Some("trident".to_string()),
                                match_uuid: None,
                            },
                        ],
                    },
                    Disk {
                        id: "disk2".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![Partition {
                            id: "raidpart2".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        }],
                        ..Default::default()
                    },
                ],
                raid: trident_api::config::Raid {
                    software: vec![trident_api::config::SoftwareRaidArray {
                        name: "raid1".to_string(),
                        id: "raid1".to_string(),
                        level: RaidLevel::Raid1,
                        devices: vec!["raidpart1".to_string(), "raidpart2".to_string()],
                    }],
                    ..Default::default()
                },

                ..Default::default()
            },
            ..Default::default()
        };
        let host_status = trident_api::status::HostStatus {
            spec: host_config.clone(),
            servicing_state: Provisioned,
            ..Default::default()
        };

        (host_config, host_status)
    }

    /// Deletes the partition /dev/sda6.
    fn delete_partition() {
        // Get sfdisk information for /dev/sda6.
        let sfdisk_info = sfdisk::SfDisk::get_info(PathBuf::from("/dev/sda")).unwrap();
        // Get partition information for /dev/sda6.
        let partition_info = sfdisk_info
            .partitions
            .iter()
            .find(|p| p.name == Some(String::from("raidpart1")))
            .unwrap();
        // Delete the partition.
        partition_info.delete().unwrap();
        // Run partx --delete on /dev/sda6 to delete the partition.
        Command::new("partx")
            .arg("--delete")
            .arg("/dev/sda6")
            .output()
            .unwrap();
        // Also run  partx --update on /dev/sda to update the partition table.
        block_devices::partx_update("/dev/sda").unwrap();
    }

    #[functional_test]
    fn test_validate_and_rebuild_raid_success() {
        let (host_config, mut host_status) = get_hostconfig_and_hoststatus();
        let mut ctx = EngineContext {
            spec: host_status.spec.clone(),
            ..Default::default()
        };

        // Stop any pre-existing RAID arrays.
        let err = storage::raid::stop_pre_existing_raid_arrays(&host_config);
        assert!(err.is_ok());

        // Create partitions on the test disks.
        storage::partitioning::create_partitions(&mut ctx).unwrap();
        host_status.block_device_paths = ctx.block_device_paths;
        host_status.disks_by_uuid = ctx.disks_by_uuid;
        udevadm::settle().unwrap();

        // Create a raid array raid1.
        let raid_path = PathBuf::from("/dev/md/raid1");
        let devices = [PathBuf::from("/dev/sda6"), PathBuf::from("/dev/sdb1")].to_vec();

        mdadm::create(&raid_path, &RaidLevel::Raid1, devices.clone()).unwrap();
        udevadm::wait(&raid_path).unwrap();
        raid::verify_raid_creation(&raid_path, devices.clone());

        // Add block device path of raid array to host status.
        host_status
            .block_device_paths
            .insert("raid1".to_string(), raid_path.clone());

        // Mark raid1 array as failed to simulate a disk failure.
        mdadm::fail(&raid_path, PathBuf::from("/dev/sdb1")).unwrap();

        // Wait for sdb to be freed.
        udevadm::settle().unwrap();

        // Now remove the disk2part1 from the RAID array.
        mdadm::remove(&raid_path, PathBuf::from("/dev/sdb1")).unwrap();

        // Disks to rebuild is empty as 2 disks UUIDs are already present in host status.
        validate_and_rebuild_raid(&host_config, &mut host_status).unwrap();

        // Verify that the RAID array hasnt been rebuilt as disks to rebuild is empty.
        let raid_devices = mdadm::detail(raid_path.as_ref()).unwrap();
        // Check if the RAID array has only 1 device.
        assert_eq!(raid_devices.devices.len(), 1);

        // Get disk UUID of disk2.
        let disk2_uuid = sfdisk::get_disk_uuid(&PathBuf::from("/dev/sdb"))
            .unwrap()
            .unwrap();

        // Remove disk2 UUID from host status.
        host_status
            .disks_by_uuid
            .remove(&disk2_uuid.as_uuid().unwrap());

        // Validate and rebuild RAID arrays.
        validate_and_rebuild_raid(&host_config, &mut host_status).unwrap();

        // Verify that the RAID array has been rebuilt successfully.
        raid::verify_raid_creation(raid_path.clone(), devices);

        // Cleanup the raid array.
        raid::stop_if_exists(&raid_path);

        osutils::wipefs::all("/dev/sda6").unwrap();

        // Delete the partition.
        delete_partition();
    }

    #[functional_test]
    fn test_validate_and_rebuild_raid_validation_failure() {
        let (host_config, mut host_status) = get_hostconfig_and_hoststatus();

        if let Some(disk2_uuid) = sfdisk::get_disk_uuid(&PathBuf::from("/dev/sdb")).unwrap() {
            // Remove disk2 UUID from host status.
            host_status
                .disks_by_uuid
                .remove(&disk2_uuid.as_uuid().unwrap());
        }

        // Fail validation.
        host_status.servicing_type = trident_api::status::ServicingType::CleanInstall;

        let err = validate_and_rebuild_raid(&host_config, &mut host_status);

        assert_eq!(
            err.unwrap_err().to_string(),
            "Trident rebuild-raid validation failed or could not be performed"
        );
    }

    #[functional_test]
    fn test_validate_and_rebuild_raid_raidrecovery_failure() {
        let (host_config, mut host_status) = get_hostconfig_and_hoststatus();

        if let Some(disk2_uuid) = sfdisk::get_disk_uuid(&PathBuf::from("/dev/sdb")).unwrap() {
            // Remove disk2 UUID from host status.
            host_status
                .disks_by_uuid
                .remove(&disk2_uuid.as_uuid().unwrap());
        }

        // Add a raid array raid2 which has partitions on disk2 to the host configuration.
        let mut host_config = host_config;
        host_config.storage.disks[1].partitions.push(Partition {
            id: "disk2part2".to_string(),
            partition_type: PartitionType::Root,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        host_config.storage.disks[1].partitions.push(Partition {
            id: "disk2part3".to_string(),
            partition_type: PartitionType::Root,
            size: PartitionSize::from_str("1G").unwrap(),
        });
        host_config
            .storage
            .raid
            .software
            .push(trident_api::config::SoftwareRaidArray {
                name: "raid2".to_string(),
                id: "raid2".to_string(),
                level: RaidLevel::Raid1,
                devices: vec!["disk2part2".to_string(), "disk2part3".to_string()],
            });

        host_status.spec = host_config.clone();

        let err = validate_and_rebuild_raid(&host_config, &mut host_status);

        assert_eq!(
            err.unwrap_err().to_string(),
            "Trident rebuild-raid validation failed or could not be performed"
        );
    }
}
