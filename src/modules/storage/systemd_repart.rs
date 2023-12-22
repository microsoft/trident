use std::{path::Path, process::Command};

use anyhow::{bail, Context, Error};
use configparser::ini::Ini;
use log::debug;
use osutils::exe::RunAndCheck;
use serde_json::Value;
use std::collections::HashMap;
use tempfile::TempDir;
use trident_api::{
    config::{Disk, Partition, PartitionSize},
    BlockDeviceId,
};
use uuid::Uuid;

pub(super) struct RepartConfiguration {
    repart_root: TempDir,
}
impl RepartConfiguration {
    // Public function new() accepts two arguments:
    // 1. disk, which is the disk to be partitioned,
    // 2. a hashmap of partlabels, which maps partition id to partlabel,
    // for partitions inside volume-pairs. This is needed so that sysupdate
    // correctly interprets which partition to update.
    pub(super) fn new(
        disk: &Disk,
        partlabels: &HashMap<BlockDeviceId, String>,
    ) -> Result<Self, Error> {
        let repart_root = tempfile::tempdir()
            .context("Failed to create temporary directory for systemd-repart files")?;
        debug!(
            "Generating systemd-repart configuration in {}",
            repart_root.path().display()
        );

        let myself = Self { repart_root };

        myself
            .generate_repart_config(disk, partlabels)
            .context(format!(
                "Failed to generate systemd-repart configuration for disk {}",
                disk.id
            ))?;

        Ok(myself)
    }

    fn generate_repart_config(
        &self,
        disk: &Disk,
        partlabels: &HashMap<BlockDeviceId, String>,
    ) -> Result<(), Error> {
        if disk.partitions.len() >= 100 {
            bail!(
                "Too many partitions ({}), maximum is 99",
                disk.partitions.len()
            );
        }

        for (index, partition) in disk.partitions.iter().enumerate() {
            // If there is an entry inside partlabels with partition.id as key,
            // then pass Some(value), which is desired partlabel, to the function as
            // the second arg. Otherwise, pass None.
            let partlabel = partlabels.get(&partition.id);
            let repart_config =
                partition_config_to_repart_config(partition, partlabel).context(format!(
                    "Failed to generate partition configuration for partition {} on disk {}",
                    partition.id, disk.id
                ))?;

            let partition_config_path = self.repart_root.path().join(format!(
                "{:02}-{}.conf",
                index,
                partition.partition_type.to_sdrepart_part_type()
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

    pub(super) fn create_partitions(
        &self,
        disk_bus_path: &Path,
    ) -> Result<Vec<RepartPartition>, Error> {
        let repart_output_json = Command::new("systemd-repart")
            .arg(disk_bus_path.as_os_str())
            .arg("--dry-run=no")
            .arg("--empty=force")
            .arg("--seed=random")
            .arg("--json=short")
            .arg("--definitions")
            .arg(self.repart_root.path())
            .output_and_check()
            .context("Failed to initialize disk")?;
        let partitions_status: Value = serde_json::from_str(repart_output_json.as_str())
            .context("Failed to deserialize output of disk initialization command")?;

        parse_partitions(&partitions_status)
            .context("Failed to parse output of disk initialization command")
    }
}

fn partition_config_to_repart_config(
    partition: &Partition,
    partlabel: Option<&String>,
) -> Result<Ini, Error> {
    let partition_type_str = partition.partition_type.to_sdrepart_part_type();

    let mut repart_config = Ini::new_cs();

    let repart_partition_section = "Partition";

    repart_config.set(
        repart_partition_section,
        "Type",
        Some(partition_type_str.to_string()),
    );

    // If partlabel passed into the func is a valid String, use that
    // as the label for the partition instead
    let label = partlabel.unwrap_or(&partition.id).clone();

    repart_config.set(repart_partition_section, "Label", Some(label));

    // Note: PartitionSize::Grow is the "default" in systemd-repart,
    // so we don't need to set anything. To create a partition with
    // a fixed size, we need to set SizeMinBytes and SizeMaxBytes.
    match partition.size {
        PartitionSize::Grow => {} // Nothing needs to be done here
        PartitionSize::Fixed(size) => {
            repart_config.set(
                repart_partition_section,
                "SizeMinBytes",
                Some(size.to_string()),
            );
            repart_config.set(
                repart_partition_section,
                "SizeMaxBytes",
                Some(size.to_string()),
            );
        }
    }

    Ok(repart_config)
}

pub(super) struct RepartPartition {
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
    use std::str::FromStr;

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

    /// Validates that partition_config_to_repart_config returns the correct Ini for each Partition.
    #[test]
    fn test_partition_config_to_repart_config() {
        let partition = Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::Esp,
            size: PartitionSize::from_str("1M").unwrap(),
        };
        // If partlabel passed into the func is None, set PARTLABEL to
        // partition.id
        let repart_config = partition_config_to_repart_config(&partition, None).unwrap();
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
            "1048576".to_owned()
        );
        assert_eq!(
            repart_config.get("Partition", "SizeMaxBytes").unwrap(),
            "1048576".to_owned()
        );
        // If partlabel passed into the func is a valid String, set PARTLABEL
        // to that String instead
        let repart_config_label =
            partition_config_to_repart_config(&partition, Some(&"_empty".to_owned())).unwrap();
        assert_eq!(
            repart_config_label.get("Partition", "Type").unwrap(),
            "esp".to_owned()
        );
        assert_eq!(
            repart_config_label.get("Partition", "Label").unwrap(),
            "_empty".to_owned()
        );
        assert_eq!(
            repart_config_label
                .get("Partition", "SizeMinBytes")
                .unwrap(),
            "1048576".to_owned()
        );
        assert_eq!(
            repart_config_label
                .get("Partition", "SizeMaxBytes")
                .unwrap(),
            "1048576".to_owned()
        );
    }

    #[test]
    fn test_partition_config_to_repart_config_grow() {
        let partition = Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::LinuxGeneric,
            size: PartitionSize::Grow,
        };
        let repart_config = partition_config_to_repart_config(&partition, None).unwrap();
        assert_eq!(
            repart_config.get("Partition", "Type").unwrap(),
            "linux-generic".to_owned()
        );
        assert_eq!(
            repart_config.get("Partition", "Label").unwrap(),
            "part1".to_owned()
        );
        assert_eq!(repart_config.get("Partition", "SizeMinBytes"), None);
        assert_eq!(repart_config.get("Partition", "SizeMaxBytes"), None);
    }
}

// assumes at least 2 sata connected 16 GB disk setup (and specifically the
// second disk needs to be 16 GiB in size)
#[cfg(all(test, feature = "functional-tests"))]
mod functional_tests {
    use super::*;

