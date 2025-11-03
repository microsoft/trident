use std::{
    fs::{self, Permissions},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use enumflags2::BitFlags;
use log::{debug, info};
use tempfile::NamedTempFile;

use osutils::{
    bootloaders::{BOOT_EFI, GRUB_EFI},
    container,
    dependencies::{Dependency, DependencyResultExt},
    efivar,
    encryption::{self, KeySlotType},
    lsblk::{self, BlockDeviceType},
    path::join_relative,
    pcrlock,
};
use sysdefs::tpm2::Pcr;
use trident_api::{
    config::{HostConfiguration, HostConfigurationStaticValidationError, PartitionSize},
    constants::{
        internal_params::{NO_CLOSE_ENCRYPTED_VOLUMES, REENCRYPT_ON_CLEAN_INSTALL},
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

        // If optional flag is set to true, clear the TPM 2.0 device to ensure that it is in a
        // known state.
        if encryption.clear_tpm_on_install == Some(true) {
            debug!("Clearing TPM 2.0 device");
            Dependency::Tpm2Clear
                .cmd()
                .run_and_check()
                .message("Failed to clear TPM 2.0 device")?;
        }

        // If this is for a grub ROS, seal against the value of PCR 7; if this is for a UKI ROS,
        // seal against a "bootstrapping" pcrlock policy that exclusively contains PCR 0.
        let pcr = if ctx.is_uki()? {
            debug!("Target OS image is a UKI image, so sealing against a pcrlock policy of PCR 0");

            // Remove any pre-existing policy
            pcrlock::remove_policy().structured(ServicingError::RemovePcrlockPolicy)?;

            // Generate a pcrlock policy for the first time
            pcrlock::generate_pcrlock_policy(BitFlags::from(Pcr::Pcr0), vec![], vec![])?;
            None
        } else {
            debug!("Target OS image is a grub image, so sealing against PCR 7");
            Some(
                encryption
                    .pcrs
                    .iter()
                    .fold(BitFlags::empty(), |acc, &pcr| acc | BitFlags::from(pcr)),
            )
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
///   volumes are sealed against the value of PCR 7 instead of a pcrlock policy, for the grub MOS +
///   grub target OS flow.
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

/// Returns paths of UKI and bootloader binaries that `systemd-pcrlock` tool should seal to. During
/// encryption provisioning, returns binaries used for the current boot, i.e. servicing OS, as well
/// as binaries that will be used in the future boot, i.e. target OS. During boot validation,
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

    // Determine ESP path depending on the environment
    let esp_path = if container::is_running_in_container()
        .unstructured("Failed to determine if running in container")?
    {
        let host_root =
            container::get_host_root_path().unstructured("Failed to get host root path")?;
        join_relative(host_root, ESP_MOUNT_POINT_PATH)
    } else {
        PathBuf::from(ESP_MOUNT_POINT_PATH)
    };

    // If either PCR 4 or PCR 11 is requested, construct UKI paths
    let uki_binaries = get_uki_paths(&esp_path, mount_path)?;

    // If PCR 4 is requested, construct bootloader paths
    let bootloader_binaries = if pcrs.contains(Pcr::Pcr4) {
        get_bootloader_paths(&esp_path, mount_path, ctx)?
    } else {
        vec![]
    };

    // Validate that these paths exist to fail early
    let mut missing_paths = Vec::new();
    for path in uki_binaries.iter().chain(bootloader_binaries.iter()) {
        if !path.exists() {
            missing_paths.push(path.display().to_string());
        }
    }

    // If any are missing, return a single error listing them
    if !missing_paths.is_empty() {
        return Err(anyhow::anyhow!(
            "Following binary paths required for pcrlock encryption do not exist:\n{}",
            missing_paths.join("\n")
        ));
    }

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
        // UKI binary in target OS to be measured; it's currently staged at designated path
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
/// for the target OS, as required for the generation of .pcrlock files. Bootloaders include shim
/// and systemd-boot EFI executables.
///
/// If `mount_path` is provided, it means that this func is called during staging of an A/B update,
/// so the bootloader binary for the target OS is requested. Otherwise, this logic is called on
/// boot validation, and so we're re-generating the pcrlock policy for the current boot only.
fn get_bootloader_paths(
    esp_path: &Path,
    mount_path: Option<&Path>,
    ctx: &EngineContext,
) -> Result<Vec<PathBuf>, Error> {
    let mut bootloader_binaries: Vec<PathBuf> = Vec::new();

    let active_volume = match mount_path {
        // Currently, not executing pcrlock encryption or this logic on clean install, so active
        // volume has to be non-null on encryption provisioning.
        Some(_) => ctx.ab_active_volume.ok_or_else(|| {
            anyhow::anyhow!("Active volume is not set outside of clean install servicing")
        })?,
        // If mount_path is null, this logic is called on rollback detection, when active volume is
        // still set to the old volume, so we need to determine the actual active volume
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

    // If there is mount_path, also construct bootloader paths in the target OS image
    if let Some(mount_path) = mount_path {
        let esp_dir_path = join_relative(mount_path, ESP_MOUNT_POINT_PATH);
        // Primary bootloader, i.e. shim EFI executable, in target OS
        let (_, shim_update_relative) = bootentries::get_label_and_path(ctx, BOOT_EFI)?;
        let shim_update = join_relative(esp_dir_path.clone(), shim_update_relative);
        bootloader_binaries.push(shim_update);

        // Secondary bootloader, i.e. systemd-boot EFI executable, in target OS
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

#[cfg(test)]
mod tests {
    use super::*;

    use trident_api::status::{AbVolumeSelection, ServicingType};

    #[test]
    fn test_get_bootloader_paths() {
        // Declare ESP path; no need to actually write anything as this func only constructs paths.
        let esp_path = PathBuf::from(ESP_MOUNT_POINT_PATH);

        let mut ctx = EngineContext {
            ab_active_volume: None,
            install_index: 0,
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        // Test case #1: Boot validation, so no mount_path. Active volume is None, so we're booting
        // into A for the first time.
        let esp_azla_path = esp_path.join("EFI").join("AZLA");
        let mut expected_paths_a = vec![
            esp_azla_path.join("bootx64.efi"),
            esp_azla_path.join("grubx64.efi"),
        ];
        assert_eq!(
            get_bootloader_paths(&esp_path, None, &ctx).unwrap(),
            expected_paths_a
        );

        // Test case #2: Boot validation, so no mount_path. Active volume is set to B, so we're
        // booting into A.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_bootloader_paths(&esp_path, None, &ctx).unwrap(),
            expected_paths_a
        );

        // Test case #3: Boot validation, so no mount_path. Active volume is set to A, so we're
        // booting into B.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        let esp_azlb_path = esp_path.join("EFI").join("AZLB");
        let mut expected_paths_b = vec![
            esp_azlb_path.join("bootx64.efi"),
            esp_azlb_path.join("grubx64.efi"),
        ];
        assert_eq!(
            get_bootloader_paths(&esp_path, None, &ctx).unwrap(),
            expected_paths_b
        );

        // Test case #4: If active volume is None, but servicing type is not clean install and
        // mount_path is provided, return an error.
        let mount_path = PathBuf::from("/mnt");
        ctx.servicing_type = ServicingType::AbUpdate;
        ctx.ab_active_volume = None;
        assert_eq!(
            get_bootloader_paths(&esp_path, Some(&mount_path), &ctx)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Active volume is not set outside of clean install servicing"
        );

        // Test case #5: Encryption provisioning during A/B update, so mount_path provided. Active
        // volume is A.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        let mount_esp_path = join_relative(&mount_path, &esp_path);
        let mount_esp_azlb_path = mount_esp_path.join("EFI").join("AZLB");
        expected_paths_a.extend([
            mount_esp_azlb_path.join("bootx64.efi"),
            mount_esp_azlb_path.join("grubx64.efi"),
        ]);
        assert_eq!(
            get_bootloader_paths(&esp_path, Some(&mount_path), &ctx).unwrap(),
            expected_paths_a
        );

        // Test case #6: Encryption provisioning during A/B update, so mount_path provided. Active
        // volume is B.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        let mount_esp_azla_path = mount_esp_path.join("EFI").join("AZLA");
        expected_paths_b.extend([
            mount_esp_azla_path.join("bootx64.efi"),
            mount_esp_azla_path.join("grubx64.efi"),
        ]);
        assert_eq!(
            get_bootloader_paths(&esp_path, Some(&mount_path), &ctx).unwrap(),
            expected_paths_b
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;
    use trident_api::status::ServicingType;

    #[functional_test(feature = "helpers")]
    fn test_get_uki_paths() {
        // Declare ESP path; no need to actually write anything as this func only constructs paths.
        let esp_path = PathBuf::from(ESP_MOUNT_POINT_PATH);
        let esp_uki_path = esp_path.join(UKI_DIRECTORY);

        // Test case #1: No mount_path, so only one path is returned, i.e. current entry.
        let current_entry = "CurrentEntry-0.efi";
        let var_name = format!(
            "{}-{}",
            efivar::BOOTLOADER_INTERFACE_GUID,
            efivar::LOADER_ENTRY_SELECTED
        );
        efivar::set_efi_variable(&var_name, &efivar::encode_utf16le(current_entry)).unwrap();

        let expected_paths = vec![esp_uki_path.join(current_entry)];
        assert_eq!(get_uki_paths(&esp_path, None).unwrap(), expected_paths);

        // Test case #2: mount_path provided, so two paths are returned, i.e. current entry and
        // update entry.
        let mount_path = PathBuf::from("/mnt");
        let expected_mount_paths = vec![
            esp_uki_path.join(current_entry),
            esp_uki_path.join(TMP_UKI_NAME),
        ];
        assert_eq!(
            get_uki_paths(&esp_path, Some(&mount_path)).unwrap(),
            expected_mount_paths
        );

        // Unset the current entry
        efivar::set_efi_variable(&var_name, &efivar::encode_utf16le("")).unwrap();
    }

    /// Helper: create dirs and test files at the given paths
    fn create_test_files(paths: &[PathBuf]) {
        for p in paths {
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, b"test-data").unwrap();
        }
    }

    #[functional_test(feature = "helpers")]
    fn test_get_binary_paths_pcrlock() {
        let esp_path = PathBuf::from(ESP_MOUNT_POINT_PATH);
        let esp_uki_path = esp_path.join(UKI_DIRECTORY);

        let mut ctx = EngineContext {
            ab_active_volume: None,
            install_index: 0,
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        // Set up current entry for UKI paths
        let current_entry = "CurrentEntry-test.efi";
        let var_name = format!(
            "{}-{}",
            efivar::BOOTLOADER_INTERFACE_GUID,
            efivar::LOADER_ENTRY_SELECTED
        );
        efivar::set_efi_variable(&var_name, &efivar::encode_utf16le(current_entry)).unwrap();

        // Test case #1: Neither PCR 4 nor 11 requested, so should return two empty vectors.
        let pcrs_none = BitFlags::empty();
        assert_eq!(
            get_binary_paths_pcrlock(&ctx, pcrs_none, None).unwrap(),
            (vec![], vec![])
        );

        // Test case #2: Both PCRs 4 and 11 are requested, but files don't exist, expect error.
        let pcrs = BitFlags::from(Pcr::Pcr4) | BitFlags::from(Pcr::Pcr11);
        let esp_azla_path = esp_path.join("EFI").join("AZLA");
        let expected_paths_a = vec![
            esp_azla_path.join("bootx64.efi"),
            esp_azla_path.join("grubx64.efi"),
        ];
        let uki_path = esp_uki_path.join(current_entry);
        let mut expected_paths = vec![uki_path.clone()];
        expected_paths.extend(expected_paths_a.clone());
        let expected_error_message = format!(
            "Following binary paths required for pcrlock encryption do not exist:\n{}",
            expected_paths
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join("\n")
        );
        assert_eq!(
            get_binary_paths_pcrlock(&ctx, pcrs, None)
                .unwrap_err()
                .to_string(),
            expected_error_message
        );

        // Test case #3: All files exist, should return correct vectors.
        create_test_files(&expected_paths);
        assert_eq!(
            get_binary_paths_pcrlock(&ctx, pcrs, None).unwrap(),
            (vec![uki_path.clone()], expected_paths_a.clone())
        );

        // Test case #4: Only PCR 11 is requested, so should return only UKI paths.
        let pcrs_11 = BitFlags::from(Pcr::Pcr11);
        assert_eq!(
            get_binary_paths_pcrlock(&ctx, pcrs_11, None).unwrap(),
            (vec![uki_path.clone()], vec![])
        );

        // Test case #5: PCRs 4 and 11 requested, mount_path provided, A is active, so should
        // return mounted binaries in B.
        let pcrs = BitFlags::from(Pcr::Pcr4) | BitFlags::from(Pcr::Pcr11);
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        let mount_path = PathBuf::from("/mnt");

        // Construct expected paths
        let esp_azlb_path = esp_path.join("EFI").join("AZLB");
        let mount_esp_azlb_path = join_relative(&mount_path, &esp_azlb_path);
        let expected_uki = vec![uki_path.clone(), esp_uki_path.join(TMP_UKI_NAME)];
        let mut expected_bootloader = expected_paths_a.clone();
        expected_bootloader.extend(vec![
            mount_esp_azlb_path.join("bootx64.efi"),
            mount_esp_azlb_path.join("grubx64.efi"),
        ]);
        let mut expected_paths_mnt = expected_uki.clone();
        expected_paths_mnt.extend(expected_bootloader.clone());
        create_test_files(&expected_paths_mnt);

        assert_eq!(
            get_binary_paths_pcrlock(&ctx, pcrs, Some(&mount_path)).unwrap(),
            (expected_uki, expected_bootloader)
        );

        // Unset the current entry
        efivar::set_efi_variable(&var_name, &efivar::encode_utf16le("")).unwrap();

        // Remove all created files
        expected_paths.extend(expected_paths_mnt.clone());
        for p in expected_paths {
            fs::remove_file(p).unwrap();
        }
    }
}
