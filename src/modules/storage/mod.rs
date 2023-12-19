use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::info;
use osutils::udevadm;
use trident_api::{
    config::{HostConfiguration, MountPoint},
    status::{self, BlockDeviceContents, HostStatus, ReconcileState, UpdateKind},
    BlockDeviceId,
};

use crate::modules::Module;
use systemd_repart::RepartConfiguration;
use tabfile::TabFileSettings;
use tabfile::{TabFile, DEFAULT_FSTAB_PATH};

pub mod image;
mod raid;
mod sfdisk;
mod systemd_repart;
pub mod tabfile;

fn create_partitions(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    for disk in &host_config.storage.disks {
        let disk_path = disk.device.canonicalize().context(format!(
            "Failed to lookup device '{}'",
            disk.device.display()
        ))?;

        let disk_bus_path =
            find_symlink_for_target(&disk_path, Path::new("/dev/disk/by-path")).context(
                format!("Failed to find bus path of '{}'", disk_path.display()),
            )?;

        // Generate a hash map of {key: partition_id, value: partlabel},
        // so that sdrepart.rs can give initial "old-version" labels, i.e.
        // "_empty", to partitions that are inside any volume-pairs. This is so
        // that when sysupdate is invoked, it interprets PARTLABEL of the
        // partition to be updated as "old" enough.

        // Initialize an empty hash map, where key is BlockDeviceId,
        // value is String
        let mut partlabels: HashMap<BlockDeviceId, String> = HashMap::new();

        // TODO: Potentially, provide support for custom user-provided
        // PARTLABELs, if required by the users. Related ADO task:
        // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6125.

        // Iterate through host_status.storage.ab_update.volume_pairs. For each
        // volume_pair, add each partition_id to the hash map, where value for
        // volume-a-id (active) is "a" and value for volume-b-id (inactive) is
        // "_empty". On next run of sysupdate, "_empty" will be updated.
        if cfg!(feature = "sysupdate") {
            if let Some(ab_update) = &host_config.storage.ab_update {
                for volume_pair in &ab_update.volume_pairs {
                    // For volume-a-id
                    partlabels.insert(volume_pair.volume_a_id.clone(), "_empty".to_string());
                    // For volume-b-id
                    partlabels.insert(volume_pair.volume_b_id.clone(), "_empty".to_string());
                }
            }
        }

        // Pass hash map as second arg into new()
        let repart_config = RepartConfiguration::new(disk, &partlabels).context(format!(
            "Failed to generate systemd-repart config for disk {}",
            disk.id
        ))?;

        info!("Creating partitions for disk {}", disk.id);

        let partitions_status = repart_config
            .create_partitions(&disk_bus_path)
            .context(format!("Failed to initialize disk {}", disk.id))?;

        let disk_information = sfdisk::get_disk_information(&disk_bus_path)
            .context(format!("Failed to retrieve GPT UUID for disk {}", disk.id))?;

        host_status.storage.disks.insert(
            disk.id.clone(),
            status::Disk {
                uuid: disk_information.uuid,
                path: disk_bus_path.clone(),
                partitions: Vec::new(),
                capacity: disk_information.capacity,
                contents: BlockDeviceContents::Unknown,
            },
        );

        let disk_status = host_status
            .storage
            .disks
            .get_mut(&disk.id)
            .context(format!("Failed to find disk {} in host status", disk.id))?;

        // ensure all /dev/disk/* symlinks are created
        udevadm::settle()?;

        for (index, partition) in disk.partitions.iter().enumerate() {
            let partition_uuid = partitions_status[index].uuid;
            let part_path = Path::new("/dev/disk/by-partuuid").join(partition_uuid.to_string());
            if !part_path.exists() {
                bail!(
                    "Partition {} partuuid symlink {} does not exist",
                    partition.id,
                    part_path.display()
                );
            }

            let start = partitions_status[index].start;
            let size = partitions_status[index].size;
            disk_status.partitions.push(status::Partition {
                id: partition.id.clone(),
                path: part_path,
                start,
                end: start + size,
                ty: partition.partition_type,
                contents: BlockDeviceContents::Initialized,
                uuid: partition_uuid,
            });
        }

        disk_status.contents = status::BlockDeviceContents::Initialized;
    }
    Ok(())
}