    use osutils::lsblk::{self, BlockDevice};
    use osutils::udevadm;
    use std::path::PathBuf;
    use trident_api::config::PartitionType;

    #[test]
    fn test() {
        let unchanged_disk_bus_path = PathBuf::from("/dev/sda");
        let unchanged_block_device_list = lsblk::run(&unchanged_disk_bus_path).unwrap();

        let disk_bus_path = PathBuf::from("/dev/sdb");

        let disk_size: u64 = 17179869184; // 16 GiB
        let part1_size = 50 * 1024 * 1024;

        let golden_disk = Disk {
            id: "disk0".to_owned(),
            device: disk_bus_path.clone(),
            partition_table_type: trident_api::config::PartitionTableType::Gpt,
            partitions: vec![
                Partition {
                    id: "part1".to_owned(),
                    partition_type: PartitionType::Esp,
                    size: PartitionSize::Fixed(part1_size),
                },
                Partition {
                    id: "part2".to_owned(),
                    partition_type: PartitionType::LinuxGeneric,
                    size: PartitionSize::Grow,
                },
            ],
            ..Default::default()
        };
        let mut disk = golden_disk.clone();

        let partlabels = HashMap::new();

        let repart_config = RepartConfiguration::new(&disk, &partlabels).unwrap();

        let partitions = repart_config.create_partitions(&disk_bus_path).unwrap();

        assert_eq!(partitions.len(), 2);

        let part1 = &partitions[0];
        let part1_start = 1024 * 1024;
        assert_eq!(part1.start, part1_start);
        assert_eq!(part1.size, part1_size);

        let part2 = &partitions[1];
        assert_eq!(part2.start, part1_start + part1_size);
        assert_eq!(
            part2.size,
            16 * 1024 * 1024 * 1024 - part1_start - part1_size - 20 * 1024 // 16 GiB disk - 1 MiB prefix - 50 MiB ESP - 20 KiB (rounding?)
        );

        udevadm::settle().unwrap();

        let unchanged_block_device_list_after = lsblk::run(&unchanged_disk_bus_path).unwrap();
        assert_eq!(
            unchanged_block_device_list,
            unchanged_block_device_list_after
        );

        let expected_block_device_list = vec![BlockDevice {
            name: "/dev/sdb".into(),
            part_uuid: None,
            size: disk_size,
            parent_kernel_name: None,
            children: Some(vec![
                BlockDevice {
                    name: "/dev/sdb1".into(),
                    part_uuid: Some(part1.uuid),
                    size: part1.size,
                    parent_kernel_name: Some(PathBuf::from("/dev/sdb")),
                    children: None,
                },
                BlockDevice {
                    name: "/dev/sdb2".into(),
                    part_uuid: Some(part2.uuid),
                    size: part2.size,
                    parent_kernel_name: Some(PathBuf::from("/dev/sdb")),
                    children: None,
                },
            ]),
        }];

        let block_device_list = lsblk::run(&disk_bus_path).unwrap();
        assert_eq!(expected_block_device_list, block_device_list);

        disk.device = PathBuf::from("/dev/null");
        let repart_config = RepartConfiguration::new(&disk, &partlabels).unwrap();
        assert_eq!(repart_config.create_partitions(&disk.device).err().unwrap().root_cause().to_string(), "Process output:\nstderr:\nFailed to open file or determine backing device of /dev/null: Block device required\n\n");

        disk.device = PathBuf::from("/dev/does-not-exist");
        let repart_config = RepartConfiguration::new(&disk, &partlabels).unwrap();
        assert_eq!(repart_config.create_partitions(&disk.device).err().unwrap().root_cause().to_string(), "Process output:\nstderr:\nFailed to open file or determine backing device of /dev/does-not-exist: No such file or directory\n\n");

        let mut disk = golden_disk.clone();
        disk.partitions[1].size = PartitionSize::Fixed(disk_size);
        let repart_config = RepartConfiguration::new(&disk, &partlabels).unwrap();
        assert_eq!(repart_config.create_partitions(&disk.device).err().unwrap().root_cause().to_string(), "Process output:\nstderr:\nCan't fit requested partitions into available free space (15.9G), refusing.\nAutomatically determined minimal disk image size as 16.0G, current image size is 16.0G.\n\n");
    }
}
