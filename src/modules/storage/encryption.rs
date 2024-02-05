use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, bail, Context, Error};
use log::{debug, info};
use osutils::exe::RunAndCheck;
use serde::{Deserialize, Serialize};

use trident_api::{
    config::{HostConfiguration, PartitionType},
    status::{BlockDeviceContents, HostStatus},
};

const LUKS_HEADER_SEGMENT_KEY: &str = "0";
const LUKS_HEADER_SIZE_IN_MIB: usize = 16;

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
        } else {
            bail!("Recovery key file URL not specified and recovery key file generation not yet implemented.");
        }
    }

    Ok(())
}

/// This function provisions all configured encrypted volumes.
pub fn provision(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    if let Some(encryption) = &host_config.storage.encryption {
        let key_file: PathBuf = encryption
            .recovery_key_url
            .as_ref()
            .context("Recovery key file generation not yet implemented.")?
            .path()
            .into();

        debug!(
            "Using key file '{}' to initialize all encrypted volume targets",
            key_file.display()
        );

        // Check that the TPM 2.0 device is accessible.
        Command::new("tpm2_pcrread")
            .run_and_check()
            .context("Encryption requires access to a TPM 2.0 device but one is not accessible")?;

        for ev in encryption.volumes.iter() {
            let (target_path, target_content_status, target_size_in_bytes, target_partition_type) =
                if let Some(partition) = host_status
                    .storage
                    .disks
                    .iter_mut()
                    .flat_map(|(_, disk)| disk.partitions.iter_mut())
                    .find(|partition: &&mut trident_api::status::Partition| {
                        partition.id == ev.target_id
                    })
                {
                    info!(
                        "Encrypting underlying partition target '{}' ({}) of encrypted volume '{}'",
                        ev.target_id,
                        partition.path.display(),
                        ev.id
                    );

                    (
                        partition.path.clone(),
                        &mut partition.contents,
                        partition.end - partition.start,
                        partition.ty,
                    )
                } else if let Some(array) = host_status.storage.raid_arrays.get_mut(&ev.target_id) {
                    info!(
                        "Encrypting underlying software RAID array target '{}' ({}) of encrypted volume '{}'",
                        ev.target_id,
                        array.path.display(),
                        ev.id
                    );

                    (
                        array.path.clone(),
                        &mut array.contents,
                        array.array_size,
                        array.partition_type,
                    )
                } else {
                    bail!(format!(
                        "Underlying target '{}' of encrypted volume '{}' is not a partition or software RAID array",
                        ev.target_id,
                        ev.id
                    ))
                };

            // Set the content status of the target to unknown since we
            // are about to encrypt the block device and this may fail.
            *target_content_status = BlockDeviceContents::Unknown;

            encrypt_and_open_target(&target_path, &ev.device_name, &key_file).context(format!(
                "Failed to encrypt and open target '{}' ({}) as {} for volume '{}'",
                target_path.display(),
                ev.target_id,
                ev.device_name,
                ev.id
            ))?;

            // Set the content status of the target to initialized since
            // the block device now contains a valid LUKS volume.
            *target_content_status = BlockDeviceContents::Initialized;

            let header_offset_in_bytes: u64 =
                get_luks_header_offset(&target_path).context(format!(
                    "Failed to get LUKS header offset for target '{}'",
                    target_path.display()
                ))?;

            // Add a representation of the created volume in the host
            // status. The content status is unknown since it is new and
            // there isn't even an empty filesystem on it yet.
            host_status.storage.encrypted_volumes.insert(
                ev.id.clone(),
                trident_api::status::EncryptedVolume {
                    device_name: ev.device_name.clone(),
                    target_path,
                    partition_type: target_partition_type,
                    size: target_size_in_bytes - header_offset_in_bytes,
                    contents: BlockDeviceContents::Unknown,
                },
            );
        }
    }

    Ok(())
}

/// This function encrypts the target of a single encrypted volume by
/// reformatting the target with a LUK2 header, enrolling a key file,
/// enrolling another randomly-generated key and sealing it in the TPM2
/// device with PCR 7, then opening the target as a LUKS2 volume.
fn encrypt_and_open_target(
    target_path: &Path,
    device_name: &String,
    key_file: &Path,
) -> Result<(), Error> {
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
        .arg(target_path.as_os_str())
        .run_and_check()
        .context(format!(
            "Failed to encrypt underlying target '{}'",
            target_path.display()
        ))?;

    debug!(
        "Enrolling TPM2 device for underlying target '{}'",
        target_path.display()
    );

    Command::new("systemd-cryptenroll")
        .arg("--tpm2-device=auto")
        .arg("--tpm2-pcrs=7")
        .arg("--unlock-key-file")
        .arg(key_file.as_os_str())
        .arg("--wipe-slot=tpm2")
        .arg(target_path.as_os_str())
        .run_and_check()
        .context(format!(
            "Failed to enroll TPM2 device for underlying target '{}'",
            target_path.display()
        ))?;

    debug!(
        "Opening underlying encrypted target '{}' as '{}'",
        target_path.display(),
        device_name
    );

    Command::new("cryptsetup")
        .arg("luksOpen")
        .arg("--key-file")
        .arg(key_file.as_os_str())
        .arg(target_path.as_os_str())
        .arg(device_name)
        .run_and_check()
        .context(format!(
            "Failed to open underlying target '{}' as '{}'",
            target_path.display(),
            device_name
        ))?;

    Ok(())
}

