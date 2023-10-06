use std::{path::Path, process::Command};

use anyhow::{bail, Context, Error};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, PartialEq)]
pub(super) struct SfDisk {
    pub uuid: Uuid,
    pub capacity: u64,
}

pub(super) fn get_disk_information(disk_bus_path: &Path) -> Result<SfDisk, Error> {
    let sfdisk_output_json = crate::run_command(
        Command::new("sfdisk")
            .arg("-J")
            .arg(disk_bus_path.as_os_str()),
    )
    .context("Failed to fetch disk information")?;

    parse_disk(sfdisk_output_json.stdout.as_slice()).context("Failed to extract disk information")
}

fn parse_disk(sfdisk_output_json: &[u8]) -> Result<SfDisk, Error> {
    let disk_status: Value = serde_json::from_slice(sfdisk_output_json)
        .context("Failed to deserialize output of disk status querying command")?;

    let disk_uuid_str = disk_status["partitiontable"]["id"]
        .as_str()
        .context("Failed to find GPT UUID")?;
    let uuid = Uuid::parse_str(disk_uuid_str)
        .context(format!("Failed to parse disk UUID: '{}'", disk_uuid_str))?;

    let lastlba = disk_status["partitiontable"]["lastlba"]
        .as_u64()
        .context("Failed to find disk lastlba")?;
    let firstlba = disk_status["partitiontable"]["firstlba"]
        .as_u64()
        .context("Failed to find disk firstlba")?;
    let sectorsize = disk_status["partitiontable"]["sectorsize"]
        .as_u64()
        .context("Failed to find disk sectorsize")?;
    let unit = disk_status["partitiontable"]["unit"]
        .as_str()
        .context("Failed to find disk unit")?;

    if unit != "sectors" {
        bail!("Unexpected disk unit: '{}'", unit);
    }

    let capacity = (lastlba - firstlba + 1) * sectorsize;

    Ok(SfDisk { uuid, capacity })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_disk() {
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "id": "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2a3b4c5d",
                "firstlba": 2048,
                "lastlba": 67108830,
                "sectorsize": 512,
                "unit": "sectors"
            }
        }"#;
        let disk_uuid = parse_disk(sfdisk_output_json.as_bytes()).unwrap();
        assert_eq!(
            disk_uuid,
            SfDisk {
                uuid: Uuid::parse_str("a1b2c3d4-e5f6-4a5b-8c9d-0e1f2a3b4c5d").unwrap(),
                capacity: 34358672896,
            }
        );

        // malformed UUID
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "id": "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2ac5d",
                "firstlba": 2048,
                "lastlba": 67108830,
                "sectorsize": 512,
                "unit": "sectors"
            }
        }"#;

        assert!(parse_disk(sfdisk_output_json.as_bytes()).is_err());

        // missing firstlba
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "id": "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2ac5d",
                "lastlba": 67108830,
                "sectorsize": 512,
                "unit": "sectors"
            }
        }"#;

        assert!(parse_disk(sfdisk_output_json.as_bytes()).is_err());

        // missing lastlba
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "id": "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2ac5d",
                "firstlba": 2048,
                "sectorsize": 512,
                "unit": "sectors"
            }
        }"#;

        assert!(parse_disk(sfdisk_output_json.as_bytes()).is_err());

        // missing sector size
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "id": "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2ac5d",
                "firstlba": 2048,
                "lastlba": 67108830,
                "unit": "sectors"
            }
        }"#;

        assert!(parse_disk(sfdisk_output_json.as_bytes()).is_err());

        // missing unit
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "id": "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2ac5d",
                "firstlba": 2048,
                "lastlba": 67108830,
                "sectorsize": 512
            }
        }"#;

        assert!(parse_disk(sfdisk_output_json.as_bytes()).is_err());

        // unsuported unit
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "id": "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2ac5d",
                "firstlba": 2048,
                "lastlba": 67108830,
                "sectorsize": 512,
                "unit": "bytes"
            }
        }"#;

        assert!(parse_disk(sfdisk_output_json.as_bytes()).is_err());
    }
}
