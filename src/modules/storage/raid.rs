use anyhow::{bail, Context, Error};
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};
use strum_macros::{Display, EnumString};
use trident_api::{
    config::{BlockDeviceId, HostConfiguration, RaidLevel, SoftwareRaidArray},
    status::{self, HostStatus, RaidArrayStatus, RaidType},
};

use osutils::exe::OutputChecker;
pub(super) const RAID_SYNC_TIMEOUT_SECS: u64 = 180;

#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq, Display, EnumString)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub(super) enum RaidState {
    /// in a clean, healthy state
    #[strum(serialize = "clean")]
    Clean,
    /// active and operational  
    #[strum(serialize = "active")]
    Active,
    /// IO error
    #[strum(serialize = "inactive")]
    Inactive,
}

pub(super) fn create(config: SoftwareRaidArray, host_status: &HostStatus) -> Result<Output, Error> {
    let devices = &config.devices;
    let raid_path = PathBuf::from(format!("/dev/md/{}", &config.name));
    let device_paths =
        get_device_paths(host_status, devices).context("Failed to get device paths")?;
    info!("Creating RAID array '{}'", raid_path.to_string_lossy());

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command
        .arg("--create")
        .arg(raid_path)
        .arg(format!("--level={}", &config.level.to_string()))
        .arg(format!("--raid-devices={}", &config.devices.len()))
        .args(device_paths)
        .arg(format!("--metadata={}", &config.metadata_version));

    let output = mdadm_command
        .output()
        .context("Failed to run mdadm create")?;
    output.check().context("mdadm exited with an error")?;
    Ok(output)
}

pub(super) fn stop_all() -> Result<Output, Error> {
    info!("Stopping all RAID arrays");

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command.arg("--stop").arg("--scan");

    let output = mdadm_command.output().context("Failed to run mdadm stop")?;
    output.check().context("mdadm exited with an error")?;
    Ok(output)
}

fn examine() -> Result<Output, Error> {
    info!("Examining RAID arrays");

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command.arg("--examine").arg("--scan");

    let output = mdadm_command
        .output()
        .context("Failed to run mdadm examine")?;
    output.check().context("mdadm exited with an error")?;
    Ok(output)
}

#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub(super) struct RaidDetail {
    pub id: BlockDeviceId,
    pub name: String,
    pub path: PathBuf,
    pub symlink_path: PathBuf,
    pub devices: Vec<PathBuf>,
    pub uuid: String,
    pub level: RaidLevel,
    pub state: RaidState,
    pub num_devices: u32,
    pub metadata_version: String,
    pub size: u64,
}

pub(super) fn get_raid_details(
    raid_device: &Path,
    config: SoftwareRaidArray,
    host_status: &HostStatus,
) -> Result<RaidDetail, Error> {
    let device_name = get_raid_device_name(raid_device)?;

    let md_folder = PathBuf::from(format!("/sys/devices/virtual/block/{}/md", device_name));

    let array_state = osutils::files::read_file_trim(&md_folder.join("array_state"))?;
    let component_size =
        osutils::files::read_file_trim(&md_folder.join("component_size"))?.parse::<u64>()?;
    let raid_disks = osutils::files::read_file_trim(&md_folder.join("raid_disks"))?;
    let raid_uuid = osutils::files::read_file_trim(&md_folder.join("uuid"))?;
    let raid_level = &config.level;
    let raid_array_name = &config.name;
    let devices = &config.devices;
    let metadata_version = &config.metadata_version;
    let raid_id = &config.id;
    let raid_path = PathBuf::from(format!("/dev/md/{}", raid_array_name));
    let device_paths =
        get_device_paths(host_status, devices).context("Failed to get device paths")?;

    let raid_detail = RaidDetail {
        id: raid_id.clone(),
        name: raid_array_name.clone(),
        path: raid_path,
        symlink_path: raid_device.to_path_buf(),
        devices: device_paths.clone(),
        uuid: raid_uuid,
        level: *raid_level,
        state: RaidState::from_str(&array_state)?,
        num_devices: raid_disks.parse::<u32>()?,
        metadata_version: metadata_version.clone(),
        size: component_size,
    };

    Ok(raid_detail)
}

