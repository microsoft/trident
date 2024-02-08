use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::{info, warn};

use osutils::{
    partition_types::DiscoverablePartitionType,
    repart::{RepartMode, RepartPartitionEntry, SystemdRepartInvoker},
    sfdisk::SfDisk,
    udevadm,
};
use trident_api::{
    config::{HostConfiguration, MountPoint, PartitionSize, PartitionType},
    status::{self, BlockDeviceContents, HostStatus, ReconcileState, UpdateKind},
    BlockDeviceId,
};

use crate::modules::Module;

mod encryption;
pub mod image;
mod raid;
pub mod tabfile;

use tabfile::TabFileSettings;
use tabfile::{TabFile, DEFAULT_FSTAB_PATH};

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

        let mut repart = SystemdRepartInvoker::new(disk_path, RepartMode::Force);

        for partition in &disk.partitions {
            let partlabel = partlabels.get(&partition.id).unwrap_or(&partition.id);
            let size = match partition.size {
                PartitionSize::Grow => None,
                PartitionSize::Fixed(s) => Some(s),
            };

            repart.push_partition_entry(RepartPartitionEntry {
                partition_type: config_part_type_into_discoverable(partition.partition_type),
                label: Some(partlabel.clone()),
                size_max_bytes: size,
                size_min_bytes: size,
            })
        }

        info!("Creating partitions for disk {}", disk.id);

        let created_partitions = repart
            .execute()
            .context(format!("Failed to create partitions for disk {}", disk.id))?;

        let disk_information = SfDisk::get_info(&disk_bus_path).context(format!(
            "Failed to retrieve information for disk {}",
            disk.id
        ))?;

        host_status.storage.disks.insert(
            disk.id.clone(),
            status::Disk {
                uuid: disk_information.id,
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

        for (index, partition) in disk.partitions.iter().enumerate() {
            let partition_uuid = created_partitions[index].uuid;
            let part_path = Path::new("/dev/disk/by-partuuid").join(partition_uuid.to_string());
            udevadm::wait(&part_path).context(format!(
                "Failed waiting for '{}' to appear",
                part_path.display()
            ))?;
            if !part_path.exists() {
                bail!(
                    "Partition {} partuuid symlink {} does not exist",
                    partition.id,
                    part_path.display()
                );
            }

            let start = created_partitions[index].start;
            let size = created_partitions[index].size;
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
                    "Disks '{}' and '{}' point to the same device '{}'",
                    disk.id,
                    existing_disk_id,
                    device_path.display()
                );
            }
        }

        encryption::validate_host_config(host_config)
            .context("Encryption host configuration validation failed")?;

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
        if mount_point.exists() {
            if let Err(e) = osutils::mount::umount(mount_point, true) {
                warn!(
                    "Attempt to unmount '{}' returned error: {e}",
                    mount_point.display(),
                );
            }
        }

        host_status.storage.mount_points = host_config
            .storage
            .mount_points
            .iter()
            .map(|mount_point| {
                (
                    mount_point.path.clone(),
                    status::MountPoint {
                        target_id: mount_point.target_id.clone(),
                        filesystem: mount_point.filesystem.clone(),
                        options: mount_point.options.clone(),
                    },
                )
            })
            .collect();

        if host_status.reconcile_state == ReconcileState::CleanInstall {
            raid::stop_pre_existing_raid_arrays(host_config)
                .context("Failed to clean up pre-existing RAID arrays")?;
            create_partitions(host_status, host_config)
                .context("Failed to create disk partitions")?;
            raid::create_sw_raid(host_status, host_config)
                .context("Failed to create software RAID")?;
            encryption::provision(host_status, host_config)
                .context("Encryption submodule failed during provision")?;
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

        // TODO: update /etc/repart.d directly for the matching disk, derive
        // from where is the root located

        encryption::configure(host_status)
            .context("Encryption submodule failed during configure")?;

        // persist on reboots
        raid::create_raid_config(host_status)
            .context("Failed to create mdadm.conf file after RAID creation")?;

        image::configure(host_status, host_config)
            .context("Image submodule failed during configure")?;

        Ok(())
    }
}

