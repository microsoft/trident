use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{anyhow, Context, Error};
use log::trace;
use uuid::Uuid;

use crate::{
    lsblk::{self, BlockDevice},
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

/// Represents the Special release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub enum SpecialRelease {
    #[default]
    Other,
    SpecialLegacy,
    Special1,
}

impl SpecialRelease {
    pub fn initial_host_status(&self) -> Result<Option<HostStatus>, Error> {
        if !matches!(self, SpecialRelease::Special1) {
            return Ok(None);
        }

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

        self.inner_initial_host_status(disk_information, &root_blk_device)
    }

    fn inner_initial_host_status(
        &self,
        disk_information: SfDisk,
        root_blk_device: &BlockDevice,
    ) -> Result<Option<HostStatus>, Error> {
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

        Ok(Some(HostStatus {
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
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
}