fn get_raid_device_name(raid_device: &Path) -> Result<String, Error> {
    let device_name = match raid_device.file_name().and_then(|os_str| os_str.to_str()) {
        Some(name) => name,
        None => bail!("Invalid RAID symlink path"),
    };

    Ok(device_name.to_string())
}

pub(super) fn get_device_paths(
    host_status: &HostStatus,
    devices: &Vec<BlockDeviceId>,
) -> Result<Vec<PathBuf>, Error> {
    let mut device_paths = Vec::new();

    for device_id in devices {
        let partition = get_partition_by_id(host_status, device_id)?;
        let path = partition.path.clone();
        device_paths.push(path);
    }

    Ok(device_paths)
}

// function to get partition by id from host status
fn get_partition_by_id<'a>(
    host_status: &'a HostStatus,
    partition_id: &str,
) -> Result<&'a status::Partition, Error> {
    for disk in host_status.storage.disks.values() {
        for partition in &disk.partitions {
            if partition.id == partition_id {
                return Ok(partition);
            }
        }
    }

    bail!("Failed to find partition with id '{partition_id}'")
}

pub(super) fn create_raid_config(host_status: &HostStatus) -> Result<(), Error> {
    if !host_status.storage.raid_arrays.is_empty() {
        info!("Creating mdadm config file");
        let output = examine().context("Failed to run mdadm --examine");
        if let Ok(output) = output {
            let mdadm_config_file_path = "/etc/mdadm/mdadm.conf";
            osutils::files::create_file(mdadm_config_file_path)
                .context("Failed to create mdadm config file")?;
            fs::write(Path::new(mdadm_config_file_path), output.stdout)
                .context("Failed to write mdadm config file")?;
        } else {
            bail!("Failed to create mdadm config file. mdadm --examine failed");
        }
    }
    Ok(())
}

// Create a new RaidArray and add it to the host status
pub(super) fn add_to_host_status(host_status: &mut HostStatus, raid_details: RaidDetail) {
    let new_raid_array = status::RaidArray {
        device_paths: raid_details.devices,
        name: raid_details.name.clone(),
        level: raid_details.level,
        status: RaidArrayStatus::Created,
        array_size: raid_details.size,
        ty: RaidType::Software,
        path: raid_details.path,
        raid_symlink_path: raid_details.symlink_path,
        contents: status::BlockDeviceContents::Unknown,
    };

    host_status
        .storage
        .raid_arrays
        .insert(raid_details.id.clone(), new_raid_array);
}

// Update a RaidArray by ID
pub(super) fn update_raid_in_host_status(
    host_status: &mut HostStatus,
    id: &str,
    status: RaidArrayStatus,
    contents: status::BlockDeviceContents,
) -> Result<(), Error> {
    let raid_array = host_status
        .storage
        .raid_arrays
        .get_mut(id)
        .context(format!("Failed to update the RAID array: {id}"))?;

    raid_array.status = status;
    raid_array.contents = contents;

    Ok(())
}

pub(super) fn create_sw_raid(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    if !host_config.storage.raid.software.is_empty() {
        for software_raid_config in &host_config.storage.raid.software {
            create_sw_raid_array(host_status, software_raid_config).context(format!(
                "RAID creation failed for '{}'",
                software_raid_config.name
            ))?;
        }
        wait_for_raid_resync(&host_status.storage.raid_arrays)?;
        super::udevadm_trigger().context("Udev failed while scanning for new devices")?;

        for software_raid_config in &host_config.storage.raid.software {
            update_raid_in_host_status(
                host_status,
                &software_raid_config.id,
                RaidArrayStatus::Ready,
                status::BlockDeviceContents::Initialized,
            )
            .context(format!(
                "Failed to update host status for RAID: '{}'",
                software_raid_config.name
            ))?;
        }
    }

    Ok(())
}

