use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use enumflags2::BitFlags;
use log::{debug, info, trace};

use osutils::{
    bootloaders::{BOOT_EFI, GRUB_EFI},
    container,
    encryption::{self, KeySlotType},
    files,
    path::join_relative,
    pcrlock::{self, PCRLOCK_POLICY_JSON},
};
use sysdefs::tpm2::Pcr;

use trident_api::{
    config::{
        HostConfiguration, HostConfigurationDynamicValidationError,
        HostConfigurationStaticValidationError, PartitionType,
    },
    constants::{
        internal_params::OVERRIDE_ENCRYPTION_PCRS, ESP_EFI_DIRECTORY, ESP_MOUNT_POINT_PATH,
    },
    error::{InternalError, InvalidInputError, ReportError, ServicingError, TridentError},
    status::AbVolumeSelection,
};

use crate::{
    engine::{
        boot::{
            self,
            uki::{self, TMP_UKI_NAME, UKI_DIRECTORY},
        },
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
///    pcrlock TPM 2.0 access policy based on PCRs 4, 7, and 11, and re-enrolls all encrypted
///    volumes with the new policy. Related ADO task:
///    https://dev.azure.com/mariner-org/ECF/_workitems/edit/12865/.
/// - On A/B update, re-generates the pcrlock policy to include current boot & future boot with
///    update OS image, using PCRs 4, 7, and 11.
#[tracing::instrument(name = "encryption_provision", skip_all)]
pub fn provision(ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
    if let Some(encryption) = &ctx.spec.storage.encryption {
        debug!("Initializing pcrlock encryption provisioning");
        // Determine PCRs depending on the current servicing type:
        // - For clean install, temporarily use only PCR 0,
        // - For A/B update, use PCRs 4, 7, and 11.
        // TODO: Once UKI MOS is built, include all UKI PCRs, i.e. 4, 7, and 11, into pcrlock
        // policy on clean install as well. Related ADO task:
        // https://dev.azure.com/mariner-org/ECF/_workitems/edit/12865/.
        let pcrs = match ctx.servicing_type {
            ServicingType::CleanInstall => {
                // Generate .pcrlock files for runtime OS image A, only using PCR 0, thanks to
                // OVERRIDE_ENCRYPTION_PCRS.
                //
                // TODO: Once UKI MOS is built, include ROS A UKI and bootloader binaries.
                // https://dev.azure.com/mariner-org/ECF/_workitems/edit/12865/.
                let pcrs = ctx
                    .spec
                    .internal_params
                    .get::<Vec<Pcr>>(OVERRIDE_ENCRYPTION_PCRS)
                    .transpose()
                    .structured(InvalidInputError::InvalidInternalParameter {
                        name: OVERRIDE_ENCRYPTION_PCRS.to_string(),
                        explanation: format!(
                            "Failed to parse internal parameter '{OVERRIDE_ENCRYPTION_PCRS}' as BitFlags<Pcr>",
                        ),
                    })?
                    .map(|v| BitFlags::<Pcr>::from_iter(v.into_iter()))
                    .unwrap_or(Pcr::Pcr4 | Pcr::Pcr7 | Pcr::Pcr11);

                pcrlock::generate_pcrlock_files(pcrs, vec![], vec![])
                    .structured(ServicingError::GeneratePcrlockFiles)?;

                pcrs
            }
            ServicingType::AbUpdate => {
                // Use UKI PCRs
                let pcrs = Pcr::Pcr4 | Pcr::Pcr11;

                // Construct current UKI path
                let esp_path = if container::is_running_in_container()? {
                    let host_root = container::get_host_root_path()?;
                    join_relative(host_root, ESP_MOUNT_POINT_PATH)
                } else {
                    PathBuf::from(ESP_MOUNT_POINT_PATH)
                };
                let esp_uki_directory = join_relative(esp_path.clone(), UKI_DIRECTORY);
                let uki_suffix = match ctx.ab_active_volume {
                    Some(AbVolumeSelection::VolumeA) => format!("azla{}.efi", ctx.install_index),
                    Some(AbVolumeSelection::VolumeB) => {
                        format!("azlb{}.efi", ctx.install_index)
                    }
                    None => {
                        return Err(TridentError::new(InternalError::GetAbActiveVolume));
                    }
                };
                let existing_ukis = uki::enumerate_existing_ukis(&esp_uki_directory)
                    .structured(ServicingError::EnumerateUkis)?;
                let max_index = existing_ukis
                    .iter()
                    .map(|(index, _suffix, _path)| *index)
                    .max()
                    .unwrap_or(99);
                let uki_current =
                    esp_uki_directory.join(format!("vmlinuz-{max_index}-{uki_suffix}"));
                trace!("Current UKI binary path: {}", uki_current.display());

                // Construct current primary bootloader path, i.e. shim EFI executable
                let ab_volume = ctx
                    .ab_active_volume
                    .ok_or_else(|| TridentError::new(InternalError::GetAbActiveVolume))?;
                let esp_dir_name = boot::make_esp_dir_name(ctx.install_index, ab_volume);
                let shim_path = Path::new(ESP_EFI_DIRECTORY)
                    .join(&esp_dir_name)
                    .join(BOOT_EFI);
                let shim_current = join_relative(esp_path.clone(), &shim_path);
                trace!("Current shim binary path: {}", shim_current.display());

                // Construct current secondary bootloader path, i.e. systemd-boot EFI executable
                let systemd_boot_path = Path::new(ESP_EFI_DIRECTORY)
                    .join(&esp_dir_name)
                    .join(GRUB_EFI);
                let systemd_boot_current = join_relative(esp_path, &systemd_boot_path);
                trace!(
                    "Current systemd-boot binary path: {}",
                    systemd_boot_current.display()
                );

                // UKI binary in runtime OS to be measured; it's currently staged at designated
                // path
                let esp_dir_path = join_relative(mount_path, ESP_MOUNT_POINT_PATH);
                let uki_update = esp_dir_path.join(UKI_DIRECTORY).join(TMP_UKI_NAME);
                trace!("Update UKI binary path: {}", uki_update.display());

                // Primary bootloader, i.e. shim EFI executable, in update image
                let (_, shim_update_relative) = bootentries::get_label_and_path(ctx, BOOT_EFI)
                    .structured(ServicingError::GetLabelAndPath)?;
                let shim_update = join_relative(esp_dir_path.clone(), shim_update_relative);
                trace!("Update shim binary path: {}", shim_update.display());

                // Secondary bootloader, i.e. systemd-boot EFI executable, in update image
                let (_, systemd_boot_update_relative) =
                    bootentries::get_label_and_path(ctx, GRUB_EFI)
                        .structured(ServicingError::GetLabelAndPath)?;
                let systemd_boot_update = join_relative(esp_dir_path, systemd_boot_update_relative);
                trace!(
                    "Update systemd-boot binary path: {}",
                    systemd_boot_update.display()
                );

                // Generate .pcrlock files for runtime OS image A
                pcrlock::generate_pcrlock_files(
                    pcrs,
                    // List of UKI binaries
                    vec![Some(uki_current), Some(uki_update)],
                    // List of bootloader PE binaries
                    vec![
                        Some(shim_current),
                        Some(systemd_boot_current),
                        Some(shim_update),
                        Some(systemd_boot_update),
                    ],
                )
                .structured(ServicingError::GeneratePcrlockFiles)?;

                pcrs
            }
            _ => {
                return Err(TridentError::new(InternalError::UnexpectedServicingType {
                    servicing_type: ctx.servicing_type,
                }));
            }
        };

        // Generate pcrlock policy; on A/B update, the existing binding will be automatically
        // updated with the new pcrlock policy
        pcrlock::generate_tpm2_access_policy(pcrs)
            .structured(ServicingError::GenerateTpm2AccessPolicy)?;

        // Copy the pcrlock policy JSON to the update volume
        debug!("Copying pcrlock policy JSON to update volume");
        let pcrlock_json_copy = join_relative(mount_path, PCRLOCK_POLICY_JSON);
        fs::copy(PCRLOCK_POLICY_JSON, pcrlock_json_copy.clone()).structured(
            ServicingError::CopyPcrlockPolicyJson {
                path: PCRLOCK_POLICY_JSON.to_string(),
                destination: pcrlock_json_copy.display().to_string(),
            },
        )?;

        // On clean install, we need to iterate through encrypted volumes and bind them to the
        // newly generated pcrlock policy
        if ctx.servicing_type == ServicingType::CleanInstall {
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
                encryption::systemd_cryptenroll(None::<&Path>, device_path.clone(), true, pcrs)
                    .structured(ServicingError::BindEncryptionToPcrlockPolicy)?;

                // If the key file was randomly generated and NOT provided by the user as a
                // recovery key, remove the password key slot from the encrypted volume, as it's
                // not needed, for security
                if encryption.recovery_key_url.is_none() {
                    debug!("Removing password key slot from encrypted volume {}", ev.id);
                    encryption::systemd_cryptenroll_wipe_slot(
                        device_path.clone(),
                        KeySlotType::Password,
                    )
                    .structured(ServicingError::WipePasswordKeySlot {
                        device_path: device_path.to_string_lossy().to_string(),
                    })?;
                }
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
