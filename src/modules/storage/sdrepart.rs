use std::{path::Path, process::Command};

use anyhow::{bail, Context, Error};
use configparser::ini::Ini;
use log::info;
use serde_json::Value;
use tempfile::TempDir;
use trident_api::config::{Disk, Partition, PartitionType};
use uuid::Uuid;

pub struct RepartConfiguration {
    repart_root: TempDir,
}
impl RepartConfiguration {
    pub fn new(disk: &Disk) -> Result<Self, Error> {
        let repart_root = tempfile::tempdir()
            .context("Failed to create temporary directory for systemd-repart files")?;
        info!(
            "Generating systemd-repart configuration in {}",
            repart_root.path().display()
        );

        let myself = Self { repart_root };

        myself.generate_repart_config(disk).context(format!(
            "Failed to generate systemd-repart configuration for disk {}",
            disk.id
        ))?;

        Ok(myself)
    }

    fn generate_repart_config(&self, disk: &Disk) -> Result<(), Error> {
        if disk.partitions.len() >= 100 {
            bail!(
                "Too many partitions ({}), maximum is 99",
                disk.partitions.len()
            );
        }

        for (index, partition) in disk.partitions.iter().enumerate() {
            let repart_config = partition_config_to_repart_config(partition).context(format!(
                "Failed to generate partition configuration for partition {} on disk {}",
                partition.id, disk.id
            ))?;

            let partition_config_path = self.repart_root.path().join(format!(
                "{:02}-{}.conf",
                index,
                partition_type_to_string(&partition.partition_type)?
            ));

            repart_config
                .write(&partition_config_path)
                .context(format!(
                    "Failed to create partition configuration file {}",
                    partition_config_path.to_string_lossy()
                ))?;
        }

        Ok(())
    }

    pub fn create_partitions(&self, disk_bus_path: &Path) -> Result<Vec<RepartPartition>, Error> {
        let repart_output_json = crate::run_command(
            Command::new("systemd-repart")
                .arg(disk_bus_path.as_os_str())
                .arg("--dry-run=no")
                .arg("--empty=force")
                .arg("--seed=random")
                .arg("--json=short")
                .arg("--definitions")
                .arg(self.repart_root.path()),
        )
        .context("Failed to initialize disk")?;
        let partitions_status: Value = serde_json::from_slice(&repart_output_json.stdout)
            .context("Failed to deserialize output of disk initialization command")?;

        parse_partitions(&partitions_status)
            .context("Failed to parse output of disk initialization command")
    }
}

fn partition_config_to_repart_config(partition: &Partition) -> Result<Ini, Error> {
    let partition_type_str = partition_type_to_string(&partition.partition_type)?;

    // validate the size formatting to ensure it is compatible with what
    // systemd-repart expects, failure during validation will be handled
    // directly by Trident to ensure higher fidelity error messages
    parse_size(&partition.size).context(format!(
        "Failed to parse size ('{}') for partition '{}'",
        partition.size, partition.id
    ))?;

    let mut repart_config = Ini::new_cs();

    let repart_partition_section = "Partition";

    repart_config.set(repart_partition_section, "Type", Some(partition_type_str));
    repart_config.set(
        repart_partition_section,
        "Label",
        Some(partition.id.clone()),
    );

    repart_config.set(
        repart_partition_section,
        "SizeMinBytes",
        Some(partition.size.clone()),
    );
    repart_config.set(
        repart_partition_section,
        "SizeMaxBytes",
        Some(partition.size.clone()),
    );

    Ok(repart_config)
}

fn partition_type_to_string(partition_type: &PartitionType) -> Result<String, Error> {
    serde_json::to_value(partition_type)?
        .as_str()
        .map(|s| s.to_owned())
        .context(format!(
            "Failed to convert partition type {:?} to string",
            partition_type
        ))
}

fn parse_size(value: &str) -> Result<u64, Error> {
    Ok(if let Some(n) = value.strip_suffix('K') {
        n.parse::<u64>()? << 10
    } else if let Some(n) = value.strip_suffix('M') {
        n.parse::<u64>()? << 20
    } else if let Some(n) = value.strip_suffix('G') {
        n.parse::<u64>()? << 30
    } else if let Some(n) = value.strip_suffix('T') {
        n.parse::<u64>()? << 40
    } else {
        value.parse()?
    })
}

pub struct RepartPartition {
    pub uuid: Uuid,
    pub start: u64,
    pub size: u64,
}

fn parse_partitions(partitions_status: &serde_json::Value) -> Result<Vec<RepartPartition>, Error> {
    partitions_status
        .as_array()
        .context("Failed to find partitions")?
        .iter()
        .map(parse_partition)
        .collect()
}

