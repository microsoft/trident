use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use enumflags2::BitFlags;
use log::{debug, info, trace};

use osutils::{
    container, efivar, encryption as osutils_encryption, files, path,
    pcrlock::{self, PCRLOCK_POLICY_JSON_PATH},
};
use sysdefs::tpm2::Pcr;
use trident_api::{
    config::{
        HostConfigurationDynamicValidationError, HostConfigurationStaticValidationError,
        PartitionType,
    },
    error::{InternalError, InvalidInputError, ReportError, ServicingError, TridentError},
};

use crate::{
    engine::{storage::encryption as engine_encryption, EngineContext},
    ServicingType,
};

const CRYPTTAB_PATH: &str = "/etc/crypttab";

/// Validates the encryption configuration.
pub(super) fn validate_host_config(ctx: &EngineContext) -> Result<(), TridentError> {
    if let Some(encryption) = &ctx.spec.storage.encryption {
        // If a recovery key URL is specified, ensure that the file exists, is a regular file,
        // is not empty, and is only accessible by the owner.
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

        // We've already validated that only supported PCRs, i.e. 4, 7, and/or 11, are specified;
        // but we also need to ensure that only PCR 7 is specified for grub images.
        if !ctx.is_uki()? {
            let invalid_pcrs: Vec<_> = encryption
                .pcrs
                .iter()
                .filter(|&&pcr| pcr != Pcr::Pcr7)
                .cloned()
                .collect();

            if !invalid_pcrs.is_empty() {
                let pcrs_string = invalid_pcrs
                    .iter()
                    .map(|pcr| pcr.to_num().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::InvalidEncryptionPcrsForGrubImage {
                        pcrs: pcrs_string,
                    },
                )));
            }
        } else {
            // For UKI images, if PCR 7 is requested, we need to ensure that:
            // 1. Secure Boot is enabled,
            // 2. Trident is NOT running inside a container,
            // due to the limitations of `systemd-pcrlock`.
            if encryption.pcrs.contains(&Pcr::Pcr7) {
                if !efivar::secure_boot_is_enabled() {
                    return Err(TridentError::new(InvalidInputError::from(
                        HostConfigurationDynamicValidationError::Pcr7EncryptionForUkiWhenSecureBootDisabled,
                    )));
                }

                if container::is_running_in_container()? {
                    return Err(TridentError::new(InvalidInputError::from(
                        HostConfigurationDynamicValidationError::Pcr7EncryptionForUkiWhenRunningInContainer,
                    )));
                }
            }
        }
    }

    Ok(())
}

