use anyhow::{bail, Context, Error};
use log::{info, warn};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};
use strum_macros::{Display, EnumString};
use trident_api::{
    config::{HostConfiguration, PartitionType, RaidLevel, SoftwareRaidArray},
    status::{self, HostStatus, RaidArrayStatus, RaidType},
    BlockDeviceId,
};
use uuid::Uuid;

use osutils::{
    exe::{OutputChecker, RunAndCheck},
    lsblk, udevadm,
};

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

pub(super) fn create(config: SoftwareRaidArray, host_status: &HostStatus) -> Result<(), Error> {
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

    mdadm_command
        .output()
        .context("Failed to run mdadm create")?
        .check()
        .context("mdadm exited with an error")?;
    Ok(())
}

fn examine() -> Result<String, Error> {
    info!("Examining RAID arrays");

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command.arg("--examine").arg("--scan");

    mdadm_command.output_and_check()
}

fn detail() -> Result<Output, Error> {
    info!("Getting RAID array details");

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command.arg("--detail").arg("--scan").arg("--verbose");

    let output = mdadm_command
        .output()
        .context("Failed to run mdadm detail")?;
    output.check().context("mdadm exited with an error")?;
    Ok(output)
}

fn stop(raid_array_name: &Path) -> Result<(), Error> {
    info!("Stopping RAID array: {:?}", raid_array_name);

    let mut mdadm_command = Command::new("mdadm");
    mdadm_command.arg("--stop").arg(raid_array_name);

    mdadm_command
        .output()
        .context("Failed to run mdadm stop")?
        .check()
        .context("mdadm exited with an error")?;

    Ok(())
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
    let first_device: &status::Partition = get_partition_by_id(host_status, &devices[0])?;
    let first_device_type: PartitionType = first_device.ty;
    let component_size = osutils::files::read_file_trim(&md_folder.join("component_size"))?;

    // TODO(6331): fins a better way to get the size of the RAID array
    let array_size = component_size.parse::<u64>()? * 1024;

    let raid_detail = RaidDetail {
        id: raid_id.clone(),
        name: raid_array_name.clone(),
        path: raid_path,
        symlink_path: raid_device.to_path_buf(),
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
        let output = examine().context("Failed to examine RAID arrays")?;
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
    let new_raid_array = status::RaidArray {
        device_paths: raid_details.devices,
        partition_type: raid_details.partition_type,
        name: raid_details.name.clone(),
        level: raid_details.level,
        status: RaidArrayStatus::Created,
        array_size: raid_details.size,
        ty: RaidType::Software,
        path: raid_details.path,
        raid_symlink_path: raid_details.symlink_path,
        uuid: Uuid::parse_str(&raid_details.uuid).unwrap(),
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

fn get_disk_for_partition(partition: &Path) -> Result<PathBuf, Error> {
    let partition_block_device_list =
        lsblk::run(partition).context("Failed to get partition metadata")?;
    if partition_block_device_list.len() != 1 {
        bail!(
            "Failed to get disk for partition: {:?}, unexpected number of results returned",
            partition
        );
    }

    let parent_kernel_name = &partition_block_device_list[0]
        .parent_kernel_name
        .as_ref()
        .context(format!(
            "Failed to get disk for partition: {:?}, pk_name not found",
            partition
        ))?;

    Ok(PathBuf::from(parent_kernel_name))
}

pub(super) fn stop_pre_existing_raid_arrays(host_config: &HostConfiguration) -> Result<(), Error> {
    if !mdstat_present(Path::new("/proc/mdstat"))? {
        // No pre-existing RAID arrays. Nothing to do.
        return Ok(());
    }

    // Check if mdadm is present, we need it to stop RAID arrays.
    check_if_mdadm_present().context(
        "Failed to clean up pre-existing RAID arrays. Mdadm is required for RAID operations",
    )?;

    let mdadm_detail_output = detail().context("Failed to get existing RAID details")?;

    let mdadm_detail_output_str = String::from_utf8(mdadm_detail_output.stdout)
        .context("Failed to convert mdadm output to string")?;

    if mdadm_detail_output_str.is_empty() {
        return Ok(());
    }

    let parsed_mdadm_detail =
        mdadm_detail_to_struct(&mdadm_detail_output_str).context("Failed to parse mdadm detail")?;

    let trident_disks =
        get_trident_disks(host_config).context("Failed to get disks defined in Trident")?;

    for raid_array in parsed_mdadm_detail {
        let raid_symlink = raid_array
            .raid_path
            .clone()
            .canonicalize()
            .context("Failed to get existing RAID symlink")?;

        let mut raid_disks = HashSet::new();

        for device in &raid_array.devices {
            let disk = get_disk_for_partition(&PathBuf::from(device)).with_context(|| {
                format!(
                    "Failed to get disk for partition in an existing RAID: {:?}",
                    raid_array.raid_path
                )
            })?;

            raid_disks.insert(disk);
        }
        if can_stop_pre_existing_raid(&raid_array.raid_path, &raid_disks, &trident_disks)? {
            unmount_and_stop(&raid_symlink)?;
        }
    }

    Ok(())
}

fn get_trident_disks(host_config: &HostConfiguration) -> Result<HashSet<PathBuf>, Error> {
    host_config
        .storage
        .disks
        .iter()
        .map(|disk| {
            disk.device
                .canonicalize()
                .with_context(|| format!("failed to get canonicalized path for disk: {}", disk.id))
        })
        .collect()
}

fn can_stop_pre_existing_raid(
    raid_name: &Path,
    raid_disks: &HashSet<PathBuf>,
    trident_disks: &HashSet<PathBuf>,
) -> Result<bool, Error> {
    let symmetric_diff: HashSet<_> = raid_disks
        .symmetric_difference(trident_disks)
        .cloned()
        .collect();

    if raid_disks.is_disjoint(trident_disks) {
        // RAID array does not have any of its underlying disks mentioned in HostConfig, we should not touch it
        Ok(false)
    } else if symmetric_diff.is_empty() || raid_disks.is_subset(trident_disks) {
        // RAID array's underlying disks are all part of HostConfig, we can unmount and stop the RAID
        return Ok(true);
    } else {
        // RAID array has underlying disks that are not part of HostConfig, we cannot touch it, abort
        bail!(
            "RAID array '{:?}' has underlying disks that are not part of Trident configuration. RAID disks: {:?}, Trident disks: {:?}",
            raid_name, raid_disks, trident_disks
        );
    }
}

fn mdstat_present(mdstat_path: &Path) -> Result<bool, Error> {
    // mdstat file is present only if there is any RAID on the system
    if !mdstat_path.exists() {
        return Ok(false);
    }
    let mdstat_contents =
        osutils::files::read_file_trim(&mdstat_path).context("Failed to read mdstat file")?;

    Ok(mdstat_contents.contains("active raid"))
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

fn mdadm_detail_to_struct(mdadm_output: &str) -> Result<Vec<MdadmDetail>, Error> {
    let mut mdadm_details = Vec::new();

    let array_regex = Regex::new(r"ARRAY\s+(/dev/md\S+)").unwrap();
    let level_regex = Regex::new(r"(?:^|\s)level=(\w+)").unwrap();
    let uuid_regex = Regex::new(r"(?:^|\s)UUID=([\da-zA-Z:]+)").unwrap();
    let devices_regex = Regex::new(r"(?:^|\s)devices=([^=]+)").unwrap();

    let mut current_mdadm_detail = MdadmDetail::default();

    for line in mdadm_output.lines() {
        if let Some(captures) = array_regex.captures(line) {
            current_mdadm_detail.raid_path = PathBuf::from(captures[1].to_string());
        }
        if let Some(captures) = level_regex.captures(line) {
            current_mdadm_detail.level = captures[1].to_string();
        }
        if let Some(captures) = uuid_regex.captures(line) {
            current_mdadm_detail.uuid = captures[1].to_string();
        }
        if let Some(captures) = devices_regex.captures(line) {
            current_mdadm_detail.devices = captures[1]
                .to_string()
                .split(',')
                .map(PathBuf::from)
                .collect();

            mdadm_details.push(current_mdadm_detail.clone());
            current_mdadm_detail = MdadmDetail::default();
        }
    }

    Ok(mdadm_details)
}

pub(super) fn unmount_and_stop(raid_path: &Path) -> Result<(), Error> {
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
    stop(raid_path).context("Failed to stop RAID array")?;

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
        wait_for_raid_resync(&host_status.storage.raid_arrays)?;
        udevadm::trigger().context("Udev failed while scanning for new devices")?;

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
            let non_idle_devices: Vec<_> = raid_devices
                .iter()
                .filter(|(_, sync_status)| *sync_status != "idle")
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

pub(super) fn create_sw_raid_array(
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
    use std::io::Write;

    use indoc::indoc;
    use maplit::btreemap;
    use tempfile::tempdir;

    use trident_api::{
        config::PartitionType,
        status::{
            BlockDeviceContents, Disk as DiskStatus, Partition as PartitionStatus, RaidArray,
            ReconcileState, Storage as StorageStatus,
        },
    };

    use super::*;

    #[test]
    fn test_get_device_paths() {
        let host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: StorageStatus {
                disks: btreemap! {
                    "os".into() => DiskStatus {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            PartitionStatus {
                                id: "boot".to_string(),
                                path: PathBuf::from("/dev/sda1"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000")
                                    .unwrap(),
                            },
                            PartitionStatus {
                                id: "root".to_string(),
                                path: PathBuf::from("/dev/sda2"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Root,
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000")
                                    .unwrap(),
                            },
                            PartitionStatus {
                                id: "home".to_string(),
                                path: PathBuf::from("/dev/sda3"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::LinuxGeneric,
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000")
                                    .unwrap(),
                            },
                        ],
                    },
                },
                ..Default::default()
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
                partition_type: PartitionType::LinuxGeneric,
                name: "raid1".to_string(),
                level: RaidLevel::Raid1,
                status: RaidArrayStatus::Created,
                array_size: 12345,
                ty: RaidType::Software,
                path: PathBuf::from("/dev/md/some_raid"),
                raid_symlink_path: PathBuf::from("/dev/md127"),
                uuid: Uuid::parse_str("5ddca66e-2a11-4b5c-ab97-5c5158ab10b8").unwrap(),
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
    fn test_mdadm_detail_to_struct() {
        let mdadm_detail_output = indoc!(
            r#"
        ARRAY /dev/md/my-raid2 level=raid1 num-devices=2 metadata=1.0 name=localhost:my-raid2 UUID=6245349d:505a367b:6ceba75f:7f55c158
            devices=/dev/sda8,/dev/sda9
        ARRAY /dev/md/my-raid level=raid1 num-devices=2 metadata=1.0 name=localhost:my-raid UUID=ea381b70:20b2ab81:602edecb:cf6f2032
            devices=/dev/sda6,/dev/sda7
        "#
        );
        let details =
            mdadm_detail_to_struct(mdadm_detail_output).expect("Failed to parse mdadm detail");
        let expected_details: Vec<MdadmDetail> = [
            MdadmDetail {
                raid_path: PathBuf::from("/dev/md/my-raid2"),
                level: "raid1".to_string(),
                uuid: "6245349d:505a367b:6ceba75f:7f55c158".to_string(),
                devices: ["/dev/sda8".into(), "/dev/sda9".into()].into(),
            },
            MdadmDetail {
                raid_path: PathBuf::from("/dev/md/my-raid"),
                level: "raid1".to_string(),
                uuid: "ea381b70:20b2ab81:602edecb:cf6f2032".to_string(),
                devices: ["/dev/sda6".into(), "/dev/sda7".into()].into(),
            },
        ]
        .to_vec();

        assert_eq!(details, expected_details);

        // different raid name format
        let mdadm_detail_output = indoc!(
            r#"
        ARRAY /dev/md126 level=raid1 num-devices=2 metadata=1.0 name=localhost:my-raid2 UUID=602edecb:505a367b:6ceba75f:602edecb
            devices=/dev/sda8,/dev/sda9
        ARRAY /dev/md127 level=raid1 num-devices=2 metadata=1.0 name=localhost:my-raid UUID=as381b70:20b2ab81:602edecb:cf6f20as
            devices=/dev/sda6,/dev/sda7
        "#
        );
        let details =
            mdadm_detail_to_struct(mdadm_detail_output).expect("Failed to parse mdadm detail");
        let expected_details: Vec<MdadmDetail> = [
            MdadmDetail {
                raid_path: PathBuf::from("/dev/md126"),
                level: "raid1".to_string(),
                uuid: "602edecb:505a367b:6ceba75f:602edecb".to_string(),
                devices: ["/dev/sda8".into(), "/dev/sda9".into()].into(),
            },
            MdadmDetail {
                raid_path: PathBuf::from("/dev/md127"),
                level: "raid1".to_string(),
                uuid: "as381b70:20b2ab81:602edecb:cf6f20as".to_string(),
                devices: ["/dev/sda6".into(), "/dev/sda7".into()].into(),
            },
        ]
        .to_vec();

        assert_eq!(details, expected_details);

        // empty
        let mdadm_detail_output = r#""#;
        let details =
            mdadm_detail_to_struct(mdadm_detail_output).expect("Failed to parse mdadm detail");
        let expected_details: Vec<MdadmDetail> = [].to_vec();

        assert_eq!(details, expected_details);
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

    #[test]
    fn test_can_stpp_pre_existing_raid() -> Result<(), Error> {
        let raid_name = PathBuf::from("my-raid");
        let raid_disks: HashSet<PathBuf> = ["/dev/sda".into(), "/dev/sdb".into()].into();
        let trident_disks: HashSet<PathBuf> = ["/dev/sda".into(), "/dev/sdb".into()].into();
        let trident_disks2: HashSet<PathBuf> = ["/dev/sdb".into(), "/dev/sdc".into()].into();
        let trident_disks3: HashSet<PathBuf> = ["/dev/sdc".into(), "/dev/sdd".into()].into();
        let trident_disks4: HashSet<PathBuf> =
            ["/dev/sda".into(), "/dev/sdb".into(), "/dev/sdc".into()].into();

        // No overlapping disks, should not touch
        let overlap = can_stop_pre_existing_raid(&raid_name, &raid_disks, &trident_disks3)?;
        assert!(!overlap);

        // Fully overlapping disks, should stop
        let overlap = can_stop_pre_existing_raid(&raid_name, &raid_disks, &trident_disks)?;
        assert!(overlap);

        // Partially overlapping disks, cannot touch, error.
        let overlap = can_stop_pre_existing_raid(&raid_name, &raid_disks, &trident_disks2);
        assert!(overlap.is_err());

        // Trident disks are a superset of RAID disks, we can stop
        let overlap = can_stop_pre_existing_raid(&raid_name, &raid_disks, &trident_disks4)?;
        assert!(overlap);

        Ok(())
    }

    #[test]
    fn test_mdstat_present() {
        let is_mdstat_present = mdstat_present(Path::new("non-existing-path"))
            .expect("Failed to check if mdstat is present");

        assert!(!is_mdstat_present);

        let temp_dir = tempdir().expect("Failed to create temporary directory");
        let mdstat_path = temp_dir.path().join("mdstat");

        let mut mdstat_file =
            std::fs::File::create(&mdstat_path).expect("Failed to create mdstat file");

        let raid_info = indoc!(
            r#"
        Personalities : [raid1]
        md126 : active raid1 sda9[1] sda8[0]
            51136 blocks super 1.0 [2/2] [UU]

        md127 : active raid1 sda7[1] sda6[0]
            1048512 blocks super 1.0 [2/2] [UU]

        unused devices: <none>
        "#
        );

        mdstat_file
            .write_all(raid_info.as_bytes())
            .expect("Failed to write to mdstat file");

        let is_mdstat_present =
            mdstat_present(&mdstat_path).expect("Failed to check if mdstat is present");
        assert!(is_mdstat_present);

        // Clean up the temporary directory
        temp_dir
            .close()
            .expect("Failed to close temporary directory");
    }
}
