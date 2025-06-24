use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use enumflags2::BitFlags;
use log::{debug, info, trace};

use osutils::{encryption, files, path::join_relative, pcrlock};
use sysdefs::tpm2::Pcr;

use trident_api::{
    config::{
        HostConfiguration, HostConfigurationDynamicValidationError,
        HostConfigurationStaticValidationError, PartitionType,
    },
    constants::{internal_params::OVERRIDE_ENCRYPTION_PCRS, ESP_MOUNT_POINT_PATH},
    error::{InternalError, InvalidInputError, ReportError, ServicingError, TridentError},
};

use crate::{
    engine::{
        boot::uki::{TMP_UKI_NAME, UKI_DIRECTORY},
        bootentries, EngineContext,
    },
    ServicingType,
};

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

/// Provisions encrypted volumes when a UKI image is being installed:
/// - TODO: On a clean install, generates .pcrlock files for the runtime OS image A, creates a
///   pcrlock TPM 2.0 access policy based on PCRs 4, 7, and 11, and re-enrolls all encrypted
///   volumes with the new policy,
/// - On A/B update, re-generates the pcrlock policy for the update image using PCRs 4, 7, and 11.
#[tracing::instrument(name = "encryption_provision", skip_all)]
pub fn provision(ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
    if let Some(encryption) = &ctx.spec.storage.encryption {
        // Determine PCRs depending on the current servicing type:
        // - For a clean install, use all UKI PCRs 4, 7, and 11 and generate .pcrlock files,
        // - For A/B update, reduce to PCR 7.
        let pcrs = match ctx.servicing_type {
            ServicingType::CleanInstall => {
                // If the internal parameter is not set, default to PCRs 7, 4, and 11.
                //
                // TODO: For E2E testing, we're excluding PCR 7 b/c SecureBoot is not enabled in
                // MOS or ROS. Need to enable PCR 7 for E2E testing once SecureBoot is enabled.
                //
                // TODO: Once a UKI MOS is built, i.e. sealing to PCRs 11 and 4 is possible, enable
                // PCR-based encryption for clean install.
                debug!("PCR-based encryption is disabled on clean install");
                return Ok(());
            }
            ServicingType::AbUpdate => {
                // TODO: Also pass binaries for CURRENT OS image!!!

                // UKI binary in runtime OS to be measured; it's currently staged at designated
                // path
                let esp_dir_path = join_relative(mount_path, ESP_MOUNT_POINT_PATH);
                let uki_update = esp_dir_path.join(UKI_DIRECTORY).join(TMP_UKI_NAME);

                // Bootloader binary for runtime OS to be measured, i.e. shim EFI executable for
                // UKI
                let (_, bootloader_update_relative) = bootentries::get_label_and_path(ctx)
                    .structured(ServicingError::GetLabelAndPath)?;
                let bootloader_update = join_relative(esp_dir_path, bootloader_update_relative);

                let pcrs = ctx
                    .spec
                    .internal_params
                    .get::<Vec<Pcr>>(OVERRIDE_ENCRYPTION_PCRS)
                    .transpose()
                    .structured(InvalidInputError::InvalidInternalParameter {
                        name: OVERRIDE_ENCRYPTION_PCRS.to_string(),
                        explanation: format!(
                            "Failed to parse internal parameter '{}' as BitFlags<Pcr>",
                            OVERRIDE_ENCRYPTION_PCRS
                        ),
                    })?
                    .map(|v| BitFlags::<Pcr>::from_iter(v.into_iter()))
                    .unwrap_or(Pcr::Pcr4 | Pcr::Pcr7 | Pcr::Pcr11);

                // Generate .pcrlock files for runtime OS image A
                pcrlock::generate_pcrlock_files(pcrs, vec![uki_update], vec![bootloader_update])
                    .structured(ServicingError::GeneratePcrlockFiles)?;

                pcrs
            }
            _ => {
                return Err(TridentError::new(InternalError::UnexpectedServicingType {
                    servicing_type: ctx.servicing_type,
                }))
            }
        };

        // Generate pcrlock policy; on A/B update, the binding will thus automatically be updated
        // with the new pcrlock policy
        pcrlock::generate_tpm2_access_policy(pcrs)
            .structured(ServicingError::GenerateTpm2AccessPolicy)?;

        // If PCRLOCK_POLICY_PATH doesn't exist yet, we're doing this for the first time, so we
        // need to iterate through encrypted volumes and bind them to the newly generated pcrlock
        // policy.
        //
        // TODO: Right now, this happens while Trident is staging the first A/B update. Once PCR
        // encryption is enabled on clean install, this will take place during clean install.
        if pcrlock::is_pcrlock_policy() {
            debug!("Re-enrolling encrypted volumes with the new pcrlock policy");
            for ev in encryption.volumes.iter() {
                // Fetch the block device path of the encrypted volume
                let device_path = ctx.get_block_device_path(&ev.device_id).structured(
                    ServicingError::FindEncryptedVolumeBlockDevice {
                        device_id: ev.device_id.clone(),
                        encrypted_volume: ev.id.clone(),
                    },
                )?;

                // Re-enroll the device with the pcrlock policy
                encryption::systemd_cryptenroll(None::<&Path>, device_path, true, pcrs)
                    .structured(ServicingError::BindEncryptionToPcrlockPolicy)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::{os::unix::fs::PermissionsExt, path::Path, str::FromStr};

    use url::Url;

    use trident_api::{
        config::{
            Disk, EncryptedVolume, Encryption, Partition, PartitionSize, PartitionType, Storage,
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

                    let mode = (owner_digit << 6) | (group_digit << 3) | other_digit;

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

                    let mode = (owner_digit << 6) | (group_digit << 3) | other_digit;

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
