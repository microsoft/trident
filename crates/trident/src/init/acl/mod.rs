use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, Context, Error};
use log::{trace, warn};
use uuid::Uuid;

use osutils::{
    container,
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
    constants::internal_params::MSHV_SLOT_ESP_PAYLOAD,
    error::TridentResultExt,
    status::{AbVolumeSelection, HostStatus, ServicingState},
    BlockDeviceId,
};

use crate::engine::boot::mshv;

const ACL_ESP_MOUNT_POINT: &str = "/boot";

/// Returns whether the host is running the ACL image.
pub fn is_acl() -> Result<bool, Error> {
    let os_release = OsRelease::read().context("Failed to read OS release information")?;

    Ok(os_release.variant_id == Some("azurecontainerlinux".to_string()))
}

/// An expected partition on the ACL root disk, identified by label, along
/// with its partition type, the actual `Partition` discovered on disk (if
/// any), and whether the partition is required to be present.
struct ExpectedPartition<'a> {
    label: &'a str,
    partition_type: PartitionType,
    partition: Option<Partition>,
    required: bool,
}

impl<'a> ExpectedPartition<'a> {
    fn new(label: &'a str, partition_type: PartitionType, required: bool) -> Self {
        Self {
            label,
            partition_type,
            partition: None,
            required,
        }
    }
}

/// Gathers information about the host's disk and partitions, framed by
/// the expectations of the ACL image layout, and returns a HostStatus
/// representing the current state of the host. This will be used to
/// initialize the Trident datastore if it has not been created yet.
/// Also discovers MSHV mode without modifying the ESP.
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

    let esp_path = container::get_host_relative_path(PathBuf::from(ACL_ESP_MOUNT_POINT))
        .unstructured("Failed to resolve ACL ESP path")?;
    inner_initial_host_status(&disk_information, &root_blk_device, &esp_path)
}

