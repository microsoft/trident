use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::{debug, info, trace, warn};

use osutils::mountpoint;
use trident_api::{
    config::HostConfiguration,
    constants::ROOT_MOUNT_POINT_PATH,
    error::{ManagementError, ReportError, TridentError},
    status::{AbVolumeSelection, BlockDeviceContents, HostStatus, ServicingType},
    BlockDeviceId,
};

use crate::modules::{self, Module};

use tabfile::DEFAULT_FSTAB_PATH;

mod encryption;
mod filesystem;
pub mod image;
mod partitioning;
pub mod raid;
pub mod tabfile;
mod verity;

#[derive(Default, Debug)]
pub(super) struct StorageModule;
impl Module for StorageModule {
    fn name(&self) -> &'static str {
        "storage"
    }

    fn refresh_host_status(
        &mut self,
        host_status: &mut HostStatus,
        clean_install: bool,
    ) -> Result<(), Error> {
        // Remove block devices that no longer exist.
        let original_block_devices = host_status.storage.block_devices.clone();
        host_status
            .storage
            .block_devices
            .retain(|_id, block_device| block_device.path.exists());

        let removed_block_devices = original_block_devices
            .keys()
            .filter(|id| !host_status.storage.block_devices.contains_key(*id))
            .collect::<Vec<_>>();
        if !removed_block_devices.is_empty() {
            info!(
                "Removed block devices that no longer exist from status: {}",
                removed_block_devices
                    .iter()
                    .map(|id| id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }

        image::refresh_host_status(host_status, clean_install)
            .context("Image submodule failed during refresh_host_status")?;

        Ok(())
    }

    fn validate_host_config(
        &self,
        host_status: &HostStatus,
        host_config: &HostConfiguration,
        planned_servicing_type: ServicingType,
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

        if planned_servicing_type != ServicingType::CleanInstall {
            // TODO: validate that block devices naming is consistent with the current state
            // https://dev.azure.com/mariner-org/ECF/_workitems/edit/7322/

            // If Trident is performing an A/B update, validate that every undeployed image inside
            // HostConfiguration targets either the ESP partition or an A/B volume pair. An invalid HC
            // should be rejected since Trident cannot overwrite the image on a volume that is shared
            // between A and B.
            image::validate_undeployed_images(host_status, host_config)
                .context("Validation of host configuration failed: HC requests update of images that cannot be overwritten")?;
        }

        encryption::validate_host_config(host_config)
            .context("Encryption host configuration validation failed")?;

        Ok(())
    }

    fn select_servicing_type(
        &self,
        host_status: &HostStatus,
        host_config: &HostConfiguration,
    ) -> Option<ServicingType> {
        if image::needs_ab_update(host_status, host_config) {
            return Some(ServicingType::AbUpdate);
        }

        None
    }

    fn provision(
        &mut self,
        _host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        mount_point: &Path,
    ) -> Result<(), Error> {
        if verity::validate_compatibility(host_config, mount_point)? {
            debug!("Verity devices are compatible with the current system");
            verity::create_machine_id(mount_point)?;
        }

        Ok(())
    }

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        _exec_root: &Path,
    ) -> Result<(), Error> {
        verity::configure_device_names(host_status)
            .context("Failed to finalize device names for Verity devices")?;

        generate_fstab(
            host_config,
            host_status,
            Path::new(tabfile::DEFAULT_FSTAB_PATH),
        )?;

        // TODO: update /etc/repart.d directly for the matching disk, derive
        // from where is the root located

        encryption::configure(host_status)
            .context("Encryption submodule failed during configure")?;

        // persist on reboots
        raid::create_raid_config(host_status)
            .context("Failed to create mdadm.conf file after RAID creation")?;

        // update paths for root verity devices in a grub config
        verity::update_root_verity_in_grub_config(
            host_status,
            host_config,
            Path::new(ROOT_MOUNT_POINT_PATH),
        )
        .context("Failed to update GRUB config file after Verity creation")?;

        Ok(())
    }
}

