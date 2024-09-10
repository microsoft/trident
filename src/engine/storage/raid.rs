use anyhow::{bail, Context, Error};
use log::{debug, info, trace, warn};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    thread::sleep,
    time::{Duration, Instant},
};
use strum_macros::{Display, EnumString};
use trident_api::{
    config::{HostConfiguration, PartitionType, RaidLevel, SoftwareRaidArray},
    status::HostStatus,
    BlockDeviceId,
};

use osutils::{block_devices, exe::OutputChecker, mdadm, udevadm};

use crate::engine;

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
    let device_paths =
        get_device_paths(host_status, devices).context("Failed to get device paths")?;

    mdadm::create(&config.device_path(), &config.level, device_paths)
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
            engine::get_block_device_path(host_status, device_id).context(format!(
                "Failed to get block device path for '{}'",
                device_id
            ))
        })
        .collect()
}

#[tracing::instrument(skip_all)]
pub(super) fn configure(host_status: &HostStatus) -> Result<(), Error> {
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
            block_devices::get_disk_for_partition(device).with_context(|| {
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

#[tracing::instrument(skip_all)]
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

#[tracing::instrument(name = "raid_creation", skip_all)]
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

        if let Some(sync_timeout) = host_config.storage.raid.sync_timeout {
            wait_for_raid_sync(host_status, sync_timeout)
                .context("Failed to wait for RAID sync")?;
        }

        udevadm::trigger().context("Udev failed while scanning for new devices")?;
    }

    Ok(())
}

pub fn create_sw_raid_array(
    host_status: &mut HostStatus,
    raid_array: &SoftwareRaidArray,
) -> Result<(), Error> {
    create(raid_array.clone(), host_status)
        .context(format!("Failed to create RAID array '{}'", raid_array.name))?;

    let raid_device = raid_array.device_path();

    // Wait for symlink to appear. Kernel creates /dev/mdXX and udev crates symlink (raid_device)
    udevadm::wait(&raid_device).context(format!(
        "Failed waiting for RAID device '{}' to appear",
        raid_device.display()
    ))?;

    let _raid_device_resolved_path = raid_device
        .canonicalize()
        .context("Unable to find RAID device resolved path after RAID creation")?;

    Ok(())
}

/// This function checks if the sync_action file has a value "idle" for all RAID
/// devices, and waits for all RAID devices to sync within the given timeout. If
/// the RAID arrays have not finished their sync within the timeout, an error is
/// returned.
fn wait_for_raid_sync(host_status: &mut HostStatus, sync_timeout: u64) -> Result<(), Error> {
    info!("Waiting for RAID arrays to sync");

    let start_time = Instant::now();
    let sync_timeout_secs = std::time::Duration::from_secs(sync_timeout);

    // Exponential backoff for sleep duration
    let mut sleep_duration = Duration::from_secs(5);
    let max_sleep_duration = Duration::from_secs(60);

    let raid_device_ids: Vec<String> = host_status
        .spec
        .storage
        .raid
        .software
        .iter()
        .map(|raid_array| raid_array.id.clone())
        .collect();

    let mut raid_devices: Vec<(String, String)> = get_device_paths(host_status, &raid_device_ids)
        .context("Failed to get RAID device paths")?
        .iter()
        .map(|raid_path| {
            let symlink_path = raid_path.canonicalize().context(format!(
                "Failed to get RAID device resolved path for RAID array '{}'",
                raid_path.display()
            ))?;
            let device_name = get_raid_device_name(&symlink_path)?;
            Ok((device_name, "".to_string()))
        })
        .collect::<Result<Vec<(String, String)>, Error>>()?;

    loop {
        // Check if any RAID devices are not idle
        raid_devices
            .iter_mut()
            .filter(|(_, sync_status)| *sync_status != "idle")
            .for_each(|(raid_device, sync_status)| {
                if let Ok(sync_action) = osutils::files::read_file_trim(&PathBuf::from(format!(
                    "/sys/devices/virtual/block/{}/md/sync_action",
                    raid_device
                ))) {
                    sync_status.clone_from(&sync_action);
                } else {
                    warn!("Failed to read RAID device sync status");
                }
            });

        if raid_devices
            .iter()
            .all(|(_, sync_status)| sync_status == "idle")
        {
            break;
        } else if start_time.elapsed() >= sync_timeout_secs {
            bail!("Timed out waiting for RAID arrays to sync!");
        }

        if start_time.elapsed() + sleep_duration >= sync_timeout_secs {
            sleep_duration = sync_timeout_secs - start_time.elapsed();
        } else if sleep_duration < max_sleep_duration {
            sleep_duration *= 2;
        } else {
            sleep_duration = max_sleep_duration;
        }

        debug!(
            "Still waiting for RAID arrays to sync. Checking again after {:?} seconds",
            sleep_duration.as_secs()
        );
        sleep(sleep_duration);
    }
    info!(
        "All RAID arrays have finished syncing! Total wait time: {:?} seconds",
        start_time.elapsed().as_secs()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use maplit::btreemap;

    use trident_api::{
        config::{Disk, Partition, PartitionSize, PartitionType, Storage},
        status::{ServicingState, ServicingType},
    };

    use super::*;

    #[test]
    fn test_get_device_paths() {
        let host_status = HostStatus {
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                size: 1000.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root".to_string(),
                                size: 1000.into(),
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
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "boot".into() => PathBuf::from("/dev/sda1"),
                "root".into() => PathBuf::from("/dev/sda2"),
                "home".into() => PathBuf::from("/dev/sda3"),
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

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {

    use crate::engine::storage;

    use super::*;
    use pytest_gen::functional_test;

    use const_format::formatcp;
    use osutils::testutils::{raid, repart::TEST_DISK_DEVICE_PATH};
    use std::path::PathBuf;
    use std::str::FromStr;
    use trident_api::config::{
        self, Disk, HostConfiguration, Partition, PartitionSize, PartitionType, RaidLevel,
        SoftwareRaidArray, Storage,
    };

    const DEVICE_ONE: &str = formatcp!("{TEST_DISK_DEVICE_PATH}1");
    const DEVICE_TWO: &str = formatcp!("{TEST_DISK_DEVICE_PATH}2");
    const NON_EXISTENT_DEVICE: &str = "/dev/non-existent-path";
    const RAID_PATH: &str = "/dev/md/some-raid";

    fn raid_cleanup_and_create_partitions() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "disk".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "root-a".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
                    raid: config::Raid {
                        software: vec![SoftwareRaidArray {
                            id: "raid_array".into(),
                            name: "md0".into(),
                            devices: vec!["root-a".to_string(), "root-b".to_string()],
                            level: RaidLevel::Raid1,
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let spec = &host_status.spec.clone();
        let err = storage::raid::stop_pre_existing_raid_arrays(spec);
        assert!(err.is_ok());

        let err = storage::partitioning::create_partitions(&mut host_status, spec);
        assert!(err.is_ok());
    }

    #[functional_test]
    fn test_raid_create_success() {
        raid_cleanup_and_create_partitions();
        let raid_path = PathBuf::from(RAID_PATH);
        let devices = [PathBuf::from(DEVICE_ONE), PathBuf::from(DEVICE_TWO)].to_vec();

        mdadm::create(&raid_path, &RaidLevel::Raid1, devices.clone()).unwrap();
        raid::verify_raid_creation(&raid_path, devices);

        raid::stop_if_exists(&raid_path);
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_raid_create_failure() {
        raid_cleanup_and_create_partitions();
        let raid_path = PathBuf::from(RAID_PATH);

        let devices = [
            PathBuf::from(DEVICE_ONE),
            PathBuf::from(NON_EXISTENT_DEVICE),
        ]
        .to_vec();

        assert_eq!(
            mdadm::create(&raid_path, &RaidLevel::Raid1, devices,)
                .unwrap_err()
                .to_string(),
            "Failed to run mdadm create"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_raid_create_one_partition_failure() {
        raid_cleanup_and_create_partitions();
        let raid_path = PathBuf::from(RAID_PATH);

        let devices = [PathBuf::from(DEVICE_ONE)].to_vec();

        assert_eq!(
            mdadm::create(&raid_path, &RaidLevel::Raid1, devices.clone())
                .unwrap_err()
                .to_string(),
            "Failed to run mdadm create"
        );
    }

    #[functional_test]
    fn test_raid_creation_without_sync_timeout() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "disk".to_string(),
                        device: PathBuf::from("/dev/sdb"),
                        partitions: vec![
                            Partition {
                                id: "root-a".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
                    raid: config::Raid {
                        software: vec![SoftwareRaidArray {
                            id: "raid_array".into(),
                            name: "md0".into(),
                            devices: vec!["root-a".to_string(), "root-b".to_string()],
                            level: RaidLevel::Raid1,
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let spec = &host_status.spec.clone();

        let err = stop_pre_existing_raid_arrays(spec);
        assert!(err.is_ok());

        let err = storage::partitioning::create_partitions(&mut host_status, spec);
        assert!(err.is_ok());

        create_sw_raid(&mut host_status, spec).unwrap();

        // cleanup the RAID array
        let err = stop_pre_existing_raid_arrays(spec);
        assert!(err.is_ok());
    }

    #[functional_test]
    fn test_raid_creation_with_sync_timeout() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "disk".to_string(),
                        device: PathBuf::from("/dev/sdb"),
                        partitions: vec![
                            Partition {
                                id: "root-a".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
                    raid: config::Raid {
                        software: vec![SoftwareRaidArray {
                            id: "raid_array".into(),
                            name: "md0".into(),
                            devices: vec!["root-a".to_string(), "root-b".to_string()],
                            level: RaidLevel::Raid1,
                        }],
                        sync_timeout: Some(180),
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let spec = &host_status.spec.clone();
        let err = stop_pre_existing_raid_arrays(spec);
        assert!(err.is_ok());

        let err = storage::partitioning::create_partitions(&mut host_status, spec);
        assert!(err.is_ok());

        create_sw_raid(&mut host_status, spec).unwrap();

        // cleanup the RAID array
        let err = stop_pre_existing_raid_arrays(spec);
        assert!(err.is_ok());
    }

    #[functional_test(negative = true)]
    fn test_raid_creation_with_sync_timeout_failing() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "disk".to_string(),
                        device: PathBuf::from("/dev/sdb"),
                        partitions: vec![
                            Partition {
                                id: "root-a".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
                    raid: config::Raid {
                        software: vec![SoftwareRaidArray {
                            id: "raid_array".into(),
                            name: "md0".into(),
                            devices: vec!["root-a".to_string(), "root-b".to_string()],
                            level: RaidLevel::Raid1,
                        }],
                        sync_timeout: Some(0),
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let spec = &host_status.spec.clone();
        let err = stop_pre_existing_raid_arrays(spec);
        assert!(err.is_ok());

        let err = storage::partitioning::create_partitions(&mut host_status, spec);
        assert!(err.is_ok());

        assert_eq!(
            create_sw_raid(&mut host_status, spec)
                .unwrap_err()
                .to_string(),
            "Failed to wait for RAID sync"
        );

        // cleanup the RAID array in case it was created
        let err = stop_pre_existing_raid_arrays(spec);
        assert!(err.is_ok());
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_raid_creation_failure_unequal_partitions() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "foo".into(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "boot1".into(),
                                size: PartitionSize::from_str("2G").unwrap(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root1".into(),
                                size: PartitionSize::from_str("8G").unwrap(),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "root2".into(),
                                size: PartitionSize::from_str("4G").unwrap(),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Create a raid array
        let raid_array = SoftwareRaidArray {
            id: "raid_array".into(),
            name: "md0".into(),
            devices: vec!["root1".to_string(), "root2".to_string()],
            level: RaidLevel::Raid1,
        };

        let spec = &host_status.spec.clone();
        let err = stop_pre_existing_raid_arrays(spec);
        assert!(err.is_ok());

        let err = storage::partitioning::create_partitions(&mut host_status, spec);
        assert!(err.is_ok());

        assert_eq!(
            create_sw_raid_array(&mut host_status, &raid_array)
                .unwrap_err()
                .to_string(),
            "Failed to create RAID array 'md0'"
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_raid_create_add_fail_remove() {
        raid_cleanup_and_create_partitions();
        let raid_path = PathBuf::from(RAID_PATH);
        let devices = [PathBuf::from(DEVICE_ONE), PathBuf::from(DEVICE_TWO)].to_vec();

        // Create RAID array
        mdadm::create(&raid_path, &RaidLevel::Raid1, devices.clone()).unwrap();
        raid::verify_raid_creation(&raid_path, devices);

        // Fail the device in the RAID array
        mdadm::fail(raid_path.clone(), PathBuf::from(DEVICE_TWO)).unwrap();

        // Remove the failed device from the RAID array
        mdadm::remove(raid_path.clone(), PathBuf::from(DEVICE_TWO)).unwrap();

        // Add the failed device back to the RAID array
        mdadm::add(raid_path.clone(), PathBuf::from(DEVICE_TWO)).unwrap();

        raid::stop_if_exists(&raid_path);
    }

    #[functional_test(feature = "helpers")]
    fn test_raid_add_fail_remove_rerun() {
        raid_cleanup_and_create_partitions();
        let raid_path = PathBuf::from(RAID_PATH);
        let devices = [PathBuf::from(DEVICE_ONE), PathBuf::from(DEVICE_TWO)].to_vec();

        // Create RAID array
        mdadm::create(&raid_path, &RaidLevel::Raid1, devices.clone()).unwrap();
        raid::verify_raid_creation(&raid_path, devices);

        // Fail the device in the RAID array
        mdadm::fail(raid_path.clone(), PathBuf::from(DEVICE_TWO)).unwrap();
        // Re-fail the device in the RAID array
        mdadm::fail(raid_path.clone(), PathBuf::from(DEVICE_TWO)).unwrap();

        // Remove the failed device from the RAID array
        mdadm::remove(raid_path.clone(), PathBuf::from(DEVICE_TWO)).unwrap();

        // Re-remove the failed device from the RAID array
        assert_eq!(
            mdadm::remove(raid_path.clone(), PathBuf::from(DEVICE_TWO))
                .unwrap_err()
                .to_string(),
            "Failed to run mdadm remove device"
        );

        // Add the failed device back to the RAID array
        mdadm::add(raid_path.clone(), PathBuf::from(DEVICE_TWO)).unwrap();

        // Re-add the failed device back to the RAID array
        assert_eq!(
            mdadm::add(raid_path.clone(), PathBuf::from(DEVICE_TWO))
                .unwrap_err()
                .to_string(),
            "Failed to run mdadm add device"
        );

        raid::stop_if_exists(&raid_path);
    }
}