/// Internal function that does much of the work of creating the
/// initial ACL HostStatus, separated from the code that gets the
/// Host's current disk information. This allows for easier testing
/// of the HostStatus creation logic.
/// Complete MSHV bundles enable slot-owned mode; incomplete marked bundles fail.
fn inner_initial_host_status(
    disk_information: &SfDisk,
    root_blk_device: &BlockDevice,
    esp_path: &Path,
) -> Result<HostStatus, Error> {
    let disk_name = "disk-0".to_string();
    // Construct the HostStatus disk UUID struct
    let mut disk_uuids: HashMap<BlockDeviceId, Uuid> = HashMap::new();
    disk_uuids.insert(
        disk_name.clone(),
        root_blk_device
            .clone()
            .ptuuid
            .context("Root disk is missing ptuuid")?
            .as_uuid()
            .context("Root disk has invalid ptuuid")?,
    );

    let mut expected_partition_info: Vec<ExpectedPartition> = vec![
        ExpectedPartition::new("efi-system", PartitionType::Esp, true),
        ExpectedPartition::new("usr-a", PartitionType::Usr, true),
        ExpectedPartition::new("hash-a", PartitionType::UsrVerity, true),
        ExpectedPartition::new("hash-sig-a", PartitionType::UsrVeritySig, false),
        ExpectedPartition::new("usr-b", PartitionType::Usr, true),
        ExpectedPartition::new("hash-b", PartitionType::UsrVerity, true),
        ExpectedPartition::new("hash-sig-b", PartitionType::UsrVeritySig, false),
        ExpectedPartition::new("oem", PartitionType::LinuxGeneric, true),
        ExpectedPartition::new("root", PartitionType::Root, true),
    ];

    // Iterate through the current Host's partitions and create
    // Partition structs for the expected partitions. Also, check
    // to ensure that no label is used more than once.
    for p in disk_information.partitions.iter() {
        // Ensure that partition has both a name and UUID, as both are expected to
        // exist and are needed to create the HostStatus.
        let _partition_uuid = p.id.as_uuid().context("Partition is missing UUID")?;
        let label = p
            .name
            .clone()
            .context("Partition is missing name")?
            .to_lowercase();
        // Find corresponding partition entry in expected_partition_info based on label.
        //   * ignore case for comparison: expected labels are all lowercase, actual label has
        //     been lowercased above.
        let expected_partition = expected_partition_info
            .iter_mut()
            .find(|p| p.label == label)
            .context(format!(
                "Unexpected partition label '{}' found on root disk",
                label
            ))?;
        // Ensure there are not any duplicate partition labels.
        if expected_partition.partition.is_some() {
            return Err(anyhow!(
                "Multiple identical partition labels found on root disk: '{}'",
                label
            ));
        }
        trace!("Found partition '{}' on root disk", label);
        expected_partition.partition = Some(Partition {
            id: label.clone(),
            size: PartitionSize::from(p.size),
            partition_type: expected_partition.partition_type,
            label: p.name.clone(),
            uuid: p.id.as_uuid(),
        });
    }

    // Construct the HostStatus partition paths struct
    let partition_paths: BTreeMap<BlockDeviceId, PathBuf> = disk_information
        .partitions
        .iter()
        .map(|p| {
            // Note: Name and id have already been validated in the loop above, so it's safe to unwrap here.
            //       Also, using lower-case label.
            (
                p.name
                    .clone()
                    .unwrap_or("unknown".to_string())
                    .to_lowercase(),
                PathBuf::from(format!(
                    "/dev/disk/by-partuuid/{}",
                    p.id.as_uuid().unwrap_or(Uuid::nil())
                )),
            )
        })
        .collect();

    // Make sure that all the expected ACL partitions were found
    // and Partition structs were created for them. If any were not
    // created, return error.
    let missing_partitions: Vec<_> = expected_partition_info
        .iter()
        .filter(|p| p.partition.is_none() && p.required)
        .map(|p| p.label)
        .collect();
    if !missing_partitions.is_empty() {
        return Err(anyhow!(
            "Missing partition labels found on root disk: {:#?}",
            missing_partitions
        ));
    }

    // The usr-verity hash signature partitions are optional today (older
    // images may not have them), so only register an AB volume pair for
    // them when both the "a" and "b" partitions were actually found. A
    // partial find (exactly one of the two present) likely indicates a
    // corrupted or incomplete disk layout: the lone partition is still
    // surfaced in disks[0].partitions below, but silently gets no AB pair,
    // so warn loudly here to make that case visible rather than letting it
    // pass unnoticed.
    let hash_sig_count = expected_partition_info
        .iter()
        .filter(|p| (p.label == "hash-sig-a" || p.label == "hash-sig-b") && p.partition.is_some())
        .count();
    if hash_sig_count == 1 {
        warn!(
            "Only one of hash-sig-a/hash-sig-b was found on root disk; skipping AB volume pair \
             registration for usr-hash-sig. This may indicate a corrupted or incomplete disk layout."
        );
    }
    let hash_sig_pair_found = hash_sig_count == 2;

    let mut ab_volume_pairs = vec![
        AbVolumePair {
            id: "usr-data".to_string(),
            volume_a_id: "usr-a".to_string(),
            volume_b_id: "usr-b".to_string(),
        },
        AbVolumePair {
            id: "usr-hash".to_string(),
            volume_a_id: "hash-a".to_string(),
            volume_b_id: "hash-b".to_string(),
        },
    ];
    if hash_sig_pair_found {
        ab_volume_pairs.push(AbVolumePair {
            id: "usr-hash-sig".to_string(),
            volume_a_id: "hash-sig-a".to_string(),
            volume_b_id: "hash-sig-b".to_string(),
        });
    }

    let mut host_status = HostStatus {
        spec: HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: disk_name.clone(),
                    device: root_blk_device.clone().device_path(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: expected_partition_info
                        .iter()
                        .filter_map(|p| p.partition.clone())
                        .collect(),
                    ..Default::default()
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("efi-system".to_string()),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from(ACL_ESP_MOUNT_POINT),
                            options: MountOptions("umask=0077".to_string()),
                        }),
                        source: FileSystemSource::Image,
                        is_esp: true,
                    },
                    FileSystem {
                        device_id: Some("usr".to_string()),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/usr"),
                            options: MountOptions("defaults,ro".to_string()),
                        }),
                        source: FileSystemSource::Image,
                        is_esp: false,
                    },
                    FileSystem {
                        device_id: Some("oem".to_string()),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/oem"),
                            options: MountOptions("defaults".to_string()),
                        }),
                        source: FileSystemSource::Image,
                        is_esp: false,
                    },
                    FileSystem {
                        device_id: Some("root".to_string()),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/"),
                            options: MountOptions("defaults".to_string()),
                        }),
                        source: FileSystemSource::Image,
                        is_esp: false,
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
                    volume_pairs: ab_volume_pairs,
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
    };
    // Adoption bootstraps AZLA before persisting this discovered mode.
    if mshv::has_slot_mshv_bundle(esp_path)? {
        host_status
            .spec
            .internal_params
            .set_flag(MSHV_SLOT_ESP_PAYLOAD);
    }
    Ok(host_status)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    use osutils::{
        lsblk::{BlockDevice, BlockDeviceType, PartitionTableType},
        sfdisk::{SfDisk, SfDiskLabel, SfDiskUnit, SfPartition},
    };
    use sysdefs::{osuuid::OsUuid, partition_types::DiscoverablePartitionType};

    enum TestPartitions {
        Correct,
        MissingOne,
        ExtraOne,
        Duplicate,
        WithHashSigPair,
        WithHashSigPartial,
    }
    fn create_sfpart(
        label: String,
        path: &Path,
        partition_type: DiscoverablePartitionType,
        uuid: &str,
        number: usize,
    ) -> SfPartition {
        SfPartition {
            node: path.to_path_buf(),
            start: 0,
            size_sectors: 0,
            partition_type,
            id: OsUuid::from(uuid),
            name: Some(label),
            size: 4096,
            parent: path.to_path_buf(),
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
                "usr-a".to_string(),
                &PathBuf::from("/dev/sda3"),
                DiscoverablePartitionType::Unknown(
                    Uuid::from_str("5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6").unwrap(),
                ),
                "123e4567-e89b-12d3-a456-426614174002",
                2,
            ),
            create_sfpart(
                "hash-a".to_string(),
                &PathBuf::from("/dev/sda4"),
                DiscoverablePartitionType::UsrVerity,
                "123e4567-e89b-12d3-a456-426614174003",
                3,
            ),
            create_sfpart(
                "usr-b".to_string(),
                &PathBuf::from("/dev/sda5"),
                DiscoverablePartitionType::Unknown(
                    Uuid::from_str("5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6").unwrap(),
                ),
                "123e4567-e89b-12d3-a456-426614174004",
                4,
            ),
            create_sfpart(
                "hash-b".to_string(),
                &PathBuf::from("/dev/sda6"),
                DiscoverablePartitionType::UsrVerity,
                "123e4567-e89b-12d3-a456-426614174005",
                5,
            ),
            create_sfpart(
                "oem".to_string(),
                &PathBuf::from("/dev/sda8"),
                DiscoverablePartitionType::LinuxGeneric,
                "123e4567-e89b-12d3-a456-426614174006",
                6,
            ),
            create_sfpart(
                "root".to_string(),
                &PathBuf::from("/dev/sda11"),
                DiscoverablePartitionType::Root,
                "123e4567-e89b-12d3-a456-426614174007",
                7,
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
                    "123e4567-e89b-12d3-a456-426614174008",
                    8,
                ));
            }
            TestPartitions::Duplicate => {
                // Duplicate a partition label
                partitions.push(create_sfpart(
                    "usr-a".to_string(),
                    &PathBuf::from("/dev/sda12"),
                    DiscoverablePartitionType::Unknown(
                        Uuid::from_str("5dfbf5f4-2848-4bac-aa5e-0d9a20b745a6").unwrap(),
                    ),
                    "123e4567-e89b-12d3-a456-426614174009",
                    9,
                ));
            }
            TestPartitions::WithHashSigPair => {
                // Add both usr-verity hash signature partitions
                partitions.push(create_sfpart(
                    "hash-sig-a".to_string(),
                    &PathBuf::from("/dev/sda9"),
                    DiscoverablePartitionType::UsrVeritySig,
                    "123e4567-e89b-12d3-a456-426614174010",
                    8,
                ));
                partitions.push(create_sfpart(
                    "hash-sig-b".to_string(),
                    &PathBuf::from("/dev/sda10"),
                    DiscoverablePartitionType::UsrVeritySig,
                    "123e4567-e89b-12d3-a456-426614174011",
                    9,
                ));
            }
            TestPartitions::WithHashSigPartial => {
                // Add only one of the usr-verity hash signature partitions
                partitions.push(create_sfpart(
                    "hash-sig-a".to_string(),
                    &PathBuf::from("/dev/sda9"),
                    DiscoverablePartitionType::UsrVeritySig,
                    "123e4567-e89b-12d3-a456-426614174010",
                    8,
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

    /// Creates a complete BOOT bundle for discovery tests.
    fn create_mshv_bundle(esp_path: &Path) {
        for name in [
            "bootx64.efi",
            "grubx64.efi",
            "second.efi",
            "HvLoader.efi",
            "lxhvloader.dll",
            "Windows/System32/hvix64.exe",
            "Windows/System32/hvax64.exe",
            "Windows/System32/kdstub.dll",
        ] {
            let path = esp_path.join("EFI/BOOT").join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"payload").unwrap();
        }
    }

    /// Only complete BOOT bundles enable MSHV; discovery leaves AZLA untouched.
    #[test]
    fn initial_host_status_discovers_slot_mshv_from_image() {
        let esp = tempfile::TempDir::new().unwrap();
        let disk = create_sfdisk("EFI-SYSTEM", TestPartitions::Correct);
        let device = create_blk_device();
        let status = inner_initial_host_status(&disk, &device, esp.path()).unwrap();
        assert!(!status.spec.internal_params.get_flag(MSHV_SLOT_ESP_PAYLOAD));

        std::fs::create_dir_all(esp.path().join("EFI/BOOT")).unwrap();
        std::fs::write(esp.path().join("EFI/BOOT/bootx64.efi"), b"shim").unwrap();
        std::fs::write(
            esp.path().join("EFI/HvLoader.efi"),
            b"legacy shared payload",
        )
        .unwrap();
        let status = inner_initial_host_status(&disk, &device, esp.path()).unwrap();
        assert!(!status.spec.internal_params.get_flag(MSHV_SLOT_ESP_PAYLOAD));

        create_mshv_bundle(esp.path());
        let status = inner_initial_host_status(&disk, &device, esp.path()).unwrap();
        assert!(status.spec.internal_params.get_flag(MSHV_SLOT_ESP_PAYLOAD));
        assert!(!esp.path().join("EFI/AZLA").exists());
    }

    /// Incomplete marked bundles fail discovery without creating AZLA.
    #[test]
    fn initial_host_status_rejects_incomplete_slot_mshv_payload() {
        let esp = tempfile::TempDir::new().unwrap();
        create_mshv_bundle(esp.path());
        std::fs::remove_file(esp.path().join("EFI/BOOT/HvLoader.efi")).unwrap();
        let error = inner_initial_host_status(
            &create_sfdisk("EFI-SYSTEM", TestPartitions::Correct),
            &create_blk_device(),
            esp.path(),
        )
        .unwrap_err();
        assert!(error
            .to_string()
            .contains("Missing MSHV payload file: HvLoader.efi"));
        assert!(!esp.path().join("EFI/AZLA").exists());
    }

    fn create_blk_device() -> BlockDevice {
        BlockDevice {
            name: "/dev/sda".into(),
            ptuuid: Some("5f578d9b-bc43-4778-927b-d7e019586bc5".into()),
            size: 17179869184,
            blkdev_type: BlockDeviceType::Disk,
            partition_table_type: Some(PartitionTableType::Gpt),
            ..Default::default()
        }
    }

    /// Ordinary ACL discovery needs no MSHV payload.
    #[test]
    fn test_inner_initial_host_status_success() {
        let sfdisk = create_sfdisk("efi-system", TestPartitions::Correct);
        let blkdevice = create_blk_device();
        // Run
        let init_host_status = inner_initial_host_status(
            &sfdisk,
            &blkdevice,
            tempfile::TempDir::new().unwrap().path(),
        )
        .unwrap();

        init_host_status.spec.validate().unwrap();

        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[0].label,
            Some("efi-system".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[1].label,
            Some("usr-a".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[2].label,
            Some("hash-a".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[3].label,
            Some("usr-b".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[4].label,
            Some("hash-b".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[5].label,
            Some("oem".to_string())
        );
        assert_eq!(
            init_host_status.spec.storage.disks[0].partitions[6].label,
            Some("root".to_string())
        );
    }

    #[test]
    fn test_inner_initial_host_status_without_hash_sig_partitions() {
        // When the usr-verity hash signature partitions are absent (today's
        // default), only the usr-data and usr-hash AB volume pairs should be
        // registered.
        let sfdisk = create_sfdisk("efi-system", TestPartitions::Correct);
        let blkdevice = create_blk_device();

        let init_host_status = inner_initial_host_status(
            &sfdisk,
            &blkdevice,
            tempfile::TempDir::new().unwrap().path(),
        )
        .unwrap();
        init_host_status.spec.validate().unwrap();

        let volume_pairs = &init_host_status
            .spec
            .storage
            .ab_update
            .as_ref()
            .unwrap()
            .volume_pairs;
        assert_eq!(volume_pairs.len(), 2);
        assert!(volume_pairs.iter().all(|p| p.id != "usr-hash-sig"));
    }

    #[test]
    fn test_inner_initial_host_status_with_hash_sig_partitions() {
        // When both usr-verity hash signature partitions are present, an
        // additional AB volume pair should be registered for them.
        let sfdisk = create_sfdisk("efi-system", TestPartitions::WithHashSigPair);
        let blkdevice = create_blk_device();

        let init_host_status = inner_initial_host_status(
            &sfdisk,
            &blkdevice,
            tempfile::TempDir::new().unwrap().path(),
        )
        .unwrap();
        init_host_status.spec.validate().unwrap();

        let partitions = &init_host_status.spec.storage.disks[0].partitions;
        assert!(partitions
            .iter()
            .any(|p| p.label == Some("hash-sig-a".to_string())));
        assert!(partitions
            .iter()
            .any(|p| p.label == Some("hash-sig-b".to_string())));

        let volume_pairs = &init_host_status
            .spec
            .storage
            .ab_update
            .as_ref()
            .unwrap()
            .volume_pairs;
        assert_eq!(volume_pairs.len(), 3);
        let hash_sig_pair = volume_pairs
            .iter()
            .find(|p| p.id == "usr-hash-sig")
            .expect("expected 'usr-hash-sig' AB volume pair to be present");
        assert_eq!(hash_sig_pair.volume_a_id, "hash-sig-a");
        assert_eq!(hash_sig_pair.volume_b_id, "hash-sig-b");
    }

    #[test]
    fn test_inner_initial_host_status_with_partial_hash_sig_partitions() {
        // When only one of the usr-verity hash signature partitions is
        // present, it should still be optional (no error), but no AB volume
        // pair should be registered for it since it's not a complete pair.
        let sfdisk = create_sfdisk("efi-system", TestPartitions::WithHashSigPartial);
        let blkdevice = create_blk_device();

        let init_host_status = inner_initial_host_status(
            &sfdisk,
            &blkdevice,
            tempfile::TempDir::new().unwrap().path(),
        )
        .unwrap();
        init_host_status.spec.validate().unwrap();

        let volume_pairs = &init_host_status
            .spec
            .storage
            .ab_update
            .as_ref()
            .unwrap()
            .volume_pairs;
        assert_eq!(volume_pairs.len(), 2);
        assert!(volume_pairs.iter().all(|p| p.id != "usr-hash-sig"));
    }

    /// Unexpected ESP labels fail before payload discovery.
    #[test]
    fn test_inner_initial_host_status_unexpected_part_name() {
        let wrong_label = "wrong-label";
        let unexpected_label = create_sfdisk(wrong_label, TestPartitions::Correct);
        let blkdevice = create_blk_device();
        // Run
        assert!(inner_initial_host_status(
            &unexpected_label,
            &blkdevice,
            tempfile::TempDir::new().unwrap().path()
        )
        .unwrap_err()
        .root_cause()
        .to_string()
        .contains(&format!(
            "Unexpected partition label '{}' found on root disk",
            wrong_label
        )));
    }

    /// Missing required partitions must be rejected before ESP discovery.
    #[test]
    fn test_inner_initial_host_status_missing_part() {
        let sfdisk = create_sfdisk("efi-system", TestPartitions::MissingOne);
        let blkdevice = create_blk_device();
        // Run
        assert!(inner_initial_host_status(
            &sfdisk,
            &blkdevice,
            tempfile::TempDir::new().unwrap().path()
        )
        .unwrap_err()
        .root_cause()
        .to_string()
        .contains("Missing partition labels found on root disk"));
    }

    /// Unrecognized partitions must not be silently adopted.
    #[test]
    fn test_inner_initial_host_status_extra_part() {
        let sfdisk = create_sfdisk("efi-system", TestPartitions::ExtraOne);
        let blkdevice = create_blk_device();
        // Run
        assert!(inner_initial_host_status(
            &sfdisk,
            &blkdevice,
            tempfile::TempDir::new().unwrap().path()
        )
        .unwrap_err()
        .root_cause()
        .to_string()
        .contains("Unexpected partition label 'extra-one' found on root disk"));
    }

    /// Duplicate labels cannot produce an ambiguous HostStatus.
    #[test]
    fn test_inner_initial_host_status_duplicate_part() {
        let sfdisk = create_sfdisk("efi-system", TestPartitions::Duplicate);
        let blkdevice = create_blk_device();
        // Run
        assert!(inner_initial_host_status(
            &sfdisk,
            &blkdevice,
            tempfile::TempDir::new().unwrap().path()
        )
        .unwrap_err()
        .root_cause()
        .to_string()
        .contains("Multiple identical partition labels found on root disk: 'usr-a'"));
    }
}
