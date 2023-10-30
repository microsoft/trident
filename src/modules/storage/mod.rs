use anyhow::{bail, Context, Error};
use log::info;
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};
use trident_api::{
    config::{BlockDeviceId, HostConfiguration, MountPoint, Partition},
    status::{self, BlockDeviceContents, HostStatus, UpdateKind},
};

use crate::modules::Module;

use sdrepart::RepartConfiguration;

use tabfile::{TabFile, DEFAULT_FSTAB_PATH};
mod raid;
mod sdrepart;
mod sfdisk;
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

        let repart_config = RepartConfiguration::new(disk).context(format!(
            "Failed to generate systemd-repart config for disk {}",
            disk.id
        ))?;

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
        crate::run_command(Command::new("udevadm").arg("settle"))?;

        for (index, partition) in disk.partitions.iter().enumerate() {
            let partition_uuid = partitions_status[index].uuid;
            let part_path = Path::new("/dev/disk/by-partuuid").join(partition_uuid.to_string());

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

        Ok(())
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        // TODO: reject any partition changes if we're not doing a clean install.

        // Ensure block device naming is unique across all supported block
        // device types.
        let mut block_device_ids = std::collections::HashSet::new();

        StorageModule::check_multiple_instances(
            &mut block_device_ids,
            &mut host_config.storage.disks.iter().map(|d| &d.id),
        )?;

        let partition_ids: Vec<&String> = host_config
            .storage
            .disks
            .iter()
            .flat_map(|d| &d.partitions)
            .map(|p| &p.id)
            .collect();
        let partition_ids_set: HashSet<&String> = partition_ids.iter().cloned().collect();
        let mut image_target_ids: HashSet<&String> = partition_ids_set.clone();

        StorageModule::check_multiple_instances(
            &mut block_device_ids,
            &mut partition_ids.clone().into_iter(),
        )?;

        StorageModule::check_multiple_instances(
            &mut block_device_ids,
            &mut host_config.storage.raid.software.iter().map(|r| &r.id),
        )?;

        if let Some(ab_update) = &host_config.imaging.ab_update {
            let ab_volume_ids: Vec<&String> =
                ab_update.volume_pairs.iter().map(|v| &v.id).collect();
            image_target_ids.extend(ab_volume_ids.clone());
            StorageModule::check_multiple_instances(
                &mut block_device_ids,
                &mut ab_volume_ids.into_iter(),
            )?;
        }

        // Ensure valid references.
        if let Some(ab_update) = &host_config.imaging.ab_update {
            for p in &ab_update.volume_pairs {
                for block_device_id in [&p.volume_a_id, &p.volume_b_id] {
                    if !partition_ids_set.contains(block_device_id) {
                        bail!(
                            "Block device id '{id}' was set as dependency of an A/B update volume '{parent}', but is not defined elsewhere",
                            id = block_device_id,
                            parent = p.id,
                        );
                    }
                }
            }
        }

        let raid_ids: HashSet<String> = get_raid_array_ids(host_config);

        for image in &host_config.imaging.images {
            if !image_target_ids.contains(&image.target_id) {
                bail!(
                    "Block device id '{id}' was set as dependency of an image, but is not defined elsewhere",
                    id = image.target_id,
                );
            }
            if raid_ids.contains(&image.target_id) {
                bail!(
                    "Image id '{}' targets a RAID array, which is not supported",
                    image.target_id,
                );
            }
        }

        for mount_point in &host_config.storage.mount_points {
            if !image_target_ids.contains(&mount_point.target_id)
                && !raid_ids.contains(&mount_point.target_id)
            {
                bail!(
                    "Block device id '{id}' was set as dependency of a mount point, but is not defined as an id of a partition or a raid array",
                    id = mount_point.target_id,
                );
            }
        }

        // Ensure mutual exclusivity
        if let Some(ab_update) = &host_config.imaging.ab_update {
            for p in &ab_update.volume_pairs {
                if p.volume_a_id == p.volume_b_id {
                    bail!(
                        "A/B update volume id '{id}' has the same target_id both both volumes",
                        id = p.id,
                    );
                }
            }
        }

        // Check that devices are valid partitions and only part of a single RAID array
        let mut raid_devices = HashSet::<BlockDeviceId>::new();
        for software_raid_config in &host_config.storage.raid.software {
            for device_id in &software_raid_config.devices {
                if get_partition_from_host_config(host_config, device_id).is_none() {
                    bail!("Device id '{device_id}' was set as dependency of a RAID array, but is not a valid partition");
                }
                if !raid_devices.insert(device_id.clone()) {
                    bail!("Block device '{device_id}' cannot be part of multiple RAID arrays");
                }
            }
        }

        Ok(())
    }

    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        None
    }

    fn migrate(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        _mount_path: &Path,
    ) -> Result<(), Error> {
        if host_status.reconcile_state != status::ReconcileState::CleanInstall {
            return Ok(());
        }
        raid::stop_all().context("Failed to stop all existing RAID arrays")?;
        create_partitions(host_status, host_config).context("Failed to create disk partitions")?;
        raid::create_sw_raid(host_status, host_config).context("Failed to create software RAID")?;
        Ok(())
    }

    fn reconcile(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        TabFile::from_mount_points(host_status, &host_config.storage.mount_points, None, None)
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

        Ok(())
    }
}