/// Find the mount point that is holding the given path. This is useful to find
/// the volume on which the given absolute path is located. This version uses HC
/// to find the information and is useful early in the process when HS has not
/// yet been populated.
pub(super) fn path_to_mount_point_from_config<'a>(
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

/// Find the mount point that is holding the given path. This is useful to find
/// the volume on which the given absolute path is located. This version uses HS
/// to find the information and is preferred as it refers to the status of the system.
fn path_to_mount_point_from_status<'a>(
    host_status: &'a HostStatus,
    path: &Path,
) -> Option<&'a status::MountPoint> {
    host_status
        .storage
        .mount_points
        .iter()
        .filter(|(mp_path, _)| path.starts_with(mp_path))
        .max_by_key(|(mp_path, _)| mp_path.as_os_str().len())
        .map(|(_, mp)| mp)
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
    use std::{fs::Permissions, os::unix::fs::PermissionsExt, str::FromStr};

    use tempfile::NamedTempFile;
    use trident_api::{
        config::{
            Disk, HostConfiguration, Image, ImageFormat, ImageSha256, Partition, PartitionSize,
            PartitionType, Raid, RaidLevel, SoftwareRaidArray, Storage,
        },
        constants::ROOT_MOUNT_POINT_PATH,
    };

    use super::*;

    fn get_host_status() -> HostStatus {
        HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            ..Default::default()
        }
    }

    // Create a temporary recovery key file. The file will be deleted once
    // the object returned is out of scope and dropped.
    pub fn get_recovery_key_file() -> NamedTempFile {
        let recovery_key_file: NamedTempFile = NamedTempFile::new().unwrap();
        let recovery_key_path: PathBuf = recovery_key_file.path().to_owned();
        fs::write(&recovery_key_path, "recovery-key").unwrap();
        fs::set_permissions(recovery_key_path, Permissions::from_mode(0o600)).unwrap();
        recovery_key_file
    }

    fn get_host_config(recovery_key_file: &tempfile::NamedTempFile) -> HostConfiguration {
        HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk1".to_owned(),
                        device: ROOT_MOUNT_POINT_PATH.into(),
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
                            Partition {
                                id: "part5".to_owned(),
                                partition_type: PartitionType::Srv,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    },
                ],
                raid: Raid {
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
                    path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                }],
                images: vec![Image {
                    target_id: "part1".to_owned(),
                    url: "".to_owned(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                }],
                ab_update: Some(trident_api::config::AbUpdate {
                    volume_pairs: vec![trident_api::config::AbVolumePair {
                        id: "ab1".to_owned(),
                        volume_a_id: "part1".to_owned(),
                        volume_b_id: "part2".to_owned(),
                    }],
                }),
                encryption: Some(trident_api::config::Encryption {
                    recovery_key_url: Some(
                        url::Url::from_file_path(recovery_key_file.path()).unwrap(),
                    ),
                    volumes: vec![trident_api::config::EncryptedVolume {
                        id: "enc1".to_owned(),
                        device_name: "luks-enc".to_owned(),
                        target_id: "part5".to_owned(),
                    }],
                }),
            },
            ..Default::default()
        }
    }

    /// Validates Storage module HostConfiguration validation logic.
    #[test]
    fn test_validate_host_config_pass() {
        let host_status = get_host_status();
        let recovery_key_file = get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        StorageModule
            .validate_host_config(&host_status, &host_config, ReconcileState::CleanInstall)
            .unwrap();
    }

    // Disk devices must be unique.
    #[test]
    fn tests_validate_host_config_duplicate_disk_path_fail() {
        let host_status = get_host_status();
        let recovery_key_file = get_recovery_key_file();
        let mut host_config = get_host_config(&recovery_key_file);

        host_config.storage.disks.get_mut(0).unwrap().device = "/tmp".into();

        assert_eq!(
            StorageModule
                .validate_host_config(&host_status, &host_config, ReconcileState::CleanInstall)
                .unwrap_err()
                .to_string(),
            "Disks 'disk2' and 'disk1' point to the same device '/tmp'"
        );
    }

    // Validating the Storage module include encryption configuration validation.
    #[test]
    fn test_validate_host_config_encryption_invalid_fail() {
        let host_status = get_host_status();
        let recovery_key_file = get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        // Delete the recovery key file to make the encryption configuration invalid.
        fs::remove_file(recovery_key_file.path()).unwrap();

        assert_eq!(
            StorageModule
                .validate_host_config(&host_status, &host_config, ReconcileState::CleanInstall)
                .unwrap_err()
                .to_string(),
            "Encryption host configuration validation failed"
        );
    }

    #[test]
    fn test_path_to_mount_point_from_config() {
        let mut host_config = get_host_config(&get_recovery_key_file());
        let mount_point = path_to_mount_point_from_config(
            &host_config,
            Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path(),
        )
        .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        // ensure to pick the longest prefix
        host_config.storage.mount_points.push(MountPoint {
            filesystem: "ext4".to_owned(),
            options: vec![],
            target_id: "part2".to_owned(),
            path: PathBuf::from(ROOT_MOUNT_POINT_PATH)
                .join("boot")
                .as_path()
                .into(),
        });

        let mount_point = path_to_mount_point_from_config(
            &host_config,
            Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path(),
        )
        .unwrap();
        assert_eq!(mount_point.target_id, "part2");

        // validate longer paths
        let mount_point = path_to_mount_point_from_config(
            &host_config,
            Path::new(ROOT_MOUNT_POINT_PATH)
                .join("boot/foo/bar")
                .as_path(),
        )
        .unwrap();
        assert_eq!(mount_point.target_id, "part2");

        let mount_point = path_to_mount_point_from_config(
            &host_config,
            Path::new(ROOT_MOUNT_POINT_PATH).join("foo/bar").as_path(),
        )
        .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        // validate failure without any mount points
        host_config.storage.mount_points.clear();
        assert!(path_to_mount_point_from_config(
            &host_config,
            Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path()
        )
        .is_none());
    }

    #[test]
    fn test_path_to_mount_point_from_status() {
        let mut host_status = get_host_status();
        let mount_point = status::MountPoint {
            target_id: "part1".to_owned(),
            filesystem: "ext4".to_owned(),
            options: vec![],
        };
        host_status.storage.mount_points.insert(
            PathBuf::from(ROOT_MOUNT_POINT_PATH).join("boot"),
            mount_point.clone(),
        );

        let mount_point = path_to_mount_point_from_status(
            &host_status,
            Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path(),
        )
        .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        // ensure to pick the longest prefix
        host_status.storage.mount_points.insert(
            PathBuf::from(ROOT_MOUNT_POINT_PATH),
            status::MountPoint {
                filesystem: "ext4".to_owned(),
                options: vec![],
                target_id: "part2".to_owned(),
            },
        );

        let mount_point = path_to_mount_point_from_status(
            &host_status,
            Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path(),
        )
        .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        // validate longer paths
        let mount_point = path_to_mount_point_from_status(
            &host_status,
            Path::new(ROOT_MOUNT_POINT_PATH)
                .join("boot/foo/bar")
                .as_path(),
        )
        .unwrap();
        assert_eq!(mount_point.target_id, "part1");

        let mount_point = path_to_mount_point_from_status(
            &host_status,
            Path::new(ROOT_MOUNT_POINT_PATH).join("foo/bar").as_path(),
        )
        .unwrap();
        assert_eq!(mount_point.target_id, "part2");

        // validate failure without any mount points
        host_status.storage.mount_points.clear();
        assert!(path_to_mount_point_from_status(
            &host_status,
            Path::new(ROOT_MOUNT_POINT_PATH).join("boot").as_path()
        )
        .is_none());
    }
}
