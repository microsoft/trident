use std::{
    collections::BTreeMap,
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use enumflags2::BitFlags;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use osutils::{
    dependencies::{Dependency, DependencyResultExt},
    encryption::{self, ENCRYPTION_PASSPHRASE},
    lsblk::{self, BlockDeviceType},
};
use sysdefs::tpm2::Pcr;
use trident_api::{
    config::{HostConfiguration, HostConfigurationStaticValidationError, PartitionSize},
    constants::internal_params::{
        NO_CLOSE_ENCRYPTED_VOLUMES, OVERRIDE_ENCRYPTION_PCRS, REENCRYPT_ON_CLEAN_INSTALL,
    },
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
};

use crate::engine::EngineContext;

/// Closes all open LUKS2-encrypted volumes found on the system.
pub(super) fn close_pre_existing_encrypted_volumes(
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    if host_config
        .internal_params
        .get_flag(NO_CLOSE_ENCRYPTED_VOLUMES)
    {
        return Ok(());
    }

    let crypt_block_devices = lsblk::find(|blkdev| blkdev.blkdev_type == BlockDeviceType::Crypt)
        .context("Failed to find crypt block devices")?;
    for crypt_block_device in crypt_block_devices {
        debug!(
            "Closing pre-existing encrypted volume '{}'",
            crypt_block_device.name
        );

        encryption::cryptsetup_close(&crypt_block_device.name)?;
    }

    Ok(())
}

/// Describes the type of encryption.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum EncryptionType {
    LuksFormat,
    Reencrypt,
}

/// Sets up and opens encrypted devices.
#[tracing::instrument(name = "encrypted_devices_creation", fields(total_partition_size_bytes = tracing::field::Empty), skip_all)]
pub(super) fn create_encrypted_devices(
    ctx: &EngineContext,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    if let Some(encryption) = &host_config.storage.encryption {
        let key_file_tmp: NamedTempFile;
        let key_file_path: PathBuf;

        // Store key to update ENCRYPTION_PASSPHRASE static variable
        let key_value: Option<Vec<u8>>;

        if let Some(recovery_key_url) = &encryption.recovery_key_url {
            key_file_path = recovery_key_url.path().into();

            // Read key from existing recovery key file
            let key = fs::read(&key_file_path).structured(ServicingError::ReadRecoveryKeyFile {
                key_file: key_file_path.to_string_lossy().to_string(),
            })?;
            key_value = Some(key);
        } else {
            // Create a temporary file to store the recovery key file.
            key_file_tmp =
                NamedTempFile::new().structured(ServicingError::CreateRecoveryKeyFile)?;
            key_file_path = key_file_tmp.path().to_owned();
            fs::set_permissions(&key_file_path, Permissions::from_mode(0o600)).structured(
                ServicingError::SetRecoveryKeyFilePermissions {
                    key_file: key_file_path.to_string_lossy().to_string(),
                },
            )?;
            let key = encryption::generate_recovery_key_file(&key_file_path).structured(
                ServicingError::GenerateRecoveryKeyFile {
                    key_file: key_file_path.to_string_lossy().to_string(),
                },
            )?;

            key_value = Some(key.clone());
        };

        // Store the key statically for later use, i.e. pcrlock policy enrollment
        if let Some(key) = key_value {
            debug!(
                "Storing encryption passphrase in memory for later use: {}",
                key_file_path.display()
            );
            let mut static_key = ENCRYPTION_PASSPHRASE.lock().unwrap();
            *static_key = key;
        }

        debug!(
            "Using key file '{}' to initialize all encrypted volumes",
            key_file_path.display()
        );

        // Check that the TPM 2.0 device is accessible.
        Dependency::Tpm2Pcrread
            .cmd()
            .run_and_check()
            .message("Encryption requires access to a TPM 2.0 device but one is not accessible")?;

        // Clear the TPM 2.0 device to ensure that it is in a known state. By clearing the lockout
        // value, Trident prevents the TPM 2.0 device from being placed into DA lockout mode due to
        // repeated successive provisioning attempts.
        Dependency::Tpm2Clear
            .cmd()
            .run_and_check()
            .message("Failed to clear TPM 2.0 device")?;

        let mut total_partition_size_bytes: u64 = 0;
        for ev in encryption.volumes.iter() {
            // Get the block device indicated by device_id if it is a partition; the first
            // partition of device_id if it is a RAID array; or, an error if device_id is neither
            // a partition nor a RAID array.
            let partition = ctx.get_first_backing_partition(&ev.device_id).structured(
                InvalidInputError::from(
                    HostConfigurationStaticValidationError::EncryptedVolumeNotPartitionOrRaid {
                        encrypted_volume: ev.id.clone(),
                    },
                ),
            )?;
            if let PartitionSize::Fixed(byte_count) = partition.size {
                total_partition_size_bytes += byte_count.bytes();
            }

            info!(
                "Initializing '{}': creating encrypted volume of type '{}'",
                ev.id,
                partition.partition_type.to_sdrepart_part_type()
            );

            let device_path = ctx.get_block_device_path(&ev.device_id).structured(
                ServicingError::FindEncryptedVolumeBlockDevice {
                    device_id: ev.device_id.clone(),
                    encrypted_volume: ev.id.clone(),
                },
            )?;

            let encryption_type = if host_config
                .internal_params
                .get_flag(REENCRYPT_ON_CLEAN_INSTALL)
            {
                EncryptionType::Reencrypt
            } else {
                EncryptionType::LuksFormat
            };

            let pcrs = ctx
                .spec
                .internal_params
                .get::<Vec<Pcr>>(OVERRIDE_ENCRYPTION_PCRS)
                .transpose()
                .structured(InvalidInputError::InvalidInternalParameter {
                    name: OVERRIDE_ENCRYPTION_PCRS.to_string(),
                    explanation: format!(
                        "Failed to parse internal parameter '{OVERRIDE_ENCRYPTION_PCRS}' as BitFlags<Pcr>"
                    ),
                })?
                // Convert the `Vec<Pcr>` into a `BitFlags<Pcr>`, which is a bitmask of PCRs.
                .map(|v| BitFlags::<Pcr>::from_iter(v.into_iter()))
                // If the internal parameter is not set, default to PCR 7.
                .unwrap_or(Pcr::Pcr7.into());

            // Check if `REENCRYPT_ON_CLEAN_INSTALL` internal param is set to true; if so, re-encrypt
            // the device in-place. Otherwise, initialize a new LUKS2 volume.
            encrypt_and_open_device(
                &device_path,
                &ev.device_name,
                &key_file_path,
                encryption_type,
                pcrs,
            )
            .structured(ServicingError::EncryptBlockDevice {
                device_path: device_path.to_string_lossy().to_string(),
                device_id: ev.device_id.clone(),
                encrypted_volume_device_name: ev.device_name.clone(),
                encrypted_volume: ev.id.clone(),
            })?;
        }
        tracing::Span::current().record("total_partition_size_bytes", total_partition_size_bytes);
    }

    Ok(())
}