/// Provisions encrypted volumes by re-generating the pcrlock policy, when necessary. Also,
/// persists the pcrlock policy to the update volume.
///
/// On clean install:
/// 1. TODO: UKI MOS + UKI target OS -> re-generate pcrlock policy to include PCRs 4,7,11 as
///    selected by the user; not implemented for now. Related ADO task:
///    https://dev.azure.com/mariner-org/polar/_workitems/edit/13059/.
/// 2. Grub MOS + UKI target OS -> N/A, i.e. keep bootstrapping pcrlock policy of PCR 0,
/// 3. Grub MOS + grub target OS -> N/A, i.e. keep bootstrapping pcrlock policy of PCR 0.
///
/// On A/B update:
/// 1. UKI target OS -> re-generate pcrlock policy to include PCRs 4,7,11 as selected by the user,
/// 2. Grub target OS -> N/A, i.e. keep previous pcrlock policy that includes PCR 7 only.
#[tracing::instrument(name = "encryption_provision", skip_all)]
pub fn provision(ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
    if let Some(encryption) = &ctx.spec.storage.encryption {
        debug!("Initializing encryption provisioning");

        // Determine if pcrlock policy should be re-generated to include updated PCRs
        let updated_pcrs = match ctx.servicing_type {
            ServicingType::CleanInstall => None,
            // On A/B update, use PCRs selected by the user through the API
            ServicingType::AbUpdate => {
                if ctx.is_uki()? {
                    let bitflags = encryption
                        .pcrs
                        .iter()
                        .fold(BitFlags::empty(), |acc, &pcr| acc | BitFlags::from(pcr));
                    Some(bitflags)
                } else {
                    None
                }
            }
            _ => {
                return Err(TridentError::new(InternalError::UnexpectedServicingType {
                    servicing_type: ctx.servicing_type,
                }));
            }
        };

        // If updated PCRs are specified, re-generate pcrlock policy
        if let Some(pcrs) = updated_pcrs {
            debug!(
                "Re-generating pcrlock policy to include PCRs: {:?}",
                pcrs.iter().map(|pcr| pcr.to_num()).collect::<Vec<_>>()
            );
            // Get UKI and bootloader binaries for .pcrlock file generation
            let (uki_binaries, bootloader_binaries) =
                engine_encryption::get_binary_paths_pcrlock(ctx, pcrs, Some(mount_path))
                    .structured(ServicingError::GetBinaryPathsForPcrlockEncryption)?;

            // Re-generate pcrlock policy
            pcrlock::generate_pcrlock_policy(pcrs, uki_binaries, bootloader_binaries)?;
        }

        // If a pcrlock policy JSON file exists, copy it to the update volume
        if Path::new(PCRLOCK_POLICY_JSON_PATH).exists() {
            let pcrlock_json_copy = path::join_relative(mount_path, PCRLOCK_POLICY_JSON_PATH);
            debug!(
                "Copying pcrlock policy JSON from path '{}' to update volume at path '{}'",
                PCRLOCK_POLICY_JSON_PATH,
                pcrlock_json_copy.display()
            );
            fs::copy(PCRLOCK_POLICY_JSON_PATH, pcrlock_json_copy.clone()).structured(
                ServicingError::CopyPcrlockPolicyJson {
                    path: PCRLOCK_POLICY_JSON_PATH.to_string(),
                    destination: pcrlock_json_copy.display().to_string(),
                },
            )?;
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
            ctx.get_first_backing_partition(&ev.device_id)
                .structured(InvalidInputError::from(
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
                osutils_encryption::DEV_RANDOM_PATH,
                osutils_encryption::CIPHER,
                osutils_encryption::KEY_SIZE
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::{os::unix::fs::PermissionsExt, path::Path, str::FromStr};
    use tempfile::NamedTempFile;

    use url::Url;

    use trident_api::{
        config::{
            Disk, EncryptedVolume, Encryption, HostConfiguration, Partition, PartitionSize,
            PartitionType, Storage,
        },
        error::ErrorKind,
    };

    use crate::subsystems::storage::tests as storage_tests;

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
            ab_update: None,
            encryption: Some(Encryption {
                recovery_key_url: Some(Url::from_file_path(recovery_key_file.path()).unwrap()),
                volumes: vec![EncryptedVolume {
                    id: "srv".to_owned(),
                    device_name: "luks-srv".to_owned(),
                    device_id: "srv-enc".to_owned(),
                }],
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn get_ctx(recovery_key_file: &NamedTempFile) -> EngineContext {
        EngineContext {
            spec: HostConfiguration {
                storage: get_storage(recovery_key_file),
                ..Default::default()
            },
            is_uki: Some(false),
            ..Default::default()
        }
    }

    // Encryption configuration without modification is valid.
    #[test]
    fn test_validate_host_config_pass() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let ctx = get_ctx(&recovery_key_file);
        validate_host_config(&ctx).unwrap();
    }

    // Encryption doesn't need to be configured at all.
    #[test]
    fn test_validate_host_config_encryption_none_pass() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let mut ctx = get_ctx(&recovery_key_file);

        ctx.spec.storage.encryption = None;

        validate_host_config(&ctx).unwrap();
    }

    // Encryption recovery key file needs to exist on the system.
    #[test]
    fn test_validate_host_config_recovery_key_not_exist_fail() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let ctx = get_ctx(&recovery_key_file);

        // Delete the recovery key file.
        fs::remove_file(recovery_key_file.path()).unwrap();

        assert_eq!(
            validate_host_config(&ctx).unwrap_err().kind(),
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
        let mut ctx = get_ctx(&recovery_key_file);
        let encryption = ctx.spec.storage.encryption.as_mut().unwrap();

        // Point to the recovery key file's directory.
        let recovery_key_dir: &Path = recovery_key_file.path().parent().unwrap();
        encryption.recovery_key_url = Some(Url::from_directory_path(recovery_key_dir).unwrap());

        assert_eq!(
            validate_host_config(&ctx).unwrap_err().kind(),
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
        let ctx = get_ctx(&recovery_key_file);

        // Loop through all possible permission modes.
        for owner_digit in 0..=7 {
            for group_digit in 0..=7 {
                for other_digit in 0..=7 {
                    // Skip the invalid permission modes, where either the group or other digits are not 0.
                    if group_digit != 0 || other_digit != 0 {
                        continue;
                    }

                    let mode = (owner_digit << 6) | (group_digit << 3) | other_digit;

                    // Set the recovery key file's permissions to mode
                    let mut perms = recovery_key_file.path().metadata().unwrap().permissions();
                    perms.set_mode(mode);
                    fs::set_permissions(recovery_key_file.path(), perms).unwrap();

                    validate_host_config(&ctx).unwrap();
                }
            }
        }
    }

    #[test]
    fn test_validate_host_config_recovery_key_perm_fail() {
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let ctx = get_ctx(&recovery_key_file);

        // Loop through all possible permission modes.
        for owner_digit in 0..=7 {
            for group_digit in 0..=7 {
                for other_digit in 0..=7 {
                    // Skip the valid permission modes, the ones with group and other digits set to 0o0.
                    if group_digit == 0 && other_digit == 0 {
                        continue;
                    }

                    let mode = (owner_digit << 6) | (group_digit << 3) | other_digit;

                    // Set the recovery key file's permissions to mode
                    let mut perms = recovery_key_file.path().metadata().unwrap().permissions();
                    perms.set_mode(mode);
                    fs::set_permissions(recovery_key_file.path(), perms).unwrap();

                    assert_eq!(
                        validate_host_config(&ctx).unwrap_err().kind(),
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
        let ctx = get_ctx(&recovery_key_file);

        // Set the recovery key file's contents to empty.
        fs::write(recovery_key_file.path(), "").unwrap();

        assert_eq!(
            validate_host_config(&ctx).unwrap_err().kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::EncryptionKeyEmpty {
                    key_file: recovery_key_file.path().to_string_lossy().to_string()
                }
            })
        );
    }

    #[test]
    fn test_validate_host_config_encryption_pcrs() {
        // Test case #0: If OS image is a grub image and PCRs include 4, 7, and 11, then fail b/c
        // only PCR 7 is valid for non-UKI images.
        let recovery_key_file = storage_tests::get_recovery_key_file();
        let mut ctx = get_ctx(&recovery_key_file);

        {
            let encryption = ctx.spec.storage.encryption.as_mut().unwrap();
            let pcrs = vec![Pcr::Pcr4, Pcr::Pcr7, Pcr::Pcr11];
            encryption.pcrs = pcrs.clone();
        }

        let pcrs_str = [Pcr::Pcr4, Pcr::Pcr11]
            .iter()
            .map(|pcr| pcr.to_num().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            validate_host_config(&ctx).unwrap_err().kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::InvalidEncryptionPcrsForGrubImage {
                    pcrs: pcrs_str,
                }
            })
        );

        // Test case #1: If OS image is a grub image AND PCRs only include 7, then pass.
        {
            let encryption = ctx.spec.storage.encryption.as_mut().unwrap();
            encryption.pcrs = vec![Pcr::Pcr7];
        }
        validate_host_config(&ctx).unwrap();

        // Test case #2: If OS image is a UKI image AND PCRs only include 4 and 11, then pass.
        ctx.is_uki = Some(true);
        {
            let encryption = ctx.spec.storage.encryption.as_mut().unwrap();
            encryption.pcrs = vec![Pcr::Pcr4, Pcr::Pcr11];
        }
        validate_host_config(&ctx).unwrap();
    }
}
