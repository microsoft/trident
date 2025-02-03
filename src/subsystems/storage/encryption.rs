use std::{fs, os::unix::fs::PermissionsExt, path::PathBuf};

use anyhow::{bail, Context, Error};
use log::{info, trace};

use osutils::{encryption, files};
use trident_api::{
    config::{
        HostConfiguration, HostConfigurationDynamicValidationError,
        HostConfigurationStaticValidationError, Partition, PartitionType,
    },
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
    BlockDeviceId,
};

use crate::engine::EngineContext;

const CRYPTTAB_PATH: &str = "/etc/crypttab";

/// Validates the encryption configuration in Host Configuration.
pub(super) fn validate_host_config(host_config: &HostConfiguration) -> Result<(), TridentError> {
    if let Some(encryption) = &host_config.storage.encryption {
        if let Some(recovery_key_url) = &encryption.recovery_key_url {
            let key_file: PathBuf = recovery_key_url.path().into();

            if !key_file.exists() {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::InvalidEncryptionKeyFilePath {
                        path: key_file.to_string_lossy().to_string(),
                    },
                )));
            }

            let key_file_metadata = fs::metadata(&key_file).structured(InvalidInputError::from(
                HostConfigurationDynamicValidationError::GetEncryptionKeyMetadata {
                    key_file: key_file.to_string_lossy().to_string(),
                },
            ))?;

            if key_file_metadata.len() == 0 {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::EncryptionKeyEmpty {
                        key_file: key_file.to_string_lossy().to_string(),
                    },
                )));
            }

            if !key_file_metadata.is_file() {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::EncryptionKeyNotRegularFile {
                        key_file: key_file.to_string_lossy().to_string(),
                    },
                )));
            }

            let key_file_perms_mode = key_file_metadata.permissions().mode();

            // In Unix-based systems, the file mode is represented by four
            // octal digits. The first digit specifies the file type,
            // while the subsequent three digits determine the access
            // permissions for the owner, group, and others, respectively.
            // To confirm that only the file owner possesses read and
            // write permissions, it's essential to check that neither the
            // group nor others have any permissions. This is accomplished
            // by applying a bitmask '& 0o77' to the mode, which isolates
            // the permissions for the group and others. We then verify
            // that these isolated permissions are indeed set to 0,
            // ensuring exclusive access for the owner.
            if (key_file_perms_mode & 0o77) != 0 {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::EncryptionKeyInvalidPermissions {
                        key_file: key_file.to_string_lossy().to_string(),
                        permissions: key_file_perms_mode & 0o777,
                    },
                )));
            }
        }
    }

    Ok(())
}

#[tracing::instrument(name = "encryption_configuration", skip_all)]
pub fn configure(ctx: &EngineContext) -> Result<(), TridentError> {
    let path = PathBuf::from(CRYPTTAB_PATH);
    let mut contents = String::new();

    let Some(ref encryption) = ctx.spec.storage.encryption else {
        return Ok(());
    };

    for ev in encryption.volumes.iter() {
        let backing_partition =
            get_first_backing_partition(ctx, &ev.device_id).structured(InvalidInputError::from(
                HostConfigurationStaticValidationError::EncryptedVolumeNotPartitionOrRaid {
                    encrypted_volume: ev.id.clone(),
                },
            ))?;
        let device_path = &ctx.get_block_device_path(&ev.device_id).structured(
            ServicingError::FindEncryptedVolumeBlockDevice {
                device_id: ev.device_id.clone(),
                encrypted_volume: ev.id.clone(),
            },
        )?;

        // An encrypted swap device is special-cased in the crypttab due to the unique nature and
        // requirements of swap spaces in a Linux system. Since it often contains sensitive data
        // temporarily stored in RAM, encrypting it is crucial for security. However, unlike the
        // regular partitions, which use TPM 2.0 devices for passwordless startup, systemd
        // completely wipes the swap device and formats it on each system startup.
        //
        // For systemd to do this, it needs a key, and here in the crypttab, the swap device is
        // configured with a randomly generated key from `/dev/random`. This is the most reliable
        // way to generate a truly random key on Linux systems.
        //
        // The default cipher (aes-cbc-essiv:sha256) and key size (256) are not used here, to
        // enhance the security posture of the swap space and align it with the rest of the
        // encrypted devices.
        if backing_partition.partition_type == PartitionType::Swap {
            contents.push_str(&format!(
                "{}\t{}\t{}\tluks,swap,cipher={},size={}\n",
                ev.device_name,
                device_path.display(),
                encryption::DEV_RANDOM_PATH,
                encryption::CIPHER,
                encryption::KEY_SIZE
            ));
        } else {
            contents.push_str(&format!(
                "{}\t{}\t{}\tluks,tpm2-device=auto\n",
                ev.device_name,
                device_path.display(),
                "none"
            ));
        }
    }

    if contents.is_empty() {
        if path.exists() {
            info!("Removing crypttab because there are no encrypted volumes");
            fs::remove_file(&path).structured(ServicingError::RemoveCrypttab {
                crypttab_path: path.to_string_lossy().to_string(),
            })?;
        }
    } else {
        trace!("crypttab file contents:\n{contents}");
        files::write_file(path.clone(), 0o644, contents.as_bytes()).structured(
            ServicingError::CreateCrypttab {
                crypttab_path: path.to_string_lossy().to_string(),
            },
        )?;
    }

    Ok(())
}

