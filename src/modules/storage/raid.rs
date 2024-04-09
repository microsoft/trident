use anyhow::{bail, Context, Error};
use log::{debug, info, trace};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    str::FromStr,
};
use strum_macros::{Display, EnumString};
use trident_api::{
    config::{HostConfiguration, PartitionType, RaidLevel, SoftwareRaidArray},
    status::{self, BlockDeviceContents, HostStatus},
    BlockDeviceId,
};

use osutils::{block_devices, exe::OutputChecker, mdadm, udevadm};

use crate::modules::storage;

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

fn create(config: SoftwareRaidArray, host_status: &HostStatus) -> Result<(), Error> {
    let devices = &config.devices;
    let raid_path = PathBuf::from(format!("/dev/md/{}", &config.name));
    let device_paths =
        get_device_paths(host_status, devices).context("Failed to get device paths")?;

    mdadm::create(
        &raid_path,
        &config.level,
        &config.metadata_version,
        device_paths,
    )
    .context("Failed to create RAID array")?;
    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub(super) struct RaidDetail {
    pub id: BlockDeviceId,
    pub name: String,
    pub path: PathBuf,
    pub devices: Vec<PathBuf>,
    pub partition_type: PartitionType,
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
    let first_device = host_status
        .spec
        .storage
        .get_partition(&devices[0])
        .context("Failed to get partition")?;
    let first_device_type: PartitionType = first_device.partition_type;
    let component_size = osutils::files::read_file_trim(&md_folder.join("component_size"))?;

    // TODO(6331): fins a better way to get the size of the RAID array
    let array_size = component_size.parse::<u64>()? * 1024;

    let raid_detail = RaidDetail {
        id: raid_id.clone(),
        name: raid_array_name.clone(),
        path: raid_path,
        devices: device_paths.clone(),
        partition_type: first_device_type,
        uuid: raid_uuid,
        level: *raid_level,
        state: RaidState::from_str(&array_state)?,
        num_devices: raid_disks.parse::<u32>()?,
        metadata_version: metadata_version.clone(),
        size: array_size,
    };

    Ok(raid_detail)
}

fn get_raid_device_name(raid_device: &Path) -> Result<String, Error> {
    let device_name = match raid_device.file_name().and_then(|os_str| os_str.to_str()) {
        Some(name) => name,
        None => bail!("Invalid RAID device absolute path"),
    };

    Ok(device_name.to_string())
}

fn get_device_paths(
    host_status: &HostStatus,
    devices: &[BlockDeviceId],
) -> Result<Vec<PathBuf>, Error> {
    devices
        .iter()
        .map(|device_id| {
            host_status
                .storage
                .block_devices
                .get(device_id)
                .map(|block_device| block_device.path.clone())
                .context(format!("Failed to find block device with id '{device_id}'"))
        })
        .collect()
}

pub(super) fn create_raid_config(host_status: &HostStatus) -> Result<(), Error> {
    if !host_status.spec.storage.raid.software.is_empty() {
        info!("Creating mdadm config file");
        let output = mdadm::examine().context("Failed to examine RAID arrays")?;
        let mdadm_config_file_path = "/etc/mdadm/mdadm.conf";
        osutils::files::create_file(mdadm_config_file_path)
            .context("Failed to create mdadm config file")?;
        fs::write(Path::new(mdadm_config_file_path), output)
            .context("Failed to write mdadm config file")?;
    }
    Ok(())
}

// Create a new RaidArray and add it to the host status
pub(super) fn add_to_host_status(host_status: &mut HostStatus, raid_details: RaidDetail) {
    // let new_raid_array = status::RaidArray {
    //     device_paths: raid_details.devices,
    //     partition_type: raid_details.partition_type,
    //     name: raid_details.name.clone(),
    //     level: raid_details.level,
    //     status: RaidArrayStatus::Created,
    //     array_size: raid_details.size,
    //     ty: RaidType::Software,
    //     path: raid_details.path,
    //     uuid: Uuid::parse_str(&raid_details.uuid).unwrap(),
    //     contents: status::BlockDeviceContents::Unknown,
    // };

    // TODO: Track more details in HS
    host_status.storage.block_devices.insert(
        raid_details.id.clone(),
        status::BlockDeviceInfo {
            path: raid_details.path.clone(),
            size: raid_details.size,
            contents: status::BlockDeviceContents::Unknown,
        },
    );
}

// Update a RaidArray by ID
pub(super) fn update_raid_in_host_status(
    host_status: &mut HostStatus,
    id: &str,
    contents: status::BlockDeviceContents,
) -> Result<(), Error> {
    let raid_array = host_status
        .storage
        .block_devices
        .get_mut(id)
        .context(format!("Failed to update the RAID array: {id}"))?;

    // TODO: Also track status
    raid_array.contents = contents;

    Ok(())
}

pub(super) fn get_raid_disks(raid_array: &Path) -> Result<HashSet<PathBuf>, Error> {
    // If there is no mdstat file, there are no pre-existing RAID arrays
    if !Path::new("/proc/mdstat").exists() {
        trace!("No pre-existing RAID arrays found. Skipping cleanup.");
        return Ok(HashSet::new());
    }

    // Check if mdadm is present, we need it to stop RAID arrays.
    check_if_mdadm_present().context(
        "Failed to clean up pre-existing RAID arrays. Mdadm is required for RAID operations",
    )?;

    let mdadm_detail = mdadm::detail(raid_array).context("Failed to get existing RAID details")?;
    get_raid_disks_internal(&mdadm_detail)
}

fn get_raid_disks_internal(mdadm_detail: &mdadm::MdadmDetail) -> Result<HashSet<PathBuf>, Error> {
    let raid_disks = mdadm_detail
        .clone()
        .devices
        .into_iter()
        .map(|device| {
            block_devices::get_disk_for_partition(&device).with_context(|| {
                format!(
                    "Failed to get disk for partition in an existing RAID: {:?}",
                    mdadm_detail.raid_path
                )
            })
        })
        .collect::<Result<HashSet<_>, Error>>()
        .context("Failed to get RAID disks")?;

    Ok(raid_disks)
}

pub(super) fn stop_pre_existing_raid_arrays(host_config: &HostConfiguration) -> Result<(), Error> {
    // If there is no mdstat file, there are no pre-existing RAID arrays
    if !Path::new("/proc/mdstat").exists() {
        trace!("No pre-existing RAID arrays found. Skipping cleanup.");
        return Ok(());
    }

    // Check if mdadm is present, we need it to stop RAID arrays.
    check_if_mdadm_present().context(
        "Failed to clean up pre-existing RAID arrays. Mdadm is required for RAID operations",
    )?;

    debug!("Attempting to stop pre-existing RAID arrays");

    let mdadm_detail = mdadm::details().context("Failed to get existing RAID details")?;

    if mdadm_detail.is_empty() {
        trace!(
            "Mdstat file is present however, nothing found in RAID details scan. Skipping cleanup."
        );
        return Ok(());
    }

    let trident_disks = super::get_hostconfig_disk_paths(host_config)
        .context("Failed to get disks defined in Host Configuration")?;

    for raid_array in mdadm_detail {
        let raid_device_resolved_path = raid_array
            .raid_path
            .clone()
            .canonicalize()
            .context("Failed to get existing RAID device resolved path")?;

        let raid_disks = get_raid_disks_internal(&raid_array)?;
        if block_devices::can_stop_pre_existing_device(
            &raid_disks,
            &trident_disks.iter().cloned().collect::<HashSet<_>>(),
        )
        .context(format!(
            "Failed to stop RAID array '{}'",
            raid_array.raid_path.display()
        ))? {
            unmount_and_stop(&raid_device_resolved_path)?;
        }
    }

    Ok(())
}

fn check_if_mdadm_present() -> Result<(), Error> {
    let mut mdadm_command = Command::new("mdadm");
    mdadm_command.arg("--version");

    mdadm_command
        .output()
        .context("Failed to run mdadm command. Mdadm is required for RAID operations")?;

    Ok(())
}

#[derive(Serialize, Deserialize, Clone, Debug, Hash, Eq, PartialEq, Default)]
struct MdadmDetail {
    raid_path: PathBuf,
    level: String,
    uuid: String,
    devices: Vec<PathBuf>,
}

pub fn unmount_and_stop(raid_path: &Path) -> Result<(), Error> {
    debug!("Unmounting RAID array: {:?}", raid_path);
    let mut umount_command = Command::new("umount");
    umount_command.arg(raid_path);

    let output = umount_command
        .output()
        .context("Failed to unmount RAID array")?;

    if !output.stderr.is_empty() {
        let stderr_str = String::from_utf8_lossy(&output.stderr);

        // Error code 32 means there was a mount faliure (device not mounted)
        if !stderr_str.contains("not mounted") || output.exit_code() != Some(32) {
            bail!("Failed to unmount: {:?}", raid_path);
        }
    }
    mdadm::stop(raid_path).context("Failed to stop RAID array")?;

    Ok(())
}

pub(super) fn create_sw_raid(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    if !host_config.storage.raid.software.is_empty() {
        check_if_mdadm_present()
            .context("Failed to create software RAID. Mdadm is required for RAID")?;
        for software_raid_config in &host_config.storage.raid.software {
            create_sw_raid_array(host_status, software_raid_config).context(format!(
                "RAID creation failed for '{}'",
                software_raid_config.name
            ))?;
        }
        udevadm::trigger().context("Udev failed while scanning for new devices")?;

        for software_raid_config in &host_config.storage.raid.software {
            update_raid_in_host_status(
                host_status,
                &software_raid_config.id,
                status::BlockDeviceContents::Unknown,
            )
            .context(format!(
                "Failed to update host status for RAID: '{}'",
                software_raid_config.name
            ))?;
        }
    }

    Ok(())
}

pub fn create_sw_raid_array(
    host_status: &mut HostStatus,
    config: &SoftwareRaidArray,
) -> Result<(), Error> {
    create(config.clone(), host_status)
        .context(format!("Failed to create RAID array '{}'", config.name))?;

    let raid_device = &PathBuf::from(format!("/dev/md/{}", &config.name));

    // Wait for symlink to appear. Kernel creates /dev/mdXX and udev crates symlink (raid_device)
    udevadm::wait(raid_device).context(format!(
        "Failed waiting for RAID device '{}' to appear",
        raid_device.display()
    ))?;

    let raid_device_resolved_path = raid_device
        .canonicalize()
        .context("Unable to find RAID device resolved path after RAID creation")?;

    let raid_details = get_raid_details(&raid_device_resolved_path, config.clone(), host_status)
        .context("Failed to read RAID details after creation")?;

    add_to_host_status(host_status, raid_details.clone());

    for block_device_id in &config.devices {
        storage::set_host_status_block_device_contents(
            host_status,
            block_device_id,
            BlockDeviceContents::Initialized,
        )
        .context(format!(
            "Failed to set block device contents for block device '{}'",
            block_device_id,
        ))?
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use maplit::btreemap;

    use trident_api::{
        config::{Disk, Partition, PartitionSize, PartitionType, Storage},
        status::{BlockDeviceContents, BlockDeviceInfo, ReconcileState, Storage as StorageStatus},
    };

    use super::*;

    #[test]
    fn test_get_device_paths() {
        let host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                size: PartitionSize::Fixed(1000),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root".to_string(),
                                size: PartitionSize::Fixed(1000),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "home".to_string(),
                                size: PartitionSize::Grow,
                                partition_type: PartitionType::LinuxGeneric,
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: StorageStatus {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda2"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "home".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda3"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let result: Result<Vec<PathBuf>, Error> =
            get_device_paths(&host_status, &["boot".to_string(), "root".to_string()]);

        assert!(result.is_ok());

        let device_paths = result.unwrap();
        assert_eq!(device_paths.len(), 2);

        let expected_paths = vec![PathBuf::from("/dev/sda1"), PathBuf::from("/dev/sda2")];

        assert_eq!(device_paths, expected_paths);

        let result: Result<Vec<PathBuf>, Error> =
            get_device_paths(&host_status, &["boot2".to_string(), "root2".to_string()]);
        assert!(result.is_err());
    }

    #[test]
    fn test_create_raid_array() {
        let raid_details: RaidDetail = RaidDetail {
            id: "some_raid".to_string(),
            name: "raid1".to_string(),
            path: PathBuf::from("/dev/md/some_raid"),
            devices: vec![PathBuf::from("/dev/sda1"), PathBuf::from("/dev/sdb1")],
            partition_type: PartitionType::LinuxGeneric,
            uuid: "00000000-0000-0000-0000-000000000000".to_string(),
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
            .block_devices
            .contains_key(&raid_details.id));
    }

    #[test]
    fn test_update_raid_status() {
        let host_status = &mut HostStatus::default();

        host_status.storage.block_devices.insert(
            "some_raid".to_string(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/md/some_raid"),
                size: 12345,
                contents: BlockDeviceContents::Unknown,
            },
        );

        let _ = update_raid_in_host_status(
            host_status,
            "some_raid",
            status::BlockDeviceContents::Initialized,
        );
        assert_eq!(
            host_status
                .storage
                .block_devices
                .get("some_raid")
                .unwrap()
                .contents,
            status::BlockDeviceContents::Initialized
        );
    }

    #[test]
    fn test_get_raid_device_name() {
        let raid_device = Path::new("/dev/md/my-raid");

        let device_name =
            get_raid_device_name(raid_device).expect("Failed to get RAID device name");

        assert_eq!(device_name, "my-raid");

        let raid_device = Path::new("/dev/md127");

        let device_name =
            get_raid_device_name(raid_device).expect("Failed to get RAID device name");

        assert_eq!(device_name, "md127");
    }
}