/// Encrypts the device of a single encrypted volume by reformatting the device with a LUKS2
/// header, enrolling a key file, enrolling another randomly generated key and sealing it in the
/// TPM 2.0 device with PCR 7, and finally, opening the device as a LUKS2 volume.
///
/// This function takes in 5 arguments:
/// - `device_path`: The path to the device to be encrypted.
/// - `device_name`: The name of the device to be used in the crypttab.
/// - `key_file`: The path to the key file to be used for encryption.
/// - `encryption_type`: The type of encryption to be used. Determines whether the device should be
///   re-encrypted in-place, or whether a new LUKS2 volume should be initialized
/// - `pcrs`: The PCRs to bind the encryption key to.
fn encrypt_and_open_device(
    device_path: &Path,
    device_name: &String,
    key_file: &Path,
    encryption_type: EncryptionType,
    pcrs: BitFlags<Pcr>,
) -> Result<(), Error> {
    match encryption_type {
        EncryptionType::Reencrypt => {
            debug!(
                "Re-encrypting underlying device '{}' in-place",
                device_path.display()
            );
            encryption::cryptsetup_reencrypt(key_file, device_path).context(format!(
                "Failed to re-encrypt underlying device '{}'",
                device_path.display()
            ))?;
        }
        EncryptionType::LuksFormat => {
            debug!(
                "Encrypting underlying device '{}' with LUKS2",
                device_path.display()
            );
            encryption::cryptsetup_luksformat(key_file, device_path).context(format!(
                "Failed to encrypt underlying device '{}'",
                device_path.display()
            ))?;
        }
    }

    debug!(
        "Enrolling TPM 2.0 device for underlying device '{}'",
        device_path.display()
    );

    // TODO: REMOVE BEFORE MERGING
    // Check if it's a valid LUKS2 device
    debug!(
        "Checking if '{}' is a LUKS2 encrypted volume",
        device_path.display()
    );
    let luks_check = encryption::cryptsetup_is_luks(device_path)?;
    debug!(
        "LUKS2 check for '{}' returned: {}",
        device_path.display(),
        luks_check
    );

    // Enroll the TPM 2.0 device for the underlying device. Currently, we bind the enrollment to
    // PCR 7 by default. pcrlock_policy bool is set to false, since while creating encrypted
    // volumes, we first bind to PCR values, not pcrlock policy.
    encryption::systemd_cryptenroll(Some(key_file), device_path, false, pcrs)?;

    debug!(
        "Checking if '{}' is a LUKS2 encrypted volume",
        device_path.display()
    );
    let luks_check = encryption::cryptsetup_is_luks(device_path)?;
    debug!(
        "LUKS2 check for '{}' returned: {}",
        device_path.display(),
        luks_check
    );

    debug!(
        "Opening underlying encrypted device '{}' as '{}'",
        device_path.display(),
        device_name
    );

    encryption::cryptsetup_open(key_file, device_path, device_name)?;

    Ok(())
}

/// This is an abbreviated representation of the JSON output of
/// `cryptsetup luksDump --dump-json-metadata <device_path>`
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
struct LuksDump {
    segments: BTreeMap<String, LuksDumpSegment>,
}

/// This is a complete representation of the segment object in the JSON
/// output of `cryptsetup luksDump --dump-json-metadata <device_path>`
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
struct LuksDumpSegment {
    #[serde(rename = "type")]
    segment_type: String,
    offset: String,
    size: String,
    iv_tweak: String,
    encryption: String,
    sector_size: u64,
}
