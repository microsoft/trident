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
    bootloaders::{BOOT_EFI, GRUB_EFI},
    container,
    dependencies::{Dependency, DependencyResultExt},
    efivar,
    encryption::{self, KeySlotType, ENCRYPTION_PASSPHRASE},
    lsblk::{self, BlockDeviceType},
    path::join_relative,
    pcrlock,
};
use sysdefs::tpm2::Pcr;
use trident_api::{
    config::{HostConfiguration, HostConfigurationStaticValidationError, PartitionSize},
    constants::{
        internal_params::{
            NO_CLOSE_ENCRYPTED_VOLUMES, OVERRIDE_PCRLOCK_ENCRYPTION, REENCRYPT_ON_CLEAN_INSTALL,
        },
        ESP_EFI_DIRECTORY, ESP_MOUNT_POINT_PATH,
    },
    error::{InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::AbVolumeSelection,
};

use crate::{
    bootentries,
    engine::{
        boot::{self, uki},
        storage::encryption::uki::{TMP_UKI_NAME, UKI_DIRECTORY},
        EngineContext,
    },
};

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
        let key_value: Vec<u8>;

        if let Some(recovery_key_url) = &encryption.recovery_key_url {
            key_file_path = recovery_key_url.path().into();

            // Read key from existing recovery key file
            let key = fs::read(&key_file_path).structured(ServicingError::ReadRecoveryKeyFile {
                key_file: key_file_path.to_string_lossy().to_string(),
            })?;
            key_value = key;
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

            key_value = key.clone();
        };

        // Store the key statically for later use, i.e. pcrlock policy enrollment
        let mut static_key = ENCRYPTION_PASSPHRASE.lock().unwrap();
        *static_key = key_value;

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

        // If this is for a grub ROS, seal against the value of PCR 7; if this is for a UKI ROS,
        // seal against a "bootstrapping" pcrlock policy that exclusively contains PCR 0.
        // TODO: If this is a flow with an internal override, seal against the value of PCR 0
        // directly. Remove this internal override once container, BM, and "rerun" E2E encryption
        // tests are fixed. Related ADO tasks:
        // https://dev.azure.com/mariner-org/polar/_workitems/edit/13344/ and
        // https://dev.azure.com/mariner-org/polar/_workitems/edit/14269/.
        let pcr = if ctx.is_uki()? {
            if ctx
                .spec
                .internal_params
                .get_flag(OVERRIDE_PCRLOCK_ENCRYPTION)
            {
                debug!(
                    "Runtime OS image is a UKI image, \
                    but internal override '{OVERRIDE_PCRLOCK_ENCRYPTION}' is set to true, \
                    so sealing against PCR 0"
                );
                Some(BitFlags::from(Pcr::Pcr0))
            } else {
                debug!(
                    "Runtime OS image is a UKI image, so sealing against a pcrlock policy of PCR 0"
                );
                pcrlock::generate_pcrlock_policy(BitFlags::from(Pcr::Pcr0), vec![], vec![])?;
                None
            }
        } else {
            debug!("Runtime OS image is a grub image, so sealing against PCR 7");
            Some(BitFlags::from(Pcr::Pcr7))
        };

        // Check if `REENCRYPT_ON_CLEAN_INSTALL` internal param is set to true; if so, re-encrypt
        // the device in-place. Otherwise, initialize a new LUKS2 volume.
        let encryption_type = if host_config
            .internal_params
            .get_flag(REENCRYPT_ON_CLEAN_INSTALL)
        {
            EncryptionType::Reencrypt
        } else {
            EncryptionType::LuksFormat
        };

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

            encrypt_and_open_device(
                &device_path,
                &ev.device_name,
                &key_file_path,
                encryption_type,
                pcr,
            )
            .structured(ServicingError::EncryptBlockDevice {
                device_path: device_path.to_string_lossy().to_string(),
                device_id: ev.device_id.clone(),
                encrypted_volume_device_name: ev.device_name.clone(),
                encrypted_volume: ev.id.clone(),
            })?;

            // If the key file was randomly generated and NOT provided by the user as a
            // recovery key, remove the password key slot from the encrypted volume, as it's
            // not needed, for security
            if encryption.recovery_key_url.is_none() {
                debug!(
                        "Recovery key file not provided, so removing password key slot from encrypted volume with id '{}'",
                        ev.id
                    );
                encryption::systemd_cryptenroll_wipe_slot(
                    device_path.clone(),
                    KeySlotType::Password,
                )
                .structured(ServicingError::WipePasswordKeySlot {
                    device_path: device_path.to_string_lossy().to_string(),
                })?;
            }
        }
        tracing::Span::current().record("total_partition_size_bytes", total_partition_size_bytes);
    }

    Ok(())
}

