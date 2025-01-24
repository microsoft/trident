use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use log::{debug, error, info, trace, warn};

use osutils::{e2fsck, hashing_reader::compute_file_hash, lsblk};
use trident_api::{
    config::{FileSystemType, HostConfiguration, HostConfigurationDynamicValidationError},
    constants::{internal_params::PRE_REBOOT_CHECKS, ROOT_MOUNT_POINT_PATH},
    error::{
        InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt,
        UnsupportedConfigurationError,
    },
    status::{HostStatus, ServicingType},
    BlockDeviceId,
};

use crate::engine::Subsystem;

mod encryption;
mod filesystem;
pub mod image;
pub mod osimage;
pub mod partitioning;
pub mod raid;
pub mod rebuild;
pub mod tabfile;
pub mod verity;

use tabfile::DEFAULT_FSTAB_PATH;

use super::EngineContext;

const IMAGE_SUBSYSTEM_NAME: &str = "image";
const ENCRYPTION_SUBSYSTEM_NAME: &str = "encryption";
const OSIMAGE_SUBSYSTEM_NAME: &str = "osimage";

#[derive(Default, Debug)]
pub(super) struct StorageSubsystem;
impl Subsystem for StorageSubsystem {
    fn name(&self) -> &'static str {
        "storage"
    }

    fn validate_host_config(
        &self,
        ctx: &EngineContext,
        host_config: &HostConfiguration,
    ) -> Result<(), TridentError> {
        if ctx.servicing_type != ServicingType::CleanInstall {
            // Ensure that relevant portions of the host configuration have not changed.
            if ctx.spec.storage.disks != host_config.storage.disks
                || ctx.spec.storage.raid != host_config.storage.raid
                || ctx.spec.storage.encryption != host_config.storage.encryption
                || ctx.spec.storage.ab_update != host_config.storage.ab_update
            {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::StorageConfigurationChanged,
                )));
            }

            // Ensure that all partitions still exist.
            let removed_block_devices: Vec<_> = ctx
                .partition_paths
                .iter()
                .filter(|&(_id, path)| !path.exists())
                .collect();
            for (id, path) in &removed_block_devices {
                error!(
                    "Partition '{id}' formerly with path '{}' no longer exists",
                    path.display()
                );
            }
            if !removed_block_devices.is_empty() {
                return Err(TridentError::new(
                    UnsupportedConfigurationError::PartitionsRemoved {
                        partition_ids: removed_block_devices
                            .into_iter()
                            .map(|(id, _path)| id.clone())
                            .collect(),
                    },
                ));
            }
        }

        // Ensure any two disks point to different devices. This requires canonicalizing the device
        // paths, which can only be done on the target system.
        let mut device_paths = HashMap::<PathBuf, BlockDeviceId>::new();
        for disk in &host_config.storage.disks {
            let device_path = disk
                .device
                .canonicalize()
                .structured(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::InvalidDiskPath {
                        disk_id: disk.id.clone(),
                        disk_path: disk.device.to_string_lossy().to_string(),
                    },
                ))?;

            if !device_path.starts_with("/dev") {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::InvalidDiskBlockDevicePath {
                        name: disk.id.clone(),
                        device: device_path.display().to_string(),
                    },
                )));
            }

            if let Some(existing_disk_id) =
                device_paths.insert(device_path.clone(), disk.id.clone())
            {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::DiskDefinitionsReferToSameDevice {
                        disk1: existing_disk_id,
                        disk2: disk.id.clone(),
                        device: device_path.display().to_string(),
                    },
                )));
            }

            // If we are adopting partitions on a disk, ensure that the disk is GPT partitioned.
            if !disk.adopted_partitions.is_empty() {
                let disk_data =
                    lsblk::get(device_path.as_path()).structured(InvalidInputError::from(
                        HostConfigurationDynamicValidationError::GetBlockDeviceInfoForDisk {
                            disk_id: disk.id.clone(),
                        },
                    ))?;

                match disk_data.partition_table_type {
                    Some(lsblk::PartitionTableType::Gpt) => {}
                    _ => return Err(TridentError::new(InvalidInputError::from(
                        HostConfigurationDynamicValidationError::AdoptPartitionsOnNonGptPartitionedDisk {
                            disk_id: disk.id.clone(),
                        },
                    ))),
                }
            }
        }

        // TODO: validate that block devices naming is consistent with the current state
        // https://dev.azure.com/mariner-org/ECF/_workitems/edit/7322/

        image::validate_host_config(ctx, host_config, ctx.servicing_type).message(format!(
            "Step 'Validate' failed for subsystem '{IMAGE_SUBSYSTEM_NAME}'"
        ))?;

        encryption::validate_host_config(host_config).message(format!(
            "Step 'Validate' failed for subsystem '{ENCRYPTION_SUBSYSTEM_NAME}'"
        ))?;

        osimage::validate_host_config(ctx, host_config).message(format!(
            "Step 'Validate' failed for subsystem '{OSIMAGE_SUBSYSTEM_NAME}'"
        ))?;

        Ok(())
    }

    fn select_servicing_type(
        &self,
        ctx: &EngineContext,
    ) -> Result<Option<ServicingType>, TridentError> {
        // If needs_ab_update() returns true, A/B update is required.
        if image::needs_ab_update(ctx) {
            return Ok(Some(ServicingType::AbUpdate));
        }

        Ok(Some(ServicingType::NoActiveServicing))
    }

    fn provision(&mut self, ctx: &EngineContext, mount_point: &Path) -> Result<(), TridentError> {
        if verity::validate_verity_compatibility(ctx, mount_point).structured(
            InvalidInputError::from(
                HostConfigurationDynamicValidationError::DmVerityMisconfiguration,
            ),
        )? {
            debug!("Verity devices are compatible with the current system");
            if ctx.servicing_type == ServicingType::CleanInstall {
                verity::create_machine_id(mount_point)
                    .structured(ServicingError::CreateMachineId)?;
            }
        }

        Ok(())
    }

    #[tracing::instrument(name = "storage_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext, _exec_root: &Path) -> Result<(), TridentError> {
        generate_fstab(ctx, Path::new(tabfile::DEFAULT_FSTAB_PATH)).structured(
            ServicingError::GenerateFstab {
                fstab_path: tabfile::DEFAULT_FSTAB_PATH.to_string(),
            },
        )?;

        // TODO: Update /etc/repart.d directly for the matching disk, derive it from where the root
        // is located

        encryption::configure(ctx).message(format!(
            "Step 'Configure' failed for subsystem '{ENCRYPTION_SUBSYSTEM_NAME}'"
        ))?;

        // Persist on reboots
        raid::configure(ctx).structured(ServicingError::CreateMdadmConf)?;

        // Update paths for root verity devices in GRUB configs
        verity::configure(ctx, Path::new(ROOT_MOUNT_POINT_PATH))
            .structured(ServicingError::UpdateGrubConfigsAfterVerityCreation)?;

        Ok(())
    }
}

