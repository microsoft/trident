use std::{
    collections::BTreeMap,
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use enumflags2::BitFlags;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use osutils::{
    dependencies::{Dependency, DependencyResultExt},
    encryption,
    lsblk::{self, BlockDeviceType},
    pcr::Pcr,
};
use trident_api::{
    config::{HostConfiguration, HostConfigurationStaticValidationError, Partition, PartitionSize},
    constants::internal_params::{NO_CLOSE_ENCRYPTED_VOLUMES, REENCRYPT_ON_CLEAN_INSTALL},
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
    BlockDeviceId,
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

/// Provisions all configured encrypted volumes.
#[tracing::instrument(name = "encryption_provision", fields(total_partition_size_bytes = tracing::field::Empty), skip_all)]
pub(super) fn provision(
    ctx: &EngineContext,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    if let Some(encryption) = &host_config.storage.encryption {
        let key_file_tmp: NamedTempFile;
        let key_file_path: PathBuf;
        if let Some(recovery_key_url) = &encryption.recovery_key_url {
            key_file_path = recovery_key_url.path().into()
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
            encryption::generate_recovery_key_file(&key_file_path).structured(
                ServicingError::GenerateRecoveryKeyFile {
                    key_file: key_file_path.to_string_lossy().to_string(),
                },
            )?;
        };

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
            let partition = get_first_backing_partition(ctx, &ev.device_id).structured(
                InvalidInputError::from(
                    HostConfigurationStaticValidationError::EncryptedVolumeNotPartitionOrRaid {
                        encrypted_volume: ev.id.clone(),
                    },
                ),
            )?;
            if let PartitionSize::Fixed(byte_count) = partition.size {
                total_partition_size_bytes += byte_count.bytes();
            }
            // TODO: Print the kind of block device that device_id points to. https://dev.azure.com/mariner-org/ECF/_workitems/edit/7323/
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

            // Check if `REENCRYPT_ON_CLEAN_INSTALL` internal param is set to true; if so, re-encrypt
            // the device in-place. Otherwise, initialize a new LUKS2 volume.
            encrypt_and_open_device(
                &device_path,
                &ev.device_name,
                &key_file_path,
                if host_config
                    .internal_params
                    .get_flag(REENCRYPT_ON_CLEAN_INSTALL)
                {
                    EncryptionType::Reencrypt
                } else {
                    EncryptionType::LuksFormat
                },
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
/// This function takes in 4 arguments:
/// - `device_path`: The path to the device to be encrypted.
/// - `device_name`: The name of the device to be used in the crypttab.
/// - `key_file`: The path to the key file to be used for encryption.
/// - `encryption_type`: The type of encryption to be used. Determines whether the device should be
///    re-encrypted in-place, or whether a new LUKS2 volume should be initialized.
fn encrypt_and_open_device(
    device_path: &Path,
    device_name: &String,
    key_file: &Path,
    encryption_type: EncryptionType,
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

    // Enroll the TPM 2.0 device for the underlying device. Currently, we bind the enrollment to
    // PCR 7 by default.
    encryption::systemd_cryptenroll(key_file, device_path, BitFlags::from(Pcr::Pcr7))?;

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

    use std::str::FromStr;

    use trident_api::config::{
        Disk, Partition, PartitionSize, PartitionType, Raid, RaidLevel, SoftwareRaidArray, Storage,
    };

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
}