fn parse_partition(partition_status: &serde_json::Value) -> Result<RepartPartition, Error> {
    let partition_uuid_str = partition_status["uuid"]
        .as_str()
        .context("Failed to find UUID for partition")?;
    let uuid = Uuid::parse_str(partition_uuid_str).context(format!(
        "Failed to parse partition UUID: '{}'",
        partition_uuid_str
    ))?;

    let start = partition_status["offset"]
        .as_u64()
        .context("Failed to find start offset for partition")?;
    let size = partition_status["raw_size"]
        .as_u64()
        .context("Failed to find size for partition")?;

    Ok(RepartPartition { uuid, start, size })
}

#[cfg(test)]
mod tests {
    use trident_api::config::{Partition, PartitionType};

    use super::*;

    #[test]
    fn test_parse_partitions() {
        let partitions_status = serde_json::json!([
            {
                "uuid": "123e4567-e89b-12d3-a456-426614174000",
                "offset": 2048,
                "raw_size": 1048576,
            },
            {
                "uuid": "123e4567-e89b-12d3-a456-426614174001",
                "offset": 2049,
                "raw_size": 1048577,
            }
        ]);

        let partitions = parse_partitions(&partitions_status).unwrap();
        assert_eq!(partitions.len(), 2);
        assert_eq!(
            partitions[0].uuid.to_string(),
            "123e4567-e89b-12d3-a456-426614174000"
        );
        assert_eq!(partitions[0].start, 2048);
        assert_eq!(partitions[0].size, 1048576);

        // input is not an array
        let partition_status = serde_json::json!({
            "uuid": "123e4567-e89b-12d3-a456-426614174000",
            "offset": 2048,
            "raw_size": 1048576,
        });

        assert!(parse_partitions(&partition_status).is_err());
    }

    #[test]
    fn test_parse_partition() {
        let partition_status = serde_json::json!({
            "uuid": "123e4567-e89b-12d3-a456-426614174000",
            "offset": 2048,
            "raw_size": 1048576,
        });

        let partition = parse_partition(&partition_status).unwrap();
        assert_eq!(
            partition.uuid.to_string(),
            "123e4567-e89b-12d3-a456-426614174000"
        );
        assert_eq!(partition.start, 2048);
        assert_eq!(partition.size, 1048576);

        // missing uuid
        let partition_status = serde_json::json!({
            "offset": 2048,
            "raw_size": 1048576,
        });
        assert!(parse_partition(&partition_status).is_err());

        // missing offset
        let partition_status = serde_json::json!({
            "uuid": "123e4567-e89b-12d3-a456-426614174000",
            "raw_size": 1048576,
        });
        assert!(parse_partition(&partition_status).is_err());

        // missing raw_size
        let partition_status = serde_json::json!({
            "uuid": "123e4567-e89b-12d3-a456-426614174000",
            "offset": 2048,
        });
        assert!(parse_partition(&partition_status).is_err());

        // malformed offset
        let partition_status = serde_json::json!({
            "uuid": "123e4567-e89b-12d3-a456-426614174000",
            "offset": "2048",
            "raw_size": 1048576,
        });
        assert!(parse_partition(&partition_status).is_err());
    }

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1").unwrap(), 1);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("12G").unwrap(), 12 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("321T").unwrap(), 321 * 1024 * 1024 * 1024 * 1024);

        assert!(parse_size("1Z").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("T1").is_err());
        assert!(parse_size("-3").is_err());
        assert!(parse_size("0x23K").is_err());
    }

    /// Validates that partition_type_to_string returns the correct string for each PartitionType.
    #[test]
    fn test_partition_type_to_string() {
        assert_eq!(
            partition_type_to_string(&PartitionType::Esp).unwrap(),
            "esp"
        );
        assert_eq!(
            partition_type_to_string(&PartitionType::Root).unwrap(),
            "root"
        );
        assert_eq!(
            partition_type_to_string(&PartitionType::RootVerity).unwrap(),
            "root-verity"
        );
        assert_eq!(
            partition_type_to_string(&PartitionType::Swap).unwrap(),
            "swap"
        );
        assert_eq!(
            partition_type_to_string(&PartitionType::Home).unwrap(),
            "home"
        );
    }

    /// Validates that partition_config_to_repart_config returns the correct Ini for each Partition.
    #[test]
    fn test_partition_config_to_repart_config() {
        let partition = Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::Esp,
            size: "1M".to_owned(),
        };
        let repart_config = partition_config_to_repart_config(&partition).unwrap();
        assert_eq!(
            repart_config.get("Partition", "Type").unwrap(),
            "esp".to_owned()
        );
        assert_eq!(
            repart_config.get("Partition", "Label").unwrap(),
            "part1".to_owned()
        );
        assert_eq!(
            repart_config.get("Partition", "SizeMinBytes").unwrap(),
            "1M".to_owned()
        );
        assert_eq!(
            repart_config.get("Partition", "SizeMaxBytes").unwrap(),
            "1M".to_owned()
        );
    }
}