/// Encrypts the device of a single encrypted volume by reformatting the device with a LUKS2
/// header, enrolling a key file, enrolling another randomly generated key and sealing it in the
/// TPM 2.0 device, and finally, opening the device as a LUKS2 volume.
///
/// This function takes in 5 arguments:
/// - `device_path`: The path to the device to be encrypted.
/// - `device_name`: The name of the device to be used in the crypttab.
/// - `key_file`: The path to the key file to be used for encryption.
/// - `encryption_type`: The type of encryption to be used. Determines whether the device should be
///   re-encrypted in-place, or whether a new LUKS2 volume should be initialized.
/// - `pcr`: The PCR to seal the key against. This is an optional PCR for scenarios where encrypted
///   volumes are sealed against the value of PCR 7 instead of a pcrlock policy, mainly for the
///   grub MOS + grub ROS flow.
fn encrypt_and_open_device(
    device_path: &Path,
    device_name: &String,
    key_file: &Path,
    encryption_type: EncryptionType,
    pcr: Option<BitFlags<Pcr>>,
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

    // Enroll the TPM 2.0 device for the underlying device
    encryption::systemd_cryptenroll(key_file, device_path, pcr)?;

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

/// Returns paths of UKI and bootloader binaries that `systemd-pcrlock` tool should seal to. During
/// encryption provisioning, returns binaries used for the current boot, as well as binaries that
/// will be used in the future boot, i.e. in the ROS update image. During rollback validation,
/// returns binaries used for the current boot only.
///
/// Returns a tuple containing two vectors:
/// - uki_binaries: Paths to the UKI binaries,
/// - bootloader_binaries: Paths to the bootloader binaries (shim and systemd-boot).
pub fn get_binary_paths_pcrlock(
    ctx: &EngineContext,
    pcrs: BitFlags<Pcr>,
    mount_path: Option<&Path>,
) -> Result<(Vec<PathBuf>, Vec<PathBuf>), Error> {
    // If neither PCR 4 nor 11 are requested, no binaries are needed
    if !pcrs.contains(Pcr::Pcr4) && !pcrs.contains(Pcr::Pcr11) {
        return Ok((vec![], vec![]));
    }

    // Determine esp path depending on the environment
    let esp_path = if container::is_running_in_container()
        .unstructured("Failed to determine if running in container")?
    {
        let host_root =
            container::get_host_root_path().unstructured("Failed to get host root path")?;
        join_relative(host_root, ESP_MOUNT_POINT_PATH)
    } else {
        PathBuf::from(ESP_MOUNT_POINT_PATH)
    };

    // Construct UKI paths
    let uki_binaries = get_uki_paths(&esp_path, mount_path)?;

    // If PCR 4 is requested, construct bootloader paths
    let bootloader_binaries = if pcrs.contains(Pcr::Pcr4) {
        get_bootloader_paths(&esp_path, mount_path, ctx)?
    } else {
        vec![]
    };

    Ok((uki_binaries, bootloader_binaries))
}

/// Returns paths of the UKI binaries for the current boot and if mount_path is provided, for the
/// future boot, i.e. update image, as required for the generation of .pcrlock files.
///
/// If `mount_path` is provided, it means that this func is called during staging of an A/B update,
/// so the UKI binary for the update image is requested. Otherwise, this logic is called on boot
/// validation, and so we're re-generating the pcrlock policy for the current boot only.
fn get_uki_paths(esp_path: &Path, mount_path: Option<&Path>) -> Result<Vec<PathBuf>, Error> {
    let mut uki_binaries: Vec<PathBuf> = Vec::new();

    // If mount_path is null, this logic is called on rollback detection, when active volume is
    // still set to the old volume, so we request UKI suffix for update image, to get it for the
    // current boot. Otherwise, when staging an A/B update, for_update is set to false.
    let esp_uki_directory = join_relative(esp_path, UKI_DIRECTORY);
    let uki_filename =
        efivar::read_current_var().unstructured("Failed to read current boot entry")?;
    let uki_current = esp_uki_directory.join(uki_filename);
    uki_binaries.push(Path::new(&uki_current).to_path_buf());

    // If this is done during encryption provisioning, i.e. update image is mounted at mount_path,
    // we also construct the update UKI binary path
    if mount_path.is_some() {
        // UKI binary in runtime OS to be measured; it's currently staged at designated
        // path
        let uki_update = esp_uki_directory.join(TMP_UKI_NAME);
        uki_binaries.push(uki_update.clone());
    }

    debug!("Paths of UKI binaries required for pcrlock encryption:");
    for (i, path) in uki_binaries.iter().enumerate() {
        debug!("UKI binary {}: {}", i + 1, path.display());
    }

    Ok(uki_binaries)
}

/// Returns paths of the bootloader binaries for the current boot and if mount_path is provided,
/// for the future boot, i.e. update image, as required for the generation of .pcrlock files.
/// Bootloaders include shim and systemd-boot EFI executables.
///
/// If `mount_path` is provided, it means that this func is called during staging of an A/B update,
/// so the bootloader binary for the update image is requested. Otherwise, this logic is called on
/// boot validation, and so we're re-generating the pcrlock policy for the current boot only.
fn get_bootloader_paths(
    esp_path: &Path,
    mount_path: Option<&Path>,
    ctx: &EngineContext,
) -> Result<Vec<PathBuf>, Error> {
    let mut bootloader_binaries: Vec<PathBuf> = Vec::new();

    // If mount_path is null, this logic is called on rollback detection, when active volume is
    // still set to the old volume, so we need to determine the actual active volume
    let active_volume = match mount_path {
        // Currently, not executing pcrlock encryption or this logic on clean install, so active
        // volume has to be non-null on encryption provisioning.
        // TODO: Once pcrlock encryption is enabled on clean install, need to adjust the logic, to
        // correctly construct the binary paths. Related ADO tasks:
        // https://dev.azure.com/mariner-org/polar/_workitems/edit/14286/ and
        // https://dev.azure.com/mariner-org/polar/_workitems/edit/13059/.
        Some(_) => ctx.ab_active_volume.ok_or_else(|| {
            anyhow::anyhow!("Active volume is not set outside of clean install servicing")
        })?,
        None => match ctx.ab_active_volume {
            None | Some(AbVolumeSelection::VolumeB) => AbVolumeSelection::VolumeA,
            Some(AbVolumeSelection::VolumeA) => AbVolumeSelection::VolumeB,
        },
    };

    let esp_dir_name = boot::make_esp_dir_name(ctx.install_index, active_volume);
    let shim_path = Path::new(ESP_EFI_DIRECTORY)
        .join(&esp_dir_name)
        .join(BOOT_EFI);
    let shim_current = join_relative(esp_path, &shim_path);
    bootloader_binaries.push(shim_current);

    // Construct current secondary bootloader path, i.e. systemd-boot EFI executable
    let systemd_boot_path = Path::new(ESP_EFI_DIRECTORY)
        .join(&esp_dir_name)
        .join(GRUB_EFI);
    let systemd_boot_current = join_relative(esp_path, &systemd_boot_path);
    bootloader_binaries.push(systemd_boot_current);

    // If there is mount_path, we are currently staging a clean install or an A/B update, so also
    // construct update paths
    if let Some(mount_path) = mount_path {
        let esp_dir_path = join_relative(mount_path, ESP_MOUNT_POINT_PATH);
        // Primary bootloader, i.e. shim EFI executable, in update image
        let (_, shim_update_relative) = bootentries::get_label_and_path(ctx, BOOT_EFI)?;
        let shim_update = join_relative(esp_dir_path.clone(), shim_update_relative);
        bootloader_binaries.push(shim_update);

        // Secondary bootloader, i.e. systemd-boot EFI executable, in update image
        let (_, systemd_boot_update_relative) = bootentries::get_label_and_path(ctx, GRUB_EFI)?;
        let systemd_boot_update = join_relative(esp_dir_path, systemd_boot_update_relative);
        bootloader_binaries.push(systemd_boot_update);
    }

    debug!("Paths of bootloader binaries required for pcrlock encryption:");
    for (i, path) in bootloader_binaries.iter().enumerate() {
        debug!("Bootloader binary {}: {}", i + 1, path.display());
    }

    Ok(bootloader_binaries)
}
