use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{anyhow, Context, Error};
use log::trace;
use uuid::Uuid;

use osutils::{
    lsblk::{self, BlockDevice},
    osrelease::OsRelease,
    sfdisk::SfDisk,
};

use trident_api::{
    config::{
        AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
        MountOptions, MountPoint, Partition, PartitionSize, PartitionTableType, PartitionType,
        Storage, VerityDevice,
    },
    status::{AbVolumeSelection, HostStatus, ServicingState},
    BlockDeviceId,
};

pub fn is_cih() -> Result<bool, Error> {
    let os_release = OsRelease::read().context("Failed to read OS release information")?;
    Ok(os_release.id == Some("cih".to_string()))
}

/// Represents the Special release.
pub fn initial_host_status() -> Result<HostStatus, Error> {
    let blk_devices = lsblk::list().context("Failed to run lsblk")?;
    let root_blk_device = blk_devices
        .into_iter()
        .find(|d| {
            d.children
                .iter()
                .filter_map(|p| p.mountpoint.as_ref())
                .any(|m| m == Path::new("/"))
        })
        .context("Failed to find root disk with lsblk")?;

    let disk_information = SfDisk::get_info(root_blk_device.device_path()).context(format!(
        "Failed to get information for disk '{}'",
        root_blk_device.device_path().display()
    ))?;

    inner_initial_host_status(&disk_information, &root_blk_device)
}