/// This is an abbreviated representation of the JSON output of
/// `cryptsetup luksDump --dump-json-metadata <target_path>`
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
struct LuksDump {
    segments: BTreeMap<String, LuksDumpSegment>,
}

/// This is a complete representation of the segment object in the JSON
/// output of `cryptsetup luksDump --dump-json-metadata <target_path>`
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
/// <target_path>` and parses the output and to return the offset of the
/// LUKS2 volume header in bytes.
fn get_luks_header_offset(target_path: &Path) -> Result<u64, Error> {
    let luks_dump_output: String = Command::new("cryptsetup")
        .arg("luksDump")
        .arg("--dump-json-metadata")
        .arg(target_path.as_os_str())
        .output_and_check()?;

    let luks_dump_output: &[u8] = luks_dump_output.as_bytes();

    parse_luks_dump_for_header_offset(luks_dump_output)
}

/// This function parses the JSON output of `cryptsetup luksDump
/// --dump-json-metadata <target_path>` and returns the offset of the
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

pub fn configure(host_status: &mut HostStatus) -> Result<(), Error> {
    let path: PathBuf = PathBuf::from("/etc/crypttab");
    let mut contents: String = String::new();

    for (_id, ev) in host_status.storage.encrypted_volumes.iter() {
        info!(
            "Adding crypttab entry for volume '{}' ({})",
            ev.device_name,
            ev.target_path.display()
        );

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
        match ev.partition_type {
            PartitionType::Swap => contents.push_str(&format!(
                "{}\t{}\t{}\tluks,swap\n",
                ev.device_name,
                ev.target_path.display(),
                "/dev/random"
            )),
            _ => contents.push_str(&format!(
                "{}\t{}\t{}\tluks,tpm2-device=auto\n",
                ev.device_name,
                ev.target_path.display(),
                "none"
            )),
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

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use trident_api::{
        config::{
            Disk, EncryptedVolume, Encryption, Image, ImageFormat, ImageSha256, MountPoint,
            Partition, PartitionSize, Raid, Storage,
        },
        constants,
    };
    use url::Url;

    use crate::modules::storage::tests::get_recovery_key_file;

    use super::*;

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
            raid: Raid { software: vec![] },
            mount_points: vec![
                MountPoint {
                    path: PathBuf::from("/boot/efi"),
                    target_id: "esp".to_string(),
                    filesystem: "fat32".to_string(),
                    options: vec!["defaults".to_owned()],
                },
                MountPoint {
                    path: constants::ROOT_MOUNT_POINT_PATH.into(),
                    target_id: "root".to_string(),
                    filesystem: "ext4".to_string(),
                    options: vec!["defaults".to_owned()],
                },
                MountPoint {
                    path: PathBuf::from("/srv"),
                    target_id: "srv".to_string(),
                    filesystem: "ext4".to_string(),
                    options: vec!["defaults".to_owned()],
                },
            ],
            images: vec![
                Image {
                    url: "file:///trident_cdrom/data/esp.rawzst".into(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZstd,
                    target_id: "esp".to_owned(),
                },
                Image {
                    url: "file:///trident_cdrom/data/root.rawzst".into(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZstd,
                    target_id: "root".to_owned(),
                },
                Image {
                    url: "file:///trident_cdrom/data/srv.rawzst".into(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZstd,
                    target_id: "srv".to_owned(),
                },
            ],
            ab_update: None,
            encryption: Some(Encryption {
                recovery_key_url: Some(Url::from_file_path(recovery_key_file.path()).unwrap()),
                volumes: vec![EncryptedVolume {
                    id: "srv".to_owned(),
                    device_name: "luks-srv".to_owned(),
                    target_id: "srv-enc".to_owned(),
                }],
            }),
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

    // Encryption configuration needs recovery key file specified.
    #[test]
    fn test_validate_host_config_recovery_key_none_fail() {
        let recovery_key_file = get_recovery_key_file();
        let mut host_config = get_host_config(&recovery_key_file);

        host_config
            .storage
            .encryption
            .as_mut()
            .unwrap()
            .recovery_key_url = None;

        assert_eq!(
            validate_host_config(&host_config).unwrap_err().to_string(),
            "Recovery key file URL not specified and recovery key file generation not yet implemented."
        );
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