/// Create a tabfile that captures all the desired as per the spec in engine context.
fn generate_fstab(ctx: &EngineContext, path: &Path) -> Result<(), Error> {
    let mut mount_points = ctx.spec.storage.internal_mount_points.clone();
    if ctx.spec.storage.has_verity_device() {
        mount_points.push(verity::create_etc_overlay_mount_point());
    }
    let fstab = tabfile::from_mountpoints(ctx, &mount_points)
        .context("Failed to serialize mount point configuration for the target OS")?;

    fstab
        .write(path)
        .context(format!("Failed to write {}", DEFAULT_FSTAB_PATH))?;

    trace!("Wrote '{}', contents: '{:?}'", DEFAULT_FSTAB_PATH, fstab);

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(super) fn create_block_devices(ctx: &mut EngineContext) -> Result<(), TridentError> {
    trace!("Mount points: {:?}", ctx.spec.storage.internal_mount_points);

    debug!("Initializing block devices");

    // Close verity devices and encrypted volumes before stopping RAID
    // arrays, as both can sit on top of RAID arrays.
    close_pre_existing_devices(ctx).message("Closing pre-existing block devices failed")?;

    partitioning::create_partitions(ctx).structured(ServicingError::CreatePartitions)?;
    raid::create_sw_raid(ctx, &ctx.spec).structured(ServicingError::CreateRaid)?;
    encryption::provision(ctx, &ctx.spec).message(format!(
        "Step 'Provision' failed for subsystem '{ENCRYPTION_SUBSYSTEM_NAME}'"
    ))?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(super) fn close_pre_existing_devices(ctx: &EngineContext) -> Result<(), TridentError> {
    debug!("Closing pre-existing block devices");

    // Close verity devices and encrypted volumes before stopping RAID
    // arrays, as both can sit on top of RAID arrays.
    verity::stop_pre_existing_verity_devices(&ctx.spec)
        .structured(ServicingError::CleanupVerity)?;
    encryption::close_pre_existing_encrypted_volumes(&ctx.spec)
        .structured(ServicingError::CleanupEncryption)?;
    raid::stop_pre_existing_raid_arrays(&ctx.spec).structured(ServicingError::CleanupRaid)?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(super) fn initialize_block_devices(ctx: &EngineContext) -> Result<(), TridentError> {
    trace!("Mount points: {:?}", ctx.spec.storage.internal_mount_points);

    image::provision(ctx, &ctx.spec).message(format!(
        "Step 'Provision' failed for subsystem '{IMAGE_SUBSYSTEM_NAME}'"
    ))?;
    filesystem::create_filesystems(ctx).structured(ServicingError::CreateFilesystems)?;

    // Assumes that images are already in place (data and hash), so that it can
    // assemble the verity devices.
    verity::setup_verity_devices(ctx).structured(ServicingError::CreateVerity)?;

    Ok(())
}

pub(super) fn check_block_devices(host_status: &HostStatus) {
    if !host_status.spec.internal_params.get_flag(PRE_REBOOT_CHECKS) {
        return;
    }

    for (id, path) in &host_status.partition_paths {
        let Ok(canonical) = path.canonicalize() else {
            warn!(
                "Block device '{id}' (path '{}'): No longer exists",
                path.display()
            );
            continue;
        };

        let Ok((length, sha384)) = compute_file_hash(&canonical) else {
            warn!(
                "Block device '{id}' (path '{}' -> '{}'): Failed to compute hash",
                path.display(),
                canonical.display()
            );
            continue;
        };

        let fs_type = host_status
            .spec
            .storage
            .internal_mount_points
            .iter()
            .find(|fs| &fs.target_id == id)
            .map(|fs| fs.filesystem);

        let fsck_status = match fs_type {
            Some(FileSystemType::Ext4) => {
                if let Err(e) = e2fsck::check(&canonical) {
                    format!(", e2fsck failed: {e:?}")
                } else {
                    ", e2fsck OK".to_string()
                }
            }
            _ => "".to_string(),
        };

        info!(
            "Block device '{id}' (path '{}' -> '{}'): Size = {length} bytes, sha384 = {sha384}{fsck_status}",
            path.display(),
            canonical.display(),
        );
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs::{self, Permissions},
        os::unix::fs::PermissionsExt,
        str::FromStr,
    };

    use maplit::btreemap;
    use tempfile::NamedTempFile;

    use trident_api::{
        config::{
            self, Disk as DiskConfig, FileSystemType, HostConfiguration, InternalMountPoint,
            Partition as PartitionConfig, PartitionSize, PartitionType, Raid, RaidLevel,
            SoftwareRaidArray, Storage as StorageConfig,
        },
        constants::ROOT_MOUNT_POINT_PATH,
        error::ErrorKind,
    };

    use osutils::encryption;

    fn get_ctx() -> EngineContext {
        EngineContext {
            servicing_type: ServicingType::CleanInstall,
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
                        device: "/dev/sda".into(),
                        ..Default::default()
                    },
                    DiskConfig {
                        id: "disk2".to_owned(),
                        device: "/dev".into(),
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
                    ..Default::default()
                },
                internal_verity: vec![],
                internal_mount_points: vec![InternalMountPoint {
                    filesystem: FileSystemType::Ext4,
                    options: vec![],
                    target_id: "part1".to_owned(),
                    path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
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

    /// Validates Storage subsystem HostConfiguration validation logic.
    #[test]
    fn test_validate_host_config_pass() {
        let ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        StorageSubsystem
            .validate_host_config(&ctx, &host_config)
            .unwrap();
    }

    /// Invalid disk device path should fail validation.
    /// Disk device path should start with '/dev'.
    #[test]
    fn test_validate_host_config_invalid_disk_device_path_fail() {
        let ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        let mut host_config = get_host_config(&recovery_key_file);

        host_config.storage.disks.get_mut(0).unwrap().device = "/tmp".into();

        assert_eq!(
            StorageSubsystem
                .validate_host_config(&ctx, &host_config)
                .unwrap_err()
                .kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::InvalidDiskBlockDevicePath {
                    name: "disk1".into(),
                    device: "/tmp".into(),
                }
            })
        );
    }

    // Disk devices must be unique.
    #[test]
    fn tests_validate_host_config_duplicate_disk_path_fail() {
        let ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        let mut host_config = get_host_config(&recovery_key_file);

        host_config.storage.disks.get_mut(0).unwrap().device = "/dev".into();

        assert_eq!(
            StorageSubsystem
                .validate_host_config(&ctx, &host_config)
                .unwrap_err()
                .kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::DiskDefinitionsReferToSameDevice {
                    disk1: "disk1".into(),
                    disk2: "disk2".into(),
                    device: "/dev".into(),
                }
            })
        );
    }

    // Validating the Storage subsystem include encryption configuration validation.
    #[test]
    fn test_validate_host_config_encryption_invalid_fail() {
        let ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        // Delete the recovery key file to make the encryption configuration invalid.
        fs::remove_file(recovery_key_file.path()).unwrap();

        assert_eq!(
            StorageSubsystem
                .validate_host_config(&ctx, &host_config)
                .unwrap_err()
                .kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::InvalidEncryptionKeyFilePath {
                    path: recovery_key_file.path().to_string_lossy().to_string(),
                }
            })
        );
    }

    #[test]
    fn test_generate_fstab() {
        let expected_contents = "/part1 / ext4 defaults 0 1\n";
        let temp_tabfile = tempfile::NamedTempFile::new().unwrap();
        // passing dummy file
        assert_eq!(
            generate_fstab(
                &EngineContext {
                    spec: get_host_config(&temp_tabfile),
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
            &EngineContext {
                spec: get_host_config(&temp_tabfile),
                partition_paths: btreemap! {
                    "part1".into() => PathBuf::from("/part1"),
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
        hc.storage.internal_verity = vec![config::VerityDevice::default()];

        generate_fstab(
            &EngineContext {
                spec: hc,
                partition_paths: btreemap! {
                    "part1".into() => PathBuf::from("/part1"),
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

    use osutils::testutils::repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH};
    use pytest_gen::functional_test;
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