impl StorageModule {
    fn check_multiple_instances<'a>(
        detected_ids: &mut HashSet<&'a String>,
        input_ids: &mut dyn Iterator<Item = &'a String>,
    ) -> Result<(), Error> {
        for name in input_ids {
            if !detected_ids.insert(name) {
                bail!("Block device name '{name}' is used more than once");
            }
        }

        Ok(())
    }
}

pub fn get_partition_from_host_config<'a>(
    host_config: &'a HostConfiguration,
    partition_id: &'a str,
) -> Option<&'a Partition> {
    host_config
        .storage
        .disks
        .iter()
        .flat_map(|disk| disk.partitions.iter())
        .find(|partition| partition.id == partition_id)
}

fn udevadm_trigger() -> Result<(), Error> {
    info!("Triggering udevadm to rescan devices...");

    let trigger_output = Command::new("udevadm").arg("trigger").output()?;

    if !trigger_output.status.success() {
        bail!(
            "Udevadm trigger failed:\n{:?}",
            String::from_utf8(trigger_output.stderr)
        );
    }
    Ok(())
}

fn get_raid_array_ids(host_config: &HostConfiguration) -> HashSet<String> {
    host_config
        .storage
        .raid
        .software
        .iter()
        .map(|config| config.id.clone())
        .collect()
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
    for f in fs::read_dir(directory)?.flatten() {
        if let Ok(target_path) = f.path().canonicalize() {
            if target_path == target {
                return Ok(f.path());
            }
        }
    }

    bail!("Failed to find symlink for '{}'", target.display())
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use indoc::indoc;
    use trident_api::config::{HostConfiguration, Partition, PartitionSize, PartitionType};

    use super::*;

    /// Validates Storage module HostConfiguration validation logic.
    #[test]
    fn test_validate_host_config() {
        let empty_host_config_yaml = indoc! {r#"
            storage:
                disks:
            imaging:
                images:
        "#};
        let empty_host_config = serde_yaml::from_str::<HostConfiguration>(empty_host_config_yaml)
            .expect("Failed to parse empty host config");

        let empty_host_status_yaml = indoc! {r#"
            reconcile-state: clean-install
            storage:
                disks:
                mount-points:
                raid-arrays:
            imaging:
        "#};
        let empty_host_status = serde_yaml::from_str(empty_host_status_yaml)
            .expect("Failed to parse empty host status");

        let storage_module = StorageModule {};

        storage_module
            .validate_host_config(&empty_host_status, &empty_host_config)
            .unwrap();

        let host_config_yaml = indoc! {r#"
            storage:
                disks:
                  - id: disk1
                    device: /dev/sda
                    partition-table-type: gpt
                    partitions:
                  - id: disk2
                    device: /dev/sdb
                    partition-table-type: gpt
                    partitions:
                      - id: part1
                        type: esp
                        size: 1M
                      - id: part2
                        type: root
                        size: 1G
                mount-points:
                  - filesystem: ext4
                    options: []
                    target-id: part1
                    path: /
            imaging:
                images:
                  - target-id: part1
                    url: ""
                    sha256: ""
                    format: raw-zstd
                ab-update:
                    volume-pairs:
                      - id: ab1
                        volume-a-id: part1
                        volume-b-id: part2
        "#};
        let mut host_config = serde_yaml::from_str::<HostConfiguration>(host_config_yaml)
            .expect("Failed to parse host config");

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_ok());

        let host_config_golden = host_config.clone();

        // fail on duplicate id
        host_config.storage.disks.get_mut(0).unwrap().partitions = vec![Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::Esp,
            size: PartitionSize::from_str("1M").unwrap(),
        }];

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on duplicate id
        host_config.imaging.ab_update.as_mut().unwrap().volume_pairs[0].id = "disk1".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on missing reference (disk4 does not exist)
        host_config.imaging.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id =
            "disk4".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on missing reference (disk4 does not exist)
        host_config.imaging.images[0].target_id = "disk4".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on missing reference (disk4 does not exist)
        host_config.storage.mount_points[0].target_id = "disk4".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on bad block device type
        host_config.imaging.images[0].target_id = "disk1".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());
    }
}