fn wait_for_raid_resync(
    raid_arrays: &BTreeMap<BlockDeviceId, status::RaidArray>,
) -> Result<(), Error> {
    info!("Waiting for RAID arrays to be in an idle state");

    let start_time = Instant::now();
    let max_duration = Duration::from_secs(RAID_SYNC_TIMEOUT_SECS);

    let mut raid_devices: Vec<(String, String)> = raid_arrays
        .values()
        .map(|raid_array| {
            let symlink_name = get_raid_device_name(raid_array.raid_symlink_path.as_path())
                .context("Failed to get RAID symlink")?;
            Ok((symlink_name, "".to_string()))
        })
        .collect::<Result<Vec<(String, String)>, Error>>()?;

    loop {
        let mut all_idle = true;

        // check if any RAID devices are not idle
        raid_devices
            .iter_mut()
            .filter(|(_, sync_status)| *sync_status != "idle")
            .for_each(|(raid_device, sync_status)| {
                if let Ok(sync_action) = osutils::files::read_file_trim(&PathBuf::from(format!(
                    "/sys/block/{}/md/sync_action",
                    raid_device.clone()
                ))) {
                    *sync_status = sync_action.clone();
                    all_idle = false;
                }
            });

        if all_idle {
            break;
        }

        if start_time.elapsed() >= max_duration {
            let non_idle_devices: Vec<(&String, &String)> = raid_devices
                .iter()
                .filter(|(_, sync_status)| *sync_status != "idle")
                .map(|(raid_device, sync_status)| (raid_device, sync_status))
                .collect();

            for (raid_device, sync_status) in &non_idle_devices {
                warn!(
                    "RAID device '{}' sync status is '{}'",
                    raid_device, sync_status
                );
            }

            if !non_idle_devices.is_empty() {
                bail!("Timed out waiting for RAID to be in a clean state");
            }
        }

        thread::sleep(Duration::from_secs(5));
    }

    Ok(())
}

