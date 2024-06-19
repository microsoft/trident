use std::{
    collections::BTreeMap,
    fs::{self, File, Permissions},
    io::Read,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, bail, Context, Error};
use log::{debug, info};
use osutils::exe::RunAndCheck;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use trident_api::{
    config::{HostConfiguration, Partition, PartitionType},
    constants::DEV_MAPPER_PATH,
    status::{BlockDeviceContents, BlockDeviceInfo, HostStatus},
    BlockDeviceId,
};

const LUKS_HEADER_SEGMENT_KEY: &str = "0";
const LUKS_HEADER_SIZE_IN_MIB: usize = 16;
const CRYPTTAB_PATH: &str = "/etc/crypttab";
const TMP_RECOVERY_KEY_SIZE: usize = 64;

pub fn validate_host_config(host_config: &HostConfiguration) -> Result<(), Error> {
    if let Some(encryption) = &host_config.storage.encryption {
        if let Some(recovery_key_url) = &encryption.recovery_key_url {
            let key_file: PathBuf = recovery_key_url.path().into();

            if !key_file.exists() {
                bail!(
                    "Recovery key file '{}' does not exist",
                    key_file.to_string_lossy()
                );
            }
            let key_file_metadata = std::fs::metadata(&key_file).context(format!(
                "Failed to get metadata for recovery key file '{}'",
                key_file.display()
            ))?;

            if key_file_metadata.len() == 0 {
                bail!(
                    "Recovery key file '{}' is empty",
                    key_file.to_string_lossy()
                );
            }

            if !key_file_metadata.is_file() {
                bail!(
                    "Recovery key '{}' is not a file",
                    key_file.to_string_lossy()
                );
            }

            let key_file_perms_mode: u32 = key_file_metadata.permissions().mode();

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
                bail!(
                    "Recovery key file '{}' must not be readable or writable by group or others but has permissions 0o{:03o}",
                    key_file.to_string_lossy(),
                    key_file_perms_mode & 0o777
                );
            }
        }
    }

    Ok(())
}

/// This function provisions all configured encrypted volumes.
#[tracing::instrument(name = "encryption_provision", skip_all)]
pub fn provision(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    if let Some(encryption) = &host_config.storage.encryption {
        let key_file_tmp: NamedTempFile;
        let key_file_path: PathBuf;
        if let Some(recovery_key_url) = &encryption.recovery_key_url {
            key_file_path = recovery_key_url.path().into()
        } else {
            // Create a temporary file to store the recovery key file. The
            // key file will be deleted once the NamedTempFile is out of
            // scope and dropped.
            key_file_tmp = NamedTempFile::new().context("Failed to create recovery key file")?;
            key_file_path = key_file_tmp.path().to_owned();
            fs::set_permissions(&key_file_path, Permissions::from_mode(0o600))
                .context("Failed to set permissions on temporary recovery key file")?;
            generate_recovery_key_file(&key_file_path)
                .context("Failed to generate recovery key file")?;
        };

        debug!(
            "Using key file '{}' to initialize all encrypted volumes",
            key_file_path.display()
        );

        // Check that the TPM 2.0 device is accessible.
        Command::new("tpm2_pcrread")
            .run_and_check()
            .context("Encryption requires access to a TPM 2.0 device but one is not accessible")?;

        // Clear the TPM 2.0 device to ensure that it is in a known state.
        // By clearing the lockout value, this prevents the TPM 2.0 device
        // from being placed into DA lockout mode due to repeated
        // successive provisioning attempts.
        Command::new("tpm2_clear")
            .run_and_check()
            .context("Failed to clear TPM 2.0 device")?;

        for ev in encryption.volumes.iter() {
            // Get the block device indicated by device_id if it is a partition, or the first
            // partition of device_id if it is a RAID array. Or return an error if device_id is
            // neither a partition nor a RAID array.
            let partition = get_first_backing_partition(host_status, &ev.device_id).context(format!(
                "Underlying device of encrypted volume '{}' is not a partition or software RAID array",
                ev.id
            ))?;

            // TODO: Print the kind of block device that device_id points to. https://dev.azure.com/mariner-org/ECF/_workitems/edit/7323/
            info!(
                "Encrypting underlying device '{}' of encrypted volume '{}' of type '{}'",
                ev.device_id,
                ev.id,
                partition.partition_type.to_sdrepart_part_type()
            );

            // Set the content status of the device to unknown since we are about to encrypt the
            // block device and this may fail.
            let device = host_status
                .storage
                .block_devices
                .get_mut(&ev.device_id)
                .context(format!(
                    "Failed to find device '{}' for encrypted volume '{}'",
                    ev.device_id, ev.id
                ))?;

            device.contents = BlockDeviceContents::Unknown;

            encrypt_and_open_device(&device.path, &ev.device_name, &key_file_path).context(
                format!(
                    "Failed to encrypt and open device '{}' ({}) as {} for volume '{}'",
                    device.path.display(),
                    ev.device_id,
                    ev.device_name,
                    ev.id
                ),
            )?;

            // Set the content status of the device to initialized since the block device now
            // contains a valid LUKS volume.
            device.contents = BlockDeviceContents::Initialized;

            let header_offset_in_bytes: u64 =
                get_luks_header_offset(&device.path).context(format!(
                    "Failed to get LUKS header offset for device '{}'",
                    device.path.display()
                ))?;

            // Add a representation of the created volume in the host status. The content status is
            // unknown since it is new and there isn't even an empty filesystem on it yet.
            let size = device.size - header_offset_in_bytes;
            host_status.storage.block_devices.insert(
                ev.id.clone(),
                BlockDeviceInfo {
                    path: Path::new(DEV_MAPPER_PATH).join(&ev.device_name),
                    size,
                    contents: BlockDeviceContents::Unknown,
                },
            );
        }
    }

    Ok(())
}

