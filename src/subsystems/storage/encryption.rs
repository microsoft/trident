use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use enumflags2::BitFlags;
use log::{debug, info, trace};

use osutils::{
    encryption, files,
    path::join_relative,
    pcrlock::{self, PCRLOCK_POLICY_JSON_PATH},
};
use sysdefs::tpm2::Pcr;
use trident_api::{
    config::{
        HostConfiguration, HostConfigurationDynamicValidationError,
        HostConfigurationStaticValidationError, PartitionType,
    },
    constants::internal_params::OVERRIDE_PCRLOCK_ENCRYPTION,
    error::{InternalError, InvalidInputError, ReportError, ServicingError, TridentError},
};

use crate::{
    engine::{storage::encryption as storage_encryption, EngineContext},
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

/// Provisions encrypted volumes by re-generating the pcrlock policy, when necessary. Also,
/// persists the pcrlock policy to the update volume.
///
/// On clean install:
/// 1. TODO: UKI MOS + UKI ROS -> re-generate pcrlock policy to include PCRs 4,7,11 as selected by
///    the user; not implemented for now. Related ADO task:
///    https://dev.azure.com/mariner-org/polar/_workitems/edit/13059/.
/// 2. Grub MOS + UKI ROS -> N/A, i.e. keep previous pcrlock policy that includes PCR 0 only,
/// 3. Grub MOS + grub ROS -> N/A, i.e. still sealed to the value of PCR 7.
///
/// On A/B update:
/// 1. UKI ROS -> re-generate pcrlock policy to include PCRs 4,7,11 as selected by the user,
/// 2. Grub ROS -> N/A, i.e. keep previous pcrlock policy that includes PCR 7 only.
#[tracing::instrument(name = "encryption_provision", skip_all)]
pub fn provision(ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
    if let Some(encryption) = &ctx.spec.storage.encryption {
        debug!("Initializing encryption provisioning");

        // Determine if pcrlock policy should be re-generated to include updated PCRs
        let updated_pcrs = match ctx.servicing_type {
            ServicingType::CleanInstall => None,
            // On A/B update, use PCRs selected by the user through the API!
            ServicingType::AbUpdate => {
                if ctx.is_uki()? {
                    // TODO: Remove this internal override once container, BM, and "rerun" E2E
                    // encryption tests are fixed. Related ADO tasks:
                    // https://dev.azure.com/mariner-org/polar/_workitems/edit/13344/ and
                    // https://dev.azure.com/mariner-org/polar/_workitems/edit/14269/.
                    let override_pcrlock_encryption = ctx
                        .spec
                        .internal_params
                        .get_flag(OVERRIDE_PCRLOCK_ENCRYPTION);
                    if !override_pcrlock_encryption {
                        let mut bitflags = BitFlags::empty();
                        if !encryption.pcrs.is_empty() {
                            for pcr in &encryption.pcrs {
                                bitflags |= BitFlags::from(*pcr);
                            }
                        } else {
                            // Use default PCRs if none specified.
                            // TODO: Before grub MOS + UKI ROS encryption flow is enabled &
                            // announced, determine what should be the default, and update here.
                            // Related ADO task:
                            // https://dev.azure.com/mariner-org/polar/_workitems/edit/14485.
                            bitflags |= BitFlags::from(Pcr::Pcr7);
                        }
                        Some(bitflags)
                    } else {
                        debug!(
                            "Runtime OS image is a UKI image, \
                            but internal override '{OVERRIDE_PCRLOCK_ENCRYPTION}' is set to true, \
                            so skipping re-generating pcrlock policy",
                        );
                        None
                    }
                } else {
                    debug!("Runtime OS image is a grub image, so skipping re-generating pcrlock policy");
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
            debug!("Re-generating pcrlock policy to include PCRs: {:?}", pcrs);
            // Get UKI and bootloader binaries for .pcrlock file generation
            let (uki_binaries, bootloader_binaries) =
                storage_encryption::get_binary_paths_pcrlock(ctx, pcrs, Some(mount_path))
                    .structured(ServicingError::GetBinaryPathsForPcrlockEncryption)?;

            // Re-generate pcrlock policy
            pcrlock::generate_pcrlock_policy(pcrs, uki_binaries, bootloader_binaries)?;
        }

        // If a pcrlock policy JSON file exists, copy it to the update volume
        if Path::new(PCRLOCK_POLICY_JSON_PATH).exists() {
            let pcrlock_json_copy = join_relative(mount_path, PCRLOCK_POLICY_JSON_PATH);
            debug!(
                "Copying pcrlock policy JSON to update volume at path '{}'",
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

    use sysdefs::tpm2::Pcr;
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
                pcrs: vec![Pcr::Pcr7],
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
