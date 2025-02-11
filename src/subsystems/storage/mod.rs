use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use log::{debug, error, trace, warn};

use osutils::lsblk;
use trident_api::{
    config::HostConfigurationDynamicValidationError,
    constants::ROOT_MOUNT_POINT_PATH,
    error::{
        InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt,
        UnsupportedConfigurationError,
    },
    status::ServicingType,
    BlockDeviceId,
};

use crate::engine::{EngineContext, Subsystem};

mod encryption;
mod image;
mod osimage;
mod raid;
mod tabfile;
mod verity;

use tabfile::DEFAULT_FSTAB_PATH;

const IMAGE_SUBSYSTEM_NAME: &str = "image";
const ENCRYPTION_SUBSYSTEM_NAME: &str = "encryption";
const OSIMAGE_SUBSYSTEM_NAME: &str = "osimage";

#[derive(Default, Debug)]
pub(crate) struct StorageSubsystem;
impl Subsystem for StorageSubsystem {
    fn name(&self) -> &'static str {
        "storage"
    }

    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.servicing_type != ServicingType::CleanInstall {
            // Ensure that relevant portions of the host configuration have not changed.
            if ctx.spec_old.storage.disks != ctx.spec.storage.disks
                || ctx.spec_old.storage.raid != ctx.spec.storage.raid
                || ctx.spec_old.storage.encryption != ctx.spec.storage.encryption
                || ctx.spec_old.storage.ab_update != ctx.spec.storage.ab_update
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
        for disk in &ctx.spec.storage.disks {
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

        image::validate_host_config(ctx).message(format!(
            "Step 'Validate' failed for subsystem '{IMAGE_SUBSYSTEM_NAME}'"
        ))?;

        encryption::validate_host_config(&ctx.spec).message(format!(
            "Step 'Validate' failed for subsystem '{ENCRYPTION_SUBSYSTEM_NAME}'"
        ))?;

        osimage::validate_host_config(ctx).message(format!(
            "Step 'Validate' failed for subsystem '{OSIMAGE_SUBSYSTEM_NAME}'"
        ))?;

        Ok(())
    }

    fn select_servicing_type(
        &self,
        ctx: &EngineContext,
    ) -> Result<Option<ServicingType>, TridentError> {
        // If ab_update_required() returns true, A/B update is required.
        if image::ab_update_required(ctx)
            .message("Failed to determine if A/B update is required")?
        {
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
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
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
        let mut ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        ctx.spec = get_host_config(&recovery_key_file);

        StorageSubsystem.validate_host_config(&ctx).unwrap();
    }

    /// Invalid disk device path should fail validation.
    /// Disk device path should start with '/dev'.
    #[test]
    fn test_validate_host_config_invalid_disk_device_path_fail() {
        let mut ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        ctx.spec = get_host_config(&recovery_key_file);

        ctx.spec.storage.disks.get_mut(0).unwrap().device = "/tmp".into();

        assert_eq!(
            StorageSubsystem
                .validate_host_config(&ctx)
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
        let mut ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        ctx.spec = get_host_config(&recovery_key_file);

        ctx.spec.storage.disks.get_mut(0).unwrap().device = "/dev".into();

        assert_eq!(
            StorageSubsystem
                .validate_host_config(&ctx)
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
        let mut ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        ctx.spec = get_host_config(&recovery_key_file);

        // Delete the recovery key file to make the encryption configuration invalid.
        fs::remove_file(recovery_key_file.path()).unwrap();

        assert_eq!(
            StorageSubsystem
                .validate_host_config(&ctx)
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