#[derive(Default, Debug)]
pub(super) struct StorageModule;
impl Module for StorageModule {
    fn name(&self) -> &'static str {
        "storage"
    }

    fn refresh_host_status(&mut self, host_status: &mut HostStatus) -> Result<(), Error> {
        // Remove disks that no longer exist.
        host_status
            .storage
            .disks
            .retain(|_id, disk| disk.path.exists());

        image::refresh_host_status(host_status)
            .context("Image submodule failed during refresh_host_status")?;

        Ok(())
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        host_config: &HostConfiguration,
        _planned_update: ReconcileState,
    ) -> Result<(), Error> {
        // Ensure any two disks point to different devices. This requires canonicalizing the device
        // paths, which can only be done on the target system.
        let mut device_paths = HashMap::<PathBuf, BlockDeviceId>::new();
        for disk in &host_config.storage.disks {
            let device_path = disk
                .device
                .canonicalize()
                .context(format!("Failed to canonicalize path of disk {}", disk.id))?;
            if let Some(existing_disk_id) =
                device_paths.insert(device_path.clone(), disk.id.clone())
            {
                bail!(
                    "Disks {} and {} point to the same device {}",
                    disk.id,
                    existing_disk_id,
                    device_path.display()
                );
            }
        }

        Ok(())
    }

    fn select_update_kind(
        &self,
        host_status: &HostStatus,
        host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        if image::needs_ab_update(host_status, host_config) {
            return Some(UpdateKind::AbUpdate);
        }

        None
    }

    fn provision(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        mount_point: &Path,
    ) -> Result<(), Error> {
        if host_status.reconcile_state == ReconcileState::CleanInstall {
            raid::stop_pre_existing_raid_arrays(host_config)
                .context("Failed to clean up pre-existing RAID arrays")?;
            create_partitions(host_status, host_config)
                .context("Failed to create disk partitions")?;
            raid::create_sw_raid(host_status, host_config)
                .context("Failed to create software RAID")?;
        }

        image::provision(host_status, host_config, mount_point)
            .context("Image submodule failed during provision")?;

        Ok(())
    }

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        TabFile::from_mount_points(
            host_status,
            &host_config.storage.mount_points,
            &TabFileSettings {
                ..Default::default()
            },
        )
        .context("Failed to serialize mount point configuration for the target OS")?
        .write(Path::new(tabfile::DEFAULT_FSTAB_PATH))
        .context(format!("Failed to write {}", DEFAULT_FSTAB_PATH))?;

        host_status.storage.mount_points = host_config
            .storage
            .mount_points
            .iter()
            .map(|mount_point| {
                (
                    mount_point.target_id.clone(),
                    status::MountPoint {
                        path: mount_point.path.clone(),
                        filesystem: mount_point.filesystem.clone(),
                        options: mount_point.options.clone(),
                    },
                )
            })
            .collect();

        // TODO: update /etc/repart.d directly for the matching disk, derive
        // from where is the root located

        // persist on reboots
        raid::create_raid_config(host_status)
            .context("Failed to create mdadm.conf file after RAID creation")?;

        image::configure(host_status, host_config)
            .context("Image submodule failed during configure")?;

        Ok(())
    }
}

pub(super) fn path_to_mount_point<'a>(
    host_config: &'a HostConfiguration,
    path: &Path,
) -> Option<&'a MountPoint> {
    host_config
        .storage
        .mount_points
        .iter()
        .filter(|mp| path.starts_with(&mp.path))
        .max_by_key(|mp| mp.path.as_os_str().len())
}

/// Returns the path of the first symlink in directory whose canonical path is target.
/// Requires that target is already a canonical path.
fn find_symlink_for_target(target: &Path, directory: &Path) -> Result<PathBuf, Error> {
    fs::read_dir(directory)?
        .flatten()
        .filter_map(|f| {
            if let Ok(target_path) = f.path().canonicalize() {
                if target_path == target {
                    return Some(f.path());
                }
            }
            None
        })
        .min()
        .context(format!("Failed to find symlink for '{}'", target.display()))
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use trident_api::config::{
        Disk, HostConfiguration, Image, ImageFormat, ImageSha256, Partition, PartitionSize,
        PartitionType, RaidConfig, RaidLevel, SoftwareRaidArray, Storage,
    };

    use super::*;

    /// Validates Storage module HostConfiguration validation logic.
    #[test]
    fn test_validate_host_config() {
        let empty_host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            ..Default::default()
        };

        let mut host_config = HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk1".to_owned(),
                        device: "/".into(),
                        ..Default::default()
                    },
                    Disk {
                        id: "disk2".to_owned(),
                        device: "/tmp".into(),
                        partitions: vec![
                            Partition {
                                id: "part1".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("1M").unwrap(),
                            },
                            Partition {
                                id: "part2".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "part3".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "part4".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    },
                ],
                raid: RaidConfig {
                    software: vec![SoftwareRaidArray {
                        id: "my-raid1".to_owned(),
                        name: "my-raid".to_owned(),
                        level: RaidLevel::Raid1,
                        metadata_version: "1.2".to_owned(),
                        devices: vec!["part3".to_owned(), "part4".to_owned()],
                    }],
                },
                mount_points: vec![MountPoint {
                    filesystem: "ext4".to_owned(),
                    options: vec![],
                    target_id: "part1".to_owned(),
                    path: PathBuf::from("/"),
                }],
                images: vec![Image {
                    target_id: "part1".to_owned(),
                    url: "".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZstd,
                }],
                ab_update: Some(trident_api::config::AbUpdate {
                    volume_pairs: vec![trident_api::config::AbVolumePair {
                        id: "ab1".to_owned(),
                        volume_a_id: "part1".to_owned(),
                        volume_b_id: "part2".to_owned(),
                    }],
                }),
                encryption: None,
            },
            ..Default::default()
        };

        // fail on duplicate disk path
        host_config.storage.disks.get_mut(0).unwrap().device = "/tmp".into();

        assert!(StorageModule
            .validate_host_config(
                &empty_host_status,
                &host_config,
                ReconcileState::CleanInstall
            )
            .is_err());
    }
}
