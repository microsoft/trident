use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{anyhow, bail, Context, Error};
use log::info;
use osutils::exe::RunAndCheck;
use serde::{Deserialize, Serialize};

use trident_api::{
    config::{HostConfiguration, PartitionType},
    status::{BlockDeviceContents, HostStatus},
};

const LUKS_HEADER_SEGMENT_KEY: &str = "0";
const LUKS_HEADER_SIZE_IN_MIB: usize = 16;

/// This function provisions all configured encrypted volumes.
pub fn provision(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    if let Some(encryption) = &host_config.storage.encryption {
        let key_file: PathBuf = match &encryption.recovery_key_url {
            Some(url) => url.path().into(),
            None => bail!("Recovery key file generation not yet implemented."),
        };

        for ev in encryption.volumes.iter() {
            let (target_path, target_content_status, target_size_in_bytes) = {
                let partition: &mut trident_api::status::Partition = host_status
                    .storage
                    .disks
                    .iter_mut()
                    .flat_map(|(_, disk)| disk.partitions.iter_mut())
                    .find(|partition: &&mut trident_api::status::Partition| {
                        partition.id == ev.target_id
                    })
                    .context(anyhow!(
                        "Failed to find partition for encrypted volume '{}'",
                        ev.id
                    ))?;

                (
                    partition.path.clone(),
                    &mut partition.contents,
                    partition.end - partition.start,
                )
            };

            // Set the content status of the target to unknown since we
            // are about to encrypt the block device and this may fail.
            *target_content_status = BlockDeviceContents::Unknown;

            encrypt_and_open_target(&target_path, &ev.device_name, &key_file).context(format!(
                "Failed to encrypt target '{}' ({}) for volume '{}'",
                target_path.display(),
                ev.target_id,
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
            // there is nothing on it yet.
            host_status.storage.encrypted_volumes.insert(
                ev.id.clone(),
                trident_api::status::EncryptedVolume {
                    device_name: ev.device_name.clone(),
                    target_path,
                    partition_type: PartitionType::LinuxGeneric,
                    size: target_size_in_bytes - header_offset_in_bytes,
                    contents: BlockDeviceContents::Unknown,
                },
            );
        }
    }

    Ok(())
}

/// This function encrypts the target of a single encrypted volume by
/// reformatting it with a LUK2 header, enrolling a key, and opening it as
/// a LUKS volume.
fn encrypt_and_open_target(
    target_path: &Path,
    device_name: &String,
    key_file: &Path,
) -> Result<(), Error> {
    info!(
        "Encrypting target '{}' as '{}'",
        target_path.display(),
        device_name
    );

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
        .run_and_check()?;

    Command::new("cryptsetup")
        .arg("luksOpen")
        .arg("--key-file")
        .arg(key_file.as_os_str())
        .arg(target_path.as_os_str())
        .arg(device_name)
        .run_and_check()?;

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
        contents.push_str(&format!(
            "{}\t{}\t{}\t{}\n",
            ev.device_name,
            ev.target_path.display(),
            "-",
            "luks"
        ));
    }

    // Avoid writing an empty file.
    if !contents.is_empty() {
        info!("Creating /etc/crypttab");
        osutils::files::write_file(path, 0o644, contents.as_bytes()).context(format!(
            "Failed to write /etc/crypttab with contents '{}'",
            contents
        ))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