/// Returns the first partition that backs the given block device, or Err if the block device ID
/// does not correspond to a partition or software RAID array.
fn get_first_backing_partition<'a>(
    ctx: &'a EngineContext,
    block_device_id: &BlockDeviceId,
) -> Result<&'a Partition, Error> {
    if let Some(partition) = ctx.spec.storage.get_partition(block_device_id) {
        Ok(partition)
    } else if let Some(array) = ctx
        .spec
        .storage
        .raid
        .software
        .iter()
        .find(|r| &r.id == block_device_id)
    {
        let partition_id = array
            .devices
            .first()
            .context(format!("RAID array '{}' has no partitions", array.id))?;

        ctx.spec
            .storage
            .get_partition(partition_id)
            .context(format!(
                "RAID array '{}' doesn't reference partition",
                block_device_id
            ))
    } else {
        bail!("Block device '{block_device_id}' is not a partition or RAID array")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{os::unix::fs::PermissionsExt, path::Path, str::FromStr};

    use url::Url;

    use trident_api::{
        config::{
            Disk, EncryptedVolume, Encryption, FileSystemType, InternalMountPoint, Partition,
            PartitionSize, PartitionType, Raid, RaidLevel, SoftwareRaidArray, Storage,
        },
        constants,
        error::ErrorKind,
    };

    use crate::subsystems::storage::tests as storage_tests;

    #[test]
    fn test_get_first_backing_partition() {
        let ctx = EngineContext {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        partitions: vec![
                            Partition {
                                id: "esp".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "root".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("8G").unwrap(),
                            },
                            Partition {
                                id: "rootb".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("8G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
                    raid: Raid {
                        software: vec![SoftwareRaidArray {
                            id: "root-raid1".to_owned(),
                            devices: vec!["root".to_string(), "rootb".to_string()],
                            name: "raid1".to_string(),
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

        assert_eq!(
            get_first_backing_partition(&ctx, &"esp".to_owned()).unwrap(),
            &ctx.spec.storage.disks[0].partitions[0]
        );
        assert_eq!(
            get_first_backing_partition(&ctx, &"root".to_owned()).unwrap(),
            &ctx.spec.storage.disks[0].partitions[1]
        );
        assert_eq!(
            get_first_backing_partition(&ctx, &"rootb".to_owned()).unwrap(),
            &ctx.spec.storage.disks[0].partitions[2]
        );
        assert_eq!(
            get_first_backing_partition(&ctx, &"root-raid1".to_owned()).unwrap(),
            &ctx.spec.storage.disks[0].partitions[1]
        );
        get_first_backing_partition(&ctx, &"os".to_owned()).unwrap_err();
        get_first_backing_partition(&ctx, &"non-existant".to_owned()).unwrap_err();
    }

    fn get_storage(recovery_key_file: &tempfile::NamedTempFile) -> Storage {
        Storage {
            disks: vec![Disk {
                id: "os".to_owned(),
                device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2".into(),
                partitions: vec![
                    Partition {
                        id: "esp".to_owned(),
                        partition_type: PartitionType::Esp,
                        size: PartitionSize::from_str("1G").unwrap(),
                    },
                    Partition {
                        id: "root".to_owned(),
                        partition_type: PartitionType::Root,
                        size: PartitionSize::from_str("8G").unwrap(),
                    },
                    Partition {
                        id: "srv-enc".to_owned(),
                        partition_type: PartitionType::Srv,
                        size: PartitionSize::from_str("1T").unwrap(),
                    },
                ],
                ..Default::default()
            }],
            internal_mount_points: vec![
                InternalMountPoint {
                    path: PathBuf::from("/boot/efi"),
                    target_id: "esp".to_string(),
                    filesystem: FileSystemType::Vfat,
                    options: vec!["defaults".to_owned()],
                },
                InternalMountPoint {
                    path: constants::ROOT_MOUNT_POINT_PATH.into(),
                    target_id: "root".to_string(),
                    filesystem: FileSystemType::Ext4,
                    options: vec!["defaults".to_owned()],
                },
                InternalMountPoint {
                    path: PathBuf::from("/srv"),
                    target_id: "srv".to_string(),
                    filesystem: FileSystemType::Ext4,
                    options: vec!["defaults".to_owned()],
                },
            ],
            ab_update: None,
            encryption: Some(Encryption {
                recovery_key_url: Some(Url::from_file_path(recovery_key_file.path()).unwrap()),
                volumes: vec![EncryptedVolume {
                    id: "srv".to_owned(),
                    device_name: "luks-srv".to_owned(),
                    device_id: "srv-enc".to_owned(),
                }],
            }),
            ..Default::default()
        }
    }

    fn get_host_config(recovery_key_file: &tempfile::NamedTempFile) -> HostConfiguration {
        HostConfiguration {
            storage: get_storage(recovery_key_file),
            ..Default::default()
        }
    }

    // Encryption configuration without modification is valid.
    #[test]
    fn test_validate_host_config_pass() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);
        validate_host_config(&host_config).unwrap();
    }

    // Encryption doesn't need to be configured at all.
    #[test]
    fn test_validate_host_config_encryption_none_pass() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let mut host_config = get_host_config(&recovery_key_file);

        host_config.storage.encryption = None;

        validate_host_config(&host_config).unwrap();
    }

    // Encryption recovery key file needs to exist on the system.
    #[test]
    fn test_validate_host_config_recovery_key_not_exist_fail() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        // Delete the recovery key file.
        fs::remove_file(recovery_key_file.path()).unwrap();

        assert_eq!(
            validate_host_config(&host_config).unwrap_err().kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::InvalidEncryptionKeyFilePath {
                    path: recovery_key_file.path().to_string_lossy().to_string()
                }
            })
        );
    }

    // Encryption needs recovery key url to point to a file.
    #[test]
    fn test_validate_host_config_recovery_key_not_file_fail() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let mut host_config = get_host_config(&recovery_key_file);
        let encryption = host_config.storage.encryption.as_mut().unwrap();

        // Point to the recovery key file's directory.
        let recovery_key_dir: &Path = recovery_key_file.path().parent().unwrap();
        encryption.recovery_key_url = Some(Url::from_directory_path(recovery_key_dir).unwrap());

        assert_eq!(
            validate_host_config(&host_config).unwrap_err().kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::EncryptionKeyNotRegularFile {
                    key_file: format!("{}/", recovery_key_dir.to_string_lossy())
                }
            })
        );
    }

    // Encryption needs recovery key url to point to a file that is only accessible by the owner.
    #[test]
    fn test_validate_host_config_recovery_key_perm_pass() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        // Loop through all possible permission modes.
        for owner_digit in 0..=7 {
            for group_digit in 0..=7 {
                for other_digit in 0..=7 {
                    // Skip the invalid permission modes, where either the group or other digits are not 0.
                    if group_digit != 0 || other_digit != 0 {
                        continue;
                    }

                    let mode = owner_digit << 6 | group_digit << 3 | other_digit;

                    // Set the recovery key file's permissions to mode
                    let mut perms = recovery_key_file.path().metadata().unwrap().permissions();
                    perms.set_mode(mode);
                    fs::set_permissions(recovery_key_file.path(), perms).unwrap();

                    validate_host_config(&host_config).unwrap();
                }
            }
        }
    }

    #[test]
    fn test_validate_host_config_recovery_key_perm_fail() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        // Loop through all possible permission modes.
        for owner_digit in 0..=7 {
            for group_digit in 0..=7 {
                for other_digit in 0..=7 {
                    // Skip the valid permission modes, the ones with group and other digits set to 0o0.
                    if group_digit == 0 && other_digit == 0 {
                        continue;
                    }

                    let mode = owner_digit << 6 | group_digit << 3 | other_digit;

                    // Set the recovery key file's permissions to mode
                    let mut perms = recovery_key_file.path().metadata().unwrap().permissions();
                    perms.set_mode(mode);
                    fs::set_permissions(recovery_key_file.path(), perms).unwrap();

                    assert_eq!(
                        validate_host_config(&host_config).unwrap_err().kind(),
                        &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                            inner: HostConfigurationDynamicValidationError::EncryptionKeyInvalidPermissions {
                                key_file: recovery_key_file.path().to_string_lossy().to_string(),
                                permissions: mode & 0o777,
                            }
                        })
                    );
                }
            }
        }
    }

    #[test]
    fn test_validate_host_config_recovery_key_empty_fail() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        // Set the recovery key file's contents to empty.
        fs::write(recovery_key_file.path(), "").unwrap();

        assert_eq!(
            validate_host_config(&host_config).unwrap_err().kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::EncryptionKeyEmpty {
                    key_file: recovery_key_file.path().to_string_lossy().to_string()
                }
            })
        );
    }
}