fn inner_initial_host_status(
    disk_information: &SfDisk,
    root_blk_device: &BlockDevice,
) -> Result<HostStatus, Error> {
    let mut disk_uuids: HashMap<BlockDeviceId, Uuid> = HashMap::new();
    disk_uuids.insert(
        root_blk_device.clone().name,
        root_blk_device
            .clone()
            .ptuuid
            .context("Root disk is missing ptuuid")?
            .as_uuid()
            .context("Root disk has invalid ptuuid")?,
    );

    let partition_paths: BTreeMap<BlockDeviceId, PathBuf> = root_blk_device
        .children
        .iter()
        .filter_map(|p| {
            p.mountpoint
                .as_ref()
                .map(|m| (p.name.clone(), PathBuf::from(m)))
        })
        .collect();

    let bios_uuid = Uuid::from_str("21686148-6449-6e6f-7468-656564454649")
        .context("Failed to parse BIOS Boot Partition UUID")?;
    let usr_uuid = Uuid::from_str("5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6")
        .context("Failed to parse user-verity data Partition UUID")?;
    let special_reserved_uuid = Uuid::from_str("c95dc21a-df0e-4340-8d7b-26cbfa9a03e0")
        .context("Failed to parse Special reserved Partition UUID")?;
    let mut expected_partition_info: Vec<(&str, PartitionType, Option<Partition>)> = vec![
        ("efi-system", PartitionType::Esp, None),
        ("bios-boot", PartitionType::Unknown(bios_uuid), None),
        ("usr-data-a", PartitionType::Unknown(usr_uuid), None),
        ("usr-hash-a", PartitionType::UsrVerity, None),
        ("usr-data-b", PartitionType::Unknown(usr_uuid), None),
        ("usr-hash-b", PartitionType::UsrVerity, None),
        ("root-c", PartitionType::LinuxGeneric, None),
        ("oem", PartitionType::LinuxGeneric, None),
        (
            "oem-config",
            PartitionType::Unknown(special_reserved_uuid),
            None,
        ),
        (
            "flatcar-reserved",
            PartitionType::Unknown(special_reserved_uuid),
            None,
        ),
        ("root", PartitionType::Root, None),
    ];

    for p in disk_information.partitions.iter() {
        let label = p.name.clone().context("Partition is missing name")?;
        // for p in root_blk_device.children.iter() {
        let expected_partition = expected_partition_info
            .iter_mut()
            .find(|(expected_label, _, _)| *expected_label == label)
            .context(format!(
                "Unexpected partition label '{}' found on root disk",
                label
            ))?;

        if expected_partition.2.is_some() {
            return Err(anyhow!(
                "Multiple identical partition labels found on root disk: {:#?}",
                label
            ));
        }
        trace!("Found partition '{}' on root disk", label);
        expected_partition.2 = Some(Partition {
            id: p.id.to_string(),
            size: PartitionSize::from(p.size),
            partition_type: expected_partition.1,
            label: p.name.clone(),
            uuid: None,
        });
    }
    let missing_partitions: Vec<_> = expected_partition_info
        .iter()
        .filter(|k| k.2.is_none())
        .map(|(label, _, _)| *label)
        .collect();
    if !missing_partitions.is_empty() {
        return Err(anyhow!(
            "Missing partition labels found on root disk: {:#?}",
            missing_partitions
        ));
    }

    Ok(HostStatus {
        spec: HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk-0".to_string(),
                    device: root_blk_device.clone().device_path(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: expected_partition_info
                        .iter()
                        .filter_map(|(_, _, p)| p.clone())
                        .collect(),
                    ..Default::default()
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("efi-system".to_string()),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/boot"),
                            options: MountOptions("umask=0077".to_string()),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("usr-a".to_string()),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/usr"),
                            options: MountOptions("defaults,ro".to_string()),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("oem".to_string()),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/oem"),
                            options: MountOptions("defaults".to_string()),
                        }),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("root".to_string()),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/"),
                            options: MountOptions("defaults".to_string()),
                        }),
                        source: FileSystemSource::Image,
                    },
                ],
                verity: vec![VerityDevice {
                    id: "usr".to_string(),
                    name: "usr".to_string(),
                    data_device_id: "usr-data".to_string(),
                    hash_device_id: "usr-hash".to_string(),
                    ..Default::default()
                }],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![
                        AbVolumePair {
                            id: "usr-data".to_string(),
                            volume_a_id: "usr-data-a".to_string(),
                            volume_b_id: "usr-data-b".to_string(),
                        },
                        AbVolumePair {
                            id: "usr-hash".to_string(),
                            volume_a_id: "usr-hash-a".to_string(),
                            volume_b_id: "usr-hash-b".to_string(),
                        },
                    ],
                }),
                ..Default::default()
            },
            ..Default::default()
        },
        servicing_state: ServicingState::Provisioned,
        install_index: 0,
        ab_active_volume: Some(AbVolumeSelection::VolumeA),
        disk_uuids,
        partition_paths,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use osutils::{
        lsblk::{BlockDevice, BlockDeviceType, PartitionTableType},
        sfdisk::{SfDisk, SfDiskLabel, SfDiskUnit, SfPartition},
    };
    use sysdefs::{osuuid::OsUuid, partition_types::DiscoverablePartitionType};

    enum TestPartitions {
        Correct,
        MissingOne,
        ExtraOne,
    }
    fn create_sfpart(
        label: String,
        path: &PathBuf,
        ptype: DiscoverablePartitionType,
        uuid: &str,
        number: usize,
    ) -> SfPartition {
        SfPartition {
            node: path.clone(),
            start: 0,
            size_sectors: 0,
            partition_type: ptype,
            id: OsUuid::from(uuid),
            name: Some(label),
            size: 0,
            parent: path.clone(),
            number,
        }
    }
    fn create_sfdisk(first_part_name: &str, test_partitions: TestPartitions) -> SfDisk {
        let mut partitions = vec![
            create_sfpart(
                first_part_name.to_string(),
                &PathBuf::from("/dev/sda1"),
                DiscoverablePartitionType::Esp,
                "123e4567-e89b-12d3-a456-426614174001",
                1,
            ),
            create_sfpart(
                "bios-boot".to_string(),
                &PathBuf::from("/dev/sda2"),
                DiscoverablePartitionType::Unknown(
                    Uuid::from_str("21686148-6449-6e6f-7468-656564454649").unwrap(),
                ),
                "123e4567-e89b-12d3-a456-426614174002",
                2,
            ),
            create_sfpart(
                "usr-data-a".to_string(),
                &PathBuf::from("/dev/sda3"),
                DiscoverablePartitionType::Unknown(
                    Uuid::from_str("5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6").unwrap(),
                ),
                "123e4567-e89b-12d3-a456-426614174003",
                3,
            ),
            create_sfpart(
                "usr-hash-a".to_string(),
                &PathBuf::from("/dev/sda4"),
                DiscoverablePartitionType::UsrVerity,
                "123e4567-e89b-12d3-a456-426614174004",
                4,
            ),
            create_sfpart(
                "usr-data-b".to_string(),
                &PathBuf::from("/dev/sda5"),
                DiscoverablePartitionType::Unknown(
                    Uuid::from_str("5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6").unwrap(),
                ),
                "123e4567-e89b-12d3-a456-426614174005",
                5,
            ),
            create_sfpart(
                "usr-hash-b".to_string(),
                &PathBuf::from("/dev/sda6"),
                DiscoverablePartitionType::UsrVerity,
                "123e4567-e89b-12d3-a456-426614174006",
                6,
            ),
            create_sfpart(
                "root-c".to_string(),
                &PathBuf::from("/dev/sda7"),
                DiscoverablePartitionType::LinuxGeneric,
                "123e4567-e89b-12d3-a456-426614174007",
                7,
            ),
            create_sfpart(
                "oem".to_string(),
                &PathBuf::from("/dev/sda8"),
                DiscoverablePartitionType::LinuxGeneric,
                "123e4567-e89b-12d3-a456-426614174008",
                8,
            ),
            create_sfpart(
                "oem-config".to_string(),
                &PathBuf::from("/dev/sda9"),
                DiscoverablePartitionType::Unknown(
                    Uuid::from_str("c95dc21a-df0e-4340-8d7b-26cbfa9a03e0").unwrap(),
                ),
                "123e4567-e89b-12d3-a456-426614174009",
                9,
            ),
            create_sfpart(
                "flatcar-reserved".to_string(),
                &PathBuf::from("/dev/sda10"),
                DiscoverablePartitionType::Unknown(
                    Uuid::from_str("c95dc21a-df0e-4340-8d7b-26cbfa9a03e0").unwrap(),
                ),
                "123e4567-e89b-12d3-a456-42661417400A",
                10,
            ),
            create_sfpart(
                "root".to_string(),
                &PathBuf::from("/dev/sda11"),
                DiscoverablePartitionType::Root,
                "123e4567-e89b-12d3-a456-42661417400B",
                11,
            ),
        ];

        match test_partitions {
            TestPartitions::Correct => {}
            TestPartitions::MissingOne => {
                // Remove a partition
                partitions.remove(2);
            }
            TestPartitions::ExtraOne => {
                // Add extra partition
                partitions.push(create_sfpart(
                    "extra-one".to_string(),
                    &PathBuf::from("/dev/sda12"),
                    DiscoverablePartitionType::LinuxGeneric,
                    "123e4567-e89b-12d3-a456-42661417400C",
                    12,
                ));
            }
        };

        SfDisk {
            label: SfDiskLabel::Gpt,
            id: OsUuid::from("123e4567-e89b-12d3-a456-426614174000"),
            device: PathBuf::from("/dev/sda"),
            unit: SfDiskUnit::Sectors,
            firstlba: 0,
            lastlba: 0,
            sectorsize: 512,
            capacity: 0,
            partitions,
        }
    }

    fn create_blk_device() -> BlockDevice {
        BlockDevice {
            name: "/dev/sda".into(),
            fstype: None,
            fssize: None,
            fsuuid: None,
            ptuuid: Some("5f578d9b-bc43-4778-927b-d7e019586bc5".into()),
            part_uuid: None,
            partn: None,
            size: 17179869184,
            parent_kernel_name: None,
            readonly: false,
            blkdev_type: BlockDeviceType::Disk,
            mountpoint: None,
            mountpoints: vec![],
            partition_table_type: Some(PartitionTableType::Gpt),
            children: vec![],
        }
    }

    #[test]
    fn test_inner_initial_host_status_success() {
        let sfdisk = create_sfdisk("efi-system", TestPartitions::Correct);
        let blkdevice = create_blk_device();
        // Run
        let init_host_status = inner_initial_host_status(&sfdisk, &blkdevice).unwrap();
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[0].label,
            Some("efi-system".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[1].label,
            Some("bios-boot".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[2].label,
            Some("usr-data-a".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[3].label,
            Some("usr-hash-a".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[4].label,
            Some("usr-data-b".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[5].label,
            Some("usr-hash-b".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[6].label,
            Some("root-c".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[7].label,
            Some("oem".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[8].label,
            Some("oem-config".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[9].label,
            Some("flatcar-reserved".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[10].label,
            Some("root".to_string())
        );
    }

    #[test]
    fn test_inner_initial_host_status_unexpected_part_name() {
        let wrong_label = "wrong-label";
        let unexpected_label = create_sfdisk(wrong_label, TestPartitions::Correct);
        let blkdevice = create_blk_device();
        // Run
        assert!(inner_initial_host_status(&unexpected_label, &blkdevice)
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains(&format!(
                "Unexpected partition label '{}' found on root disk",
                wrong_label
            )));
    }

    #[test]
    fn test_inner_initial_host_status_missing_part() {
        let sfdisk = create_sfdisk("efi-system", TestPartitions::MissingOne);
        let blkdevice = create_blk_device();
        // Run
        assert!(inner_initial_host_status(&sfdisk, &blkdevice)
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains("Missing partition labels found on root disk"));
    }

    #[test]
    fn test_inner_initial_host_status_extra_part() {
        let sfdisk = create_sfdisk("efi-system", TestPartitions::ExtraOne);
        let blkdevice = create_blk_device();
        // Run
        assert!(inner_initial_host_status(&sfdisk, &blkdevice)
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains("Unexpected partition label 'extra-one' found on root disk"));
    }
}