/// Create a tabfile that captures all the desired
/// mountpoints as per Host Configuration
fn generate_fstab(
    host_config: &HostConfiguration,
    host_status: &HostStatus,
    path: &Path,
) -> Result<(), Error> {
    let mut mount_points = host_config.storage.internal_mount_points.clone();
    if !host_config.storage.internal_verity.is_empty() {
        mount_points.push(verity::create_etc_overlay_mount_point());
    }
    let fstab = tabfile::from_mountpoints(host_status, &mount_points)
        .context("Failed to serialize mount point configuration for the target OS")?;

    fstab
        .write(path)
        .context(format!("Failed to write {}", DEFAULT_FSTAB_PATH))?;

    trace!("Wrote '{}', contents: '{:?}'", DEFAULT_FSTAB_PATH, fstab);

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(super) fn initialize_block_devices(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
    mount_point: &Path,
) -> Result<(), TridentError> {
    if mount_point.exists()
        && mountpoint::check_is_mountpoint(mount_point)
            .structured(ManagementError::MountPointCheck)?
    {
        debug!("Unmounting volumes from earlier runs of Trident");
        if let Err(e) = osutils::mount::umount(mount_point, true) {
            warn!(
                "Attempt to unmount '{}' returned error: {e}",
                mount_point.display(),
            );
        }
    }

    trace!(
        "Mount points: {:?}",
        host_config.storage.internal_mount_points
    );

    if host_status.servicing_type == Some(ServicingType::CleanInstall) {
        debug!("Initializing block devices");
        // Stop verity before RAID, as verity can sit on top of RAID
        verity::stop_pre_existing_verity_devices(host_config)
            .structured(ManagementError::CleanupVerity)?;
        raid::stop_pre_existing_raid_arrays(host_config)
            .structured(ManagementError::CleanupRaid)?;
        partitioning::create_partitions(host_status, host_config)
            .structured(ManagementError::CreatePartitions)?;
        raid::create_sw_raid(host_status, host_config).structured(ManagementError::CreateRaid)?;
        encryption::provision(host_status, host_config)
            .structured(ManagementError::CreateEncryptedVolumes)?;
    }

    image::provision(host_status, host_config).structured(ManagementError::DeployImages)?;
    filesystem::create_filesystems(host_status).structured(ManagementError::CreateFilesystems)?;

    // Assumes that images are already in place (data and hash), so that it can
    // assemble the verity devices.
    verity::setup_verity_devices(host_config, host_status)
        .structured(ManagementError::CreateVerity)?;

    Ok(())
}

/// Get the canonicalized paths of all disks in a Host Configuration
fn get_hostconfig_disk_paths(host_config: &HostConfiguration) -> Result<Vec<PathBuf>, Error> {
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

pub(super) fn set_host_status_block_device_contents(
    host_status: &mut HostStatus,
    block_device_id: &BlockDeviceId,
    contents: BlockDeviceContents,
) -> Result<(), Error> {
    debug!("Setting block device '{block_device_id}' contents to '{contents:?}'");
    if let Some(disk) = host_status.storage.block_devices.get_mut(block_device_id) {
        disk.contents = contents;
        return Ok(());
    }

    if let Some(ab_update) = &host_status.spec.storage.ab_update {
        if let Some(ab_volume_pair) = ab_update
            .volume_pairs
            .iter()
            .find(|p| &p.id == block_device_id)
        {
            let target_id = match modules::get_ab_update_volume(host_status) {
                Some(AbVolumeSelection::VolumeA) => Some(&ab_volume_pair.volume_a_id),
                Some(AbVolumeSelection::VolumeB) => Some(&ab_volume_pair.volume_b_id),
                None => None,
            };
            if let Some(target_id) = target_id {
                return set_host_status_block_device_contents(
                    host_status,
                    &target_id.clone(),
                    contents,
                );
            }
        }
    }

    anyhow::bail!("No block device with id '{}' found", block_device_id);
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, Permissions},
        os::unix::fs::PermissionsExt,
        str::FromStr,
    };

    use maplit::btreemap;
    use tempfile::NamedTempFile;
    use trident_api::{
        config::{
            self, AbUpdate, AbVolumePair, Disk as DiskConfig, FileSystemType, HostConfiguration,
            ImageFormat, ImageSha256, InternalImage, InternalMountPoint,
            Partition as PartitionConfig, PartitionSize, PartitionType, Raid, RaidLevel,
            SoftwareRaidArray, Storage as StorageConfig,
        },
        constants::ROOT_MOUNT_POINT_PATH,
        status::{BlockDeviceInfo, ServicingState, Storage},
    };

    use super::*;

    fn get_host_status() -> HostStatus {
        HostStatus {
            servicing_state: ServicingState::NotProvisioned,
            ..Default::default()
        }
    }

    // Create a temporary recovery key file. The file will be deleted once
    // the object returned is out of scope and dropped.
    pub fn get_recovery_key_file() -> NamedTempFile {
        let recovery_key_file: NamedTempFile = NamedTempFile::new().unwrap();
        let recovery_key_path: PathBuf = recovery_key_file.path().to_owned();
        fs::set_permissions(&recovery_key_path, Permissions::from_mode(0o600)).unwrap();
        encryption::generate_recovery_key_file(&recovery_key_path).unwrap();
        recovery_key_file
    }

    fn get_host_config(recovery_key_file: &tempfile::NamedTempFile) -> HostConfiguration {
        HostConfiguration {
            storage: StorageConfig {
                disks: vec![
                    DiskConfig {
                        id: "disk1".to_owned(),
                        device: ROOT_MOUNT_POINT_PATH.into(),
                        ..Default::default()
                    },
                    DiskConfig {
                        id: "disk2".to_owned(),
                        device: "/tmp".into(),
                        partitions: vec![
                            PartitionConfig {
                                id: "part1".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("1M").unwrap(),
                            },
                            PartitionConfig {
                                id: "part2".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            PartitionConfig {
                                id: "part3".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            PartitionConfig {
                                id: "part4".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            PartitionConfig {
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
                        devices: vec!["part3".to_owned(), "part4".to_owned()],
                    }],
                },
                internal_verity: vec![],
                internal_mount_points: vec![InternalMountPoint {
                    filesystem: FileSystemType::Ext4,
                    options: vec![],
                    target_id: "part1".to_owned(),
                    path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                }],
                internal_images: vec![InternalImage {
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
                        device_id: "part5".to_owned(),
                    }],
                }),
                ..Default::default()
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
            .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
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
                .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
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
                .validate_host_config(&host_status, &host_config, ServicingType::CleanInstall)
                .unwrap_err()
                .to_string(),
            "Encryption host configuration validation failed"
        );
    }

    /// Validates logic for setting block device contents
    #[test]
    fn test_set_host_status_block_device_contents() {
        let mut host_status = HostStatus {
            servicing_type: Some(ServicingType::CleanInstall),
            servicing_state: ServicingState::StagingDeployment,
            spec: HostConfiguration {
                storage: config::Storage {
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "osab".to_string(),
                            volume_a_id: "root".to_string(),
                            volume_b_id: "rootb".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "efi".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 900,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "rootb".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 9000,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "data".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 1000,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ab_active_volume: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            host_status
                .storage
                .block_devices
                .get("os")
                .unwrap()
                .contents,
            BlockDeviceContents::Unknown
        );
        assert_eq!(
            host_status
                .storage
                .block_devices
                .get("root")
                .unwrap()
                .contents,
            BlockDeviceContents::Unknown
        );

        // test for disks
        let contents = BlockDeviceContents::Zeroed;
        set_host_status_block_device_contents(&mut host_status, &"os".to_owned(), contents.clone())
            .unwrap();
        assert_eq!(
            host_status
                .storage
                .block_devices
                .get("os")
                .unwrap()
                .contents,
            contents.clone()
        );

        // test for partitions
        set_host_status_block_device_contents(
            &mut host_status,
            &"efi".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .block_devices
                .get("efi")
                .unwrap()
                .contents,
            contents.clone()
        );

        // test for ab volumes
        set_host_status_block_device_contents(
            &mut host_status,
            &"osab".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .block_devices
                .get("root")
                .unwrap()
                .contents,
            contents.clone()
        );

        host_status.servicing_type = Some(ServicingType::AbUpdate);

        set_host_status_block_device_contents(
            &mut host_status,
            &"osab".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .block_devices
                .get("root")
                .unwrap()
                .contents,
            contents.clone()
        );

        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        set_host_status_block_device_contents(
            &mut host_status,
            &"osab".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .block_devices
                .get("root")
                .unwrap()
                .contents,
            contents.clone()
        );

        // test failure when missing id is provided
        assert_eq!(
            set_host_status_block_device_contents(
                &mut host_status,
                &"foorbar".to_owned(),
                contents.clone()
            )
            .unwrap_err()
            .to_string(),
            "No block device with id 'foorbar' found"
        );
    }

    #[test]
    fn test_generate_fstab() {
        let expected_contents = "/part1 / ext4 defaults 0 1\n";
        let temp_tabfile = tempfile::NamedTempFile::new().unwrap();
        // passing dummy file
        assert_eq!(
            generate_fstab(
                &get_host_config(&temp_tabfile),
                &HostStatus {
                    ..Default::default()
                },
                temp_tabfile.path(),
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Failed to find block device with id part1"
        );

        generate_fstab(
            &get_host_config(&temp_tabfile),
            &HostStatus {
                spec: HostConfiguration {
                    storage: config::Storage {
                        disks: vec![DiskConfig {
                            id: "disk1".into(),
                            partitions: vec![PartitionConfig {
                                id: "part1".into(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(1),
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                storage: Storage {
                    block_devices: btreemap! {
                        "part1".into() => BlockDeviceInfo {
                            path: PathBuf::from("/part1"),
                            size: 1,
                            contents: BlockDeviceContents::Unknown,
                        },
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            temp_tabfile.path(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(temp_tabfile.path()).unwrap(),
            expected_contents,
        );

        // test with verity enabled

        let expected_contents = "/part1 / ext4 defaults 0 1\noverlay /etc overlay lowerdir=/etc,upperdir=/var/lib/trident-overlay/etc/upper,workdir=/var/lib/trident-overlay/etc/work,ro 0 2\n";

        let mut hc = get_host_config(&temp_tabfile);
        hc.storage.internal_verity = vec![config::InternalVerityDevice {
            device_name: "".into(),
            id: "".into(),
            data_target_id: "".into(),
            hash_target_id: "".into(),
        }];

        generate_fstab(
            &hc,
            &HostStatus {
                spec: HostConfiguration {
                    storage: config::Storage {
                        disks: vec![DiskConfig {
                            id: "disk1".into(),
                            partitions: vec![PartitionConfig {
                                id: "part1".into(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(1),
                            }],
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                storage: Storage {
                    block_devices: btreemap! {
                        "part1".into() => BlockDeviceInfo {
                            path: PathBuf::from("/part1"),
                            size: 1,
                            contents: BlockDeviceContents::Unknown,
                        },
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            temp_tabfile.path(),
        )
        .unwrap();

        assert_eq!(
            fs::read_to_string(temp_tabfile.path()).unwrap(),
            expected_contents,
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use osutils::testutils::repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH};
    use trident_api::config::{Disk, Storage};

    #[functional_test]
    fn test_get_hostconfig_disk_paths() {
        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![
                    Disk {
                        id: "disk1".to_owned(),
                        device: "/dev/sda".into(),
                        ..Default::default()
                    },
                    Disk {
                        id: "disk2".to_owned(),
                        device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-3".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let disks = get_hostconfig_disk_paths(&host_config).unwrap();
        assert_eq!(
            disks,
            vec![
                PathBuf::from(OS_DISK_DEVICE_PATH),
                PathBuf::from(TEST_DISK_DEVICE_PATH)
            ]
        );

        // fail on missing disk
        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk1".to_owned(),
                    device: "/dev/sdc".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            get_hostconfig_disk_paths(&host_config)
                .unwrap_err()
                .to_string(),
            "failed to get canonicalized path for disk: disk1"
        );
    }
}