/// This function encrypts the device of a single encrypted volume by
/// reformatting the device with a LUK2 header, enrolling a key file,
/// enrolling another randomly-generated key and sealing it in the TPM2
/// device with PCR 7, then opening the device as a LUKS2 volume.
fn encrypt_and_open_device(
    device_path: &Path,
    device_name: &String,
    key_file: &Path,
) -> Result<(), Error> {
    // TODO: move to osutils
    Command::new("cryptsetup")
        .arg("reencrypt")
        .arg("--encrypt")
        .arg("--batch-mode")
        .arg("--cipher")
        .arg("aes-xts-plain64")
        .arg("--force-password")
        .arg("--hash")
        .arg("sha512")
        .arg("--iter-time")
        .arg("0")
        .arg("--key-file")
        .arg(key_file.as_os_str())
        .arg("--key-size")
        .arg("512")
        .arg("--key-slot")
        .arg("0")
        .arg("--pbkdf")
        .arg("pbkdf2")
        .arg("--reduce-device-size")
        .arg(format!("{}M", LUKS_HEADER_SIZE_IN_MIB))
        .arg("--type")
        .arg("luks2")
        .arg(device_path.as_os_str())
        .run_and_check()
        .context(format!(
            "Failed to encrypt underlying device '{}'",
            device_path.display()
        ))?;

    debug!(
        "Enrolling TPM2 device for underlying device '{}'",
        device_path.display()
    );

    Command::new("systemd-cryptenroll")
        .arg("--tpm2-device=auto")
        .arg("--tpm2-pcrs=7")
        .arg("--unlock-key-file")
        .arg(key_file.as_os_str())
        .arg("--wipe-slot=tpm2")
        .arg(device_path.as_os_str())
        .run_and_check()
        .context(format!(
            "Failed to enroll TPM2 device for underlying device '{}'",
            device_path.display()
        ))?;

    debug!(
        "Opening underlying encrypted device '{}' as '{}'",
        device_path.display(),
        device_name
    );

    Command::new("cryptsetup")
        .arg("luksOpen")
        .arg("--key-file")
        .arg(key_file.as_os_str())
        .arg(device_path.as_os_str())
        .arg(device_name)
        .run_and_check()
        .context(format!(
            "Failed to open underlying device '{}' as '{}'",
            device_path.display(),
            device_name
        ))?;

    Ok(())
}