fn create_sw_raid_array(
    host_status: &mut HostStatus,
    config: &SoftwareRaidArray,
) -> Result<(), Error> {
    create(config.clone(), host_status)
        .context(format!("Failed to create RAID array '{}'", config.name))?;

    let raid_device = &PathBuf::from(format!("/dev/md/{}", &config.name));

    let raid_symlink_path = raid_device
        .canonicalize()
        .context("Unable to find RAID device symlink after RAID creation")?;

    let raid_details = get_raid_details(&raid_symlink_path, config.clone(), host_status)
        .context("Failed to read RAID details after creation")?;

    add_to_host_status(host_status, raid_details.clone());

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use crate::modules::storage;

    use super::*;
    use trident_api::{
        config::PartitionType,
        status::{BlockDeviceContents, Disk, Partition, RaidArray, ReconcileState, Storage},
    };
    use uuid::Uuid;

    #[test]
    fn test_get_device_paths() {
        let mut disks = BTreeMap::new();

        disks.insert(
            "some_disk".to_string(),
            Disk {
                uuid: Uuid::nil(),
                path: PathBuf::from("/dev/sda"),
                capacity: 10,
                partitions: vec![
                    Partition {
                        id: "boot".to_string(),
                        path: PathBuf::from("/dev/sda1"),
                        start: 1,
                        end: 3,
                        ty: PartitionType::Esp,
                        contents: BlockDeviceContents::Initialized,
                        uuid: Uuid::nil(),
                    },
                    Partition {
                        id: "root".to_string(),
                        path: PathBuf::from("/dev/sda2"),
                        start: 4,
                        end: 10,
                        ty: PartitionType::Root,
                        contents: BlockDeviceContents::Initialized,
                        uuid: Uuid::nil(),
                    },
                ],
                contents: BlockDeviceContents::Initialized,
            },
        );

        let host_status = HostStatus {
            reconcile_state: ReconcileState::Ready,
            storage: Storage {
                disks,
                raid_arrays: BTreeMap::new(),
                mount_points: BTreeMap::new(),
            },
            ..Default::default()
        };

        let result: Result<Vec<PathBuf>, Error> =
            get_device_paths(&host_status, &vec!["boot".to_string(), "root".to_string()]);

        assert!(result.is_ok());

        let device_paths = result.unwrap();
        assert_eq!(device_paths.len(), 2);

        let expected_paths = vec![PathBuf::from("/dev/sda1"), PathBuf::from("/dev/sda2")];

        assert_eq!(device_paths, expected_paths);

        let result: Result<Vec<PathBuf>, Error> = get_device_paths(
            &host_status,
            &vec!["boot2".to_string(), "root2".to_string()],
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_create_raid_array() {
        let raid_details: RaidDetail = RaidDetail {
            id: "some_raid".to_string(),
            name: "raid1".to_string(),
            path: PathBuf::from("/dev/md/some_raid"),
            symlink_path: PathBuf::from("/dev/md127"),
            devices: vec![PathBuf::from("/dev/sda1"), PathBuf::from("/dev/sdb1")],
            uuid: "uuid".to_string(),
            level: RaidLevel::Raid1,
            state: RaidState::Clean,
            num_devices: 2,
            metadata_version: "1.0".to_string(),
            size: 12345,
        };

        let host_status = &mut HostStatus::default();
        add_to_host_status(host_status, raid_details.clone());

        assert!(host_status
            .storage
            .raid_arrays
            .contains_key(&raid_details.id));
    }

    #[test]
    fn test_update_raid_status() {
        let host_status = &mut HostStatus::default();

        host_status.storage.raid_arrays.insert(
            "some_raid".to_string(),
            RaidArray {
                device_paths: vec![PathBuf::from("/dev/sda1"), PathBuf::from("/dev/sdb1")],
                name: "raid1".to_string(),
                level: RaidLevel::Raid1,
                status: RaidArrayStatus::Created,
                array_size: 12345,
                ty: RaidType::Software,
                path: PathBuf::from("/dev/md/some_raid"),
                raid_symlink_path: PathBuf::from("/dev/md127"),
                contents: status::BlockDeviceContents::Unknown,
            },
        );

        let _ = update_raid_in_host_status(
            host_status,
            "some_raid",
            RaidArrayStatus::Ready,
            status::BlockDeviceContents::Initialized,
        );
        assert_eq!(
            host_status
                .storage
                .raid_arrays
                .get("some_raid")
                .unwrap()
                .status,
            RaidArrayStatus::Ready
        );
        assert_eq!(
            host_status
                .storage
                .raid_arrays
                .get("some_raid")
                .unwrap()
                .contents,
            status::BlockDeviceContents::Initialized
        );
    }

    #[test]
    fn test_get_partition_from_host_config() {
        let host_config = HostConfiguration {
            storage: trident_api::config::Storage {
                disks: vec![trident_api::config::Disk {
                    id: "some_disk".to_string(),
                    partitions: vec![
                        trident_api::config::Partition {
                            id: "some_partition".to_string(),
                            partition_type: trident_api::config::PartitionType::LinuxGeneric,
                            size: trident_api::config::PartitionSize::Fixed(123),
                        },
                        trident_api::config::Partition {
                            id: "some_partition2".to_string(),
                            partition_type: trident_api::config::PartitionType::LinuxGeneric,
                            size: trident_api::config::PartitionSize::Fixed(456),
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let partition = storage::get_partition_from_host_config(&host_config, "some_partition")
            .expect("Expected to find a partition but not found.");

        assert_eq!(partition.id, "some_partition");
        assert_eq!(
            partition.partition_type,
            trident_api::config::PartitionType::LinuxGeneric
        );
        assert_eq!(
            partition.size,
            trident_api::config::PartitionSize::Fixed(123)
        );

        let partition =
            storage::get_partition_from_host_config(&host_config, "non_existing_partition");
        assert_eq!(partition, None);
    }

    #[test]
    fn test_get_raid_array_ids() {
        let host_config = HostConfiguration {
            storage: trident_api::config::Storage {
                disks: vec![trident_api::config::Disk {
                    id: "some_disk".to_string(),
                    partitions: vec![
                        trident_api::config::Partition {
                            id: "some_partition".to_string(),
                            partition_type: trident_api::config::PartitionType::LinuxGeneric,
                            size: trident_api::config::PartitionSize::Fixed(123),
                        },
                        trident_api::config::Partition {
                            id: "some_partition2".to_string(),
                            partition_type: trident_api::config::PartitionType::LinuxGeneric,
                            size: trident_api::config::PartitionSize::Fixed(456),
                        },
                    ],
                    ..Default::default()
                }],
                raid: trident_api::config::RaidConfig {
                    software: vec![trident_api::config::SoftwareRaidArray {
                        id: "some_raid".to_string(),
                        name: "raid1".to_string(),
                        level: trident_api::config::RaidLevel::Raid1,
                        devices: vec!["some_partition".to_string(), "some_partition2".to_string()],
                        metadata_version: "1.0".to_string(),
                    }],
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let raid_array_ids = storage::get_raid_array_ids(&host_config);
        assert_eq!(raid_array_ids.len(), 1);
        assert!(raid_array_ids.contains(&"some_raid".to_string()));
    }
}