/// This function creates a file at the specified path and fills it with
/// cryptographically secure random bytes sourced from `/dev/random`. It
/// is intended for generating a recovery key file with a specified size
/// defined by `TMP_RECOVERY_KEY_SIZE`.
///
/// `path` is a reference to a `Path` object that specifies the location
/// and name of the file to be created. This path should be accessible and
/// writable by the process.
///
/// This function can return an error if opening or reading `/dev/random`
/// fails. It can also error when writing to the specified file path
/// fails, which could be due to permission issues, non-existent
/// directories in the path, or other filesystem-related errors.
pub fn generate_recovery_key_file(path: &Path) -> Result<(), Error> {
    let mut random_file: File = File::open("/dev/random").context("Failed to open /dev/random")?;
    let mut random_buffer: [u8; TMP_RECOVERY_KEY_SIZE] = [0u8; TMP_RECOVERY_KEY_SIZE];
    random_file
        .read_exact(&mut random_buffer)
        .context("Failed to read from /dev/random")?;
    fs::write(path, random_buffer).context(format!(
        "Failed to write random data to recovery key file '{}'",
        path.display()
    ))
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

/// This function runs `cryptsetup luksDump --dump-json-metadata
/// <device_path>` and parses the output and to return the offset of the
/// LUKS2 volume header in bytes.
fn get_luks_header_offset(device_path: &Path) -> Result<u64, Error> {
    let luks_dump_output: String = Command::new("cryptsetup")
        .arg("luksDump")
        .arg("--dump-json-metadata")
        .arg(device_path.as_os_str())
        .output_and_check()?;

    let luks_dump_output: &[u8] = luks_dump_output.as_bytes();

    parse_luks_dump_for_header_offset(luks_dump_output)
}

/// This function parses the JSON output of `cryptsetup luksDump
/// --dump-json-metadata <device_path>` and returns the offset of the
/// LUKS2 volume header in bytes.
fn parse_luks_dump_for_header_offset(luks_dump_output: &[u8]) -> Result<u64, Error> {
    let luks_dump: LuksDump = serde_json::from_slice::<LuksDump>(luks_dump_output)
        .context("Failed to parse string as a LUKS dump JSON object")?;

    let offset = luks_dump
        .segments
        .get(LUKS_HEADER_SEGMENT_KEY)
        .context(anyhow!(
            "Failed to find segment '{}' in LUKS dump JSON object",
            LUKS_HEADER_SEGMENT_KEY
        ))?
        .offset
        .as_str();

    offset
        .parse::<u64>()
        .context(anyhow!("Failed to parse offset '{}' as u64", offset))
}

#[tracing::instrument(name = "encryption_configure", skip_all)]
pub fn configure(host_status: &mut HostStatus) -> Result<(), Error> {
    let path: PathBuf = PathBuf::from(CRYPTTAB_PATH);
    let mut contents: String = String::new();

    let Some(ref encryption) = host_status.spec.storage.encryption else {
        return Ok(());
    };

    for ev in encryption.volumes.iter() {
        let backing_partition = get_first_backing_partition(host_status, &ev.device_id).context(format!(
            "Underlying device '{}' of encrypted volume '{}' is not a partition or software RAID array",
            ev.device_id,
            ev.id
        ))?;
        let device_path = &host_status.storage.block_devices.get(&ev.device_id).context(format!(
            "Failed to find block device information for underlying device '{}' of encrypted volume '{}'",
            ev.device_id,
            ev.id
        ))?.path;

        // An encrypted swap device is special-cased in the crypttab due
        // to the unique nature and requirements of swap spaces in a Linux
        // system. It often contains sensitive data temporarily stored in
        // RAM, so encrypting it is crucial for security, and unlike
        // regular partitions, which uses a TPM2.0 device for passwordless
        // startup, the swap device is completely wiped and formatted on
        // each system startup. For systemd to do this, it needs a key,
        // and here in the crypttab the swap device is configured with a
        // randomly generated key from `/dev/random`. This is the most
        // reliable way to generated a truly random key on Linux systems.
        // Since the key that is used to open the swap deivce is
        // immediately discarded, this process also ensures that data left
        // in swap isn't recoverable after a reboot, enhancing security.
        if backing_partition.partition_type == PartitionType::Swap {
            contents.push_str(&format!(
                "{}\t{}\t{}\tluks,swap\n",
                ev.device_name,
                device_path.display(),
                "/dev/random"
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
            std::fs::remove_file(&path).context("Failed to remove crypttab")?;
        }
    } else {
        debug!("crypttab file contents:\n{contents}");
        osutils::files::write_file(path, 0o644, contents.as_bytes())
            .context("Failed to create crypttab")?;
    }

    Ok(())
}

/// Returns the first partition that backs the given block device, or Err if the block device ID
/// does not correspond to a partition or software RAID array.
fn get_first_backing_partition<'a>(
    host_status: &'a HostStatus,
    block_device_id: &BlockDeviceId,
) -> Result<&'a Partition, Error> {
    if let Some(partition) = host_status.spec.storage.get_partition(block_device_id) {
        Ok(partition)
    } else if let Some(array) = host_status
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

        host_status
            .spec
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
    use std::{os::unix::fs::PermissionsExt, str::FromStr};

    use trident_api::{
        config::{
            Disk, EncryptedVolume, Encryption, FileSystemType, ImageFormat, ImageSha256,
            InternalImage, InternalMountPoint, Partition, PartitionSize, PartitionType, Raid,
            RaidLevel, SoftwareRaidArray, Storage,
        },
        constants,
    };
    use url::Url;

    use crate::modules::storage::tests::get_recovery_key_file;

    use super::*;

    #[test]
    fn test_get_first_backing_partition() {
        let host_status = HostStatus {
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
            get_first_backing_partition(&host_status, &"esp".to_owned()).unwrap(),
            &host_status.spec.storage.disks[0].partitions[0]
        );
        assert_eq!(
            get_first_backing_partition(&host_status, &"root".to_owned()).unwrap(),
            &host_status.spec.storage.disks[0].partitions[1]
        );
        assert_eq!(
            get_first_backing_partition(&host_status, &"rootb".to_owned()).unwrap(),
            &host_status.spec.storage.disks[0].partitions[2]
        );
        assert_eq!(
            get_first_backing_partition(&host_status, &"root-raid1".to_owned()).unwrap(),
            &host_status.spec.storage.disks[0].partitions[1]
        );
        get_first_backing_partition(&host_status, &"os".to_owned()).unwrap_err();
        get_first_backing_partition(&host_status, &"non-existant".to_owned()).unwrap_err();
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
            internal_images: vec![
                InternalImage {
                    url: "file:///trident_cdrom/data/esp.rawzst".into(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                    target_id: "esp".to_owned(),
                },
                InternalImage {
                    url: "file:///trident_cdrom/data/root.rawzst".into(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                    target_id: "root".to_owned(),
                },
                InternalImage {
                    url: "file:///trident_cdrom/data/srv.rawzst".into(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                    target_id: "srv".to_owned(),
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
        let recovery_key_file = get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);
        validate_host_config(&host_config).unwrap();
    }

    // Encryption doesn't need to be configured at all.
    #[test]
    fn test_validate_host_config_encryption_none_pass() {
        let recovery_key_file = get_recovery_key_file();
        let mut host_config = get_host_config(&recovery_key_file);

        host_config.storage.encryption = None;

        validate_host_config(&host_config).unwrap();
    }

    // Encryption recovery key file needs to exist on the system.
    #[test]
    fn test_validate_host_config_recovery_key_not_exist_fail() {
        let recovery_key_file = get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        // Delete the recovery key file.
        std::fs::remove_file(recovery_key_file.path()).unwrap();

        assert_eq!(
            validate_host_config(&host_config).unwrap_err().to_string(),
            format!(
                "Recovery key file '{}' does not exist",
                recovery_key_file.path().display()
            )
        );
    }

    // Encryption needs recovery key url to point to a file.
    #[test]
    fn test_validate_host_config_recovery_key_not_file_fail() {
        let recovery_key_file = get_recovery_key_file();
        let mut host_config = get_host_config(&recovery_key_file);
        let encryption = host_config.storage.encryption.as_mut().unwrap();

        // Point to the recovery key file's directory.
        let recovery_key_dir: &Path = recovery_key_file.path().parent().unwrap();
        encryption.recovery_key_url = Some(Url::from_directory_path(recovery_key_dir).unwrap());

        assert_eq!(
            validate_host_config(&host_config).unwrap_err().to_string(),
            format!(
                "Recovery key '{}/' is not a file",
                recovery_key_dir.display()
            )
        );
    }

    // Encryption needs recovery key url to point to a file that is only accessible by the owner.
    #[test]
    fn test_validate_host_config_recovery_key_perm_pass() {
        let recovery_key_file = get_recovery_key_file();
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
                    std::fs::set_permissions(recovery_key_file.path(), perms).unwrap();

                    validate_host_config(&host_config).unwrap();
                }
            }
        }
    }

    #[test]
    fn test_validate_host_config_recovery_key_perm_fail() {
        let recovery_key_file = get_recovery_key_file();
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
                    std::fs::set_permissions(recovery_key_file.path(), perms).unwrap();

                    assert_eq!(
                        validate_host_config(&host_config).unwrap_err().to_string(),
                        format!(
                            "Recovery key file '{}' must not be readable or writable by group or others but has permissions 0o{:03o}",
                            recovery_key_file.path().display(),
                            mode
                        )
                    );
                }
            }
        }
    }

    #[test]
    fn test_validate_host_config_recovery_key_empty_fail() {
        let recovery_key_file = get_recovery_key_file();
        let host_config = get_host_config(&recovery_key_file);

        // Set the recovery key file's contents to empty.
        std::fs::write(recovery_key_file.path(), "").unwrap();

        assert_eq!(
            validate_host_config(&host_config).unwrap_err().to_string(),
            format!(
                "Recovery key file '{}' is empty",
                recovery_key_file.path().display()
            )
        );
    }

    #[test]
    fn test_parse_luks_dump_for_header_offset_str_16mib_pass() {
        let luks_dump_output: &[u8] = r#"
        {
            "keyslots": {},
            "tokens": {},
            "segments": {
                "0": {
                    "type": "crypt",
                    "offset": "16777216",
                    "size": "dynamic",
                    "iv_tweak": "0",
                    "encryption": "aes-xts-plain64",
                    "sector_size": 512
                }
            },
            "digests": {},
            "config": {}
        }
        "#
        .as_bytes();
        let offset: u64 = parse_luks_dump_for_header_offset(luks_dump_output).unwrap();
        assert_eq!(offset, 16777216);
    }

    #[test]
    fn test_parse_luks_dump_for_header_offset_str_zero_pass() {
        let luks_dump_output: &[u8] = r#"
        {
            "keyslots": {},
            "tokens": {},
            "segments": {
                "0": {
                    "type": "crypt",
                    "offset": "0",
                    "size": "dynamic",
                    "iv_tweak": "0",
                    "encryption": "aes-xts-plain64",
                    "sector_size": 512
                }
            },
            "digests": {},
            "config": {}
        }
        "#
        .as_bytes();
        let offset: u64 = parse_luks_dump_for_header_offset(luks_dump_output).unwrap();
        assert_eq!(offset, 0);
    }

    #[test]
    fn test_parse_luks_dump_for_header_offset_str_negative_fail() {
        let luks_dump_output: &[u8] = r#"
        {
            "keyslots": {},
            "tokens": {},
            "segments": {
                "0": {
                    "type": "crypt",
                    "offset": "-1",
                    "size": "dynamic",
                    "iv_tweak": "0",
                    "encryption": "aes-xts-plain64",
                    "sector_size": 512
                }
            },
            "digests": {},
            "config": {}
        }
        "#
        .as_bytes();
        assert_eq!(
            parse_luks_dump_for_header_offset(luks_dump_output)
                .unwrap_err()
                .to_string(),
            "Failed to parse offset '-1' as u64"
        );
    }

    #[test]
    fn test_parse_luks_dump_for_header_offset_str_non_numeric_fail() {
        let luks_dump_output: &[u8] = r#"
        {
            "keyslots": {},
            "tokens": {},
            "segments": {
                "0": {
                    "type": "crypt",
                    "offset": "foo",
                    "size": "dynamic",
                    "iv_tweak": "0",
                    "encryption": "aes-xts-plain64",
                    "sector_size": 512
                }
            },
            "digests": {},
            "config": {}
        }
        "#
        .as_bytes();
        assert_eq!(
            parse_luks_dump_for_header_offset(luks_dump_output)
                .unwrap_err()
                .to_string(),
            "Failed to parse offset 'foo' as u64"
        );
    }

    #[test]
    fn test_parse_luks_dump_for_header_offset_uint_fail() {
        let luks_dump_output: &[u8] = r#"
        {
            "keyslots": {},
            "tokens": {},
            "segments": {
                "0": {
                    "type": "crypt",
                    "offset": 16777216,
                    "size": "dynamic",
                    "iv_tweak": "0",
                    "encryption": "aes-xts-plain64",
                    "sector_size": 512
                }
            },
            "digests": {},
            "config": {}
        }
        "#
        .as_bytes();
        assert_eq!(
            parse_luks_dump_for_header_offset(luks_dump_output)
                .unwrap_err()
                .to_string(),
            "Failed to parse string as a LUKS dump JSON object"
        );
    }

    #[test]
    fn test_parse_luks_dump_for_header_offset_missing_fail() {
        let luks_dump_output: &[u8] = r#"
        {
            "keyslots": {},
            "tokens": {},
            "segments": {
                "0": {
                    "type": "crypt",
                    "size": "dynamic",
                    "iv_tweak": "0",
                    "encryption": "aes-xts-plain64",
                    "sector_size": 512
                }
            },
            "digests": {},
            "config": {}
        }
        "#
        .as_bytes();
        assert_eq!(
            parse_luks_dump_for_header_offset(luks_dump_output)
                .unwrap_err()
                .to_string(),
            "Failed to parse string as a LUKS dump JSON object"
        );
    }

    #[test]
    fn test_luks_dump_parse_header_segment_missing_fail() {
        let luks_dump_output: &[u8] = r#"
        {
            "keyslots": {},
            "tokens": {},
            "segments": {
                "1": {
                    "type": "crypt",
                    "offset": "16777216",
                    "size": "dynamic",
                    "iv_tweak": "0",
                    "encryption": "aes-xts-plain64",
                    "sector_size": 512
                }
            },
            "digests": {},
            "config": {}
        }
        "#
        .as_bytes();
        assert_eq!(
            parse_luks_dump_for_header_offset(luks_dump_output)
                .unwrap_err()
                .to_string(),
            "Failed to find segment '0' in LUKS dump JSON object"
        );
    }

    #[test]
    fn test_luks_dump_parse_header_no_segments_fail() {
        let luks_dump_output: &[u8] = r#"
        {
            "keyslots": {},
            "tokens": {},
            "segments": {},
            "digests": {},
            "config": {}
        }
        "#
        .as_bytes();
        assert_eq!(
            parse_luks_dump_for_header_offset(luks_dump_output)
                .unwrap_err()
                .to_string(),
            "Failed to find segment '0' in LUKS dump JSON object"
        );
    }

    #[test]
    fn test_luks_dump_parse_header_segments_missing_fail() {
        let luks_dump_output: &[u8] = r#"
        {
            "keyslots": {},
            "tokens": {},
            "digests": {},
            "config": {}
        }
        "#
        .as_bytes();
        assert_eq!(
            parse_luks_dump_for_header_offset(luks_dump_output)
                .unwrap_err()
                .to_string(),
            "Failed to parse string as a LUKS dump JSON object"
        );
    }
}
