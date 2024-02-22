use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Error};
use configparser::ini::Ini;
use serde::Deserialize;
use uuid::Uuid;

use crate::{exe::OutputChecker, partition_types::DiscoverablePartitionType};

/// Representation of a partition created by `systemd-repart`.
#[derive(Debug, Deserialize)]
pub struct RepartPartition {
    /// UUID of the partition
    pub uuid: Uuid,

    /// Offset of the partition in bytes
    #[serde(alias = "offset")]
    pub start: u64,

    /// Size of the partition in bytes
    #[serde(alias = "raw_size")]
    pub size: u64,
}

/// Representation of a partition entry in the `systemd-repart` configuration.
#[derive(Debug, Clone)]
pub struct RepartPartitionEntry {
    pub partition_type: DiscoverablePartitionType,
    pub label: Option<String>,
    pub size_min_bytes: Option<u64>,
    pub size_max_bytes: Option<u64>,
}

impl RepartPartitionEntry {
    /// Create an INI representation of this partition entry.
    fn generate_repart_config(&self) -> Ini {
        let mut repart_config = Ini::new_cs();
        let repart_partition_section = "Partition";

        repart_config.set(
            repart_partition_section,
            "Type",
            Some(self.partition_type.to_str().into()),
        );

        // While set() accepts an Option<String>, we don't want to pass it on directly, because when
        // the value is None, Ini serializes the key without a `=` and no value, which systemd-repart
        // doesn't like. Instead, we only set the value if it's Some.
        if self.label.is_some() {
            repart_config.set(repart_partition_section, "Label", self.label.clone());
        }

        if let Some(size_min_bytes) = self.size_min_bytes {
            repart_config.set(
                repart_partition_section,
                "SizeMinBytes",
                Some(size_min_bytes.to_string()),
            );
        }

        if let Some(size_max_bytes) = self.size_max_bytes {
            repart_config.set(
                repart_partition_section,
                "SizeMaxBytes",
                Some(size_max_bytes.to_string()),
            );
        }

        repart_config
    }
}

/// Possible values for the `--empty` flag of `systemd-repart`.
pub enum RepartMode {
    /// Use `--empty=force` when calling `systemd-repart`.
    Force,

    /// Use `--empty=required` (the default) when calling `systemd-repart`.
    Required,
}

impl RepartMode {
    pub fn to_str(&self) -> &'static str {
        match self {
            RepartMode::Force => "force",
            RepartMode::Required => "required",
        }
    }
}

/// Invokes `systemd-repart` to create partitions on a disk.
pub struct SystemdRepartInvoker {
    disk: PathBuf,
    mode: RepartMode,
    partition_entries: Vec<RepartPartitionEntry>,
}

impl SystemdRepartInvoker {
    pub fn new<S>(disk: S, mode: RepartMode) -> Self
    where
        S: AsRef<Path>,
    {
        Self {
            mode,
            disk: disk.as_ref().to_path_buf(),
            partition_entries: Vec::new(),
        }
    }

    pub fn with_partition_entries(mut self, partition_entries: Vec<RepartPartitionEntry>) -> Self {
        self.partition_entries = partition_entries;
        self
    }

    pub fn push_partition_entry(&mut self, partition_entry: RepartPartitionEntry) {
        self.partition_entries.push(partition_entry);
    }

    pub fn execute(&self) -> Result<Vec<RepartPartition>, Error> {
        self.execute_inner().with_context(|| {
            format!(
                "Failed to execute systemd-repart on {}",
                self.disk.display()
            )
        })
    }

    fn execute_inner(&self) -> Result<Vec<RepartPartition>, Error> {
        let repart_root = tempfile::tempdir()
            .context("Failed to create temporary directory for systemd-repart files")?;

        self.generate_config(repart_root.path())
            .context("Failed to generate systemd-repart config")?;

        let repart_output_json = Command::new("systemd-repart")
            .arg(self.disk.as_os_str())
            .arg("--dry-run=no")
            .arg(format!("--empty={}", self.mode.to_str()))
            .arg("--seed=random")
            .arg("--json=short")
            .arg("--definitions")
            .arg(repart_root.path())
            .output()
            .check_output()
            .context("Failed to execute systemd-repart")?;

        serde_json::from_str(&repart_output_json)
            .context("Failed to deserialize output of systemd-repart")
    }

    fn generate_config(&self, root: &Path) -> Result<(), Error> {
        if self.partition_entries.len() > 1000 {
            bail!(
                "Too many partitions ({}), this library only supports up to 1000 partitions",
                self.partition_entries.len()
            )
        }

        self.partition_entries
            .iter()
            .enumerate()
            .try_for_each(|(index, partition_entry)| {
                let path = root.join(format!("{:03}.conf", index));
                partition_entry
                    .generate_repart_config()
                    .write(&path)
                    .with_context(|| {
                        format!(
                            "Failed to write systemd-repart config ({}) for partition #{} of type {}",
                            path.display(),
                            index,
                            partition_entry.partition_type.to_str()
                        )
                    })
            })
    }
}

#[cfg(test)]
mod tests {
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

        let partitions = serde_json::from_value::<Vec<RepartPartition>>(partitions_status).unwrap();
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

        assert!(serde_json::from_value::<Vec<RepartPartition>>(partition_status).is_err());
    }

    #[test]
    fn test_parse_partition() {
        let partition_status = serde_json::json!({
            "uuid": "123e4567-e89b-12d3-a456-426614174000",
            "offset": 2048,
            "raw_size": 1048576,
        });

        let partition = serde_json::from_value::<RepartPartition>(partition_status).unwrap();
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
        assert!(serde_json::from_value::<RepartPartition>(partition_status).is_err());

        // missing offset
        let partition_status = serde_json::json!({
            "uuid": "123e4567-e89b-12d3-a456-426614174000",
            "raw_size": 1048576,
        });
        assert!(serde_json::from_value::<RepartPartition>(partition_status).is_err());

        // missing raw_size
        let partition_status = serde_json::json!({
            "uuid": "123e4567-e89b-12d3-a456-426614174000",
            "offset": 2048,
        });
        assert!(serde_json::from_value::<RepartPartition>(partition_status).is_err());

        // malformed offset
        let partition_status = serde_json::json!({
            "uuid": "123e4567-e89b-12d3-a456-426614174000",
            "offset": "2048",
            "raw_size": 1048576,
        });
        assert!(serde_json::from_value::<RepartPartition>(partition_status).is_err());
    }

    /// Validates that partition_config_to_repart_config returns the correct Ini for each Partition.
    #[test]
    fn test_partition_config_to_repart_config() {
        let mut partition = RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::Esp,
            label: None,
            size_min_bytes: Some(1048576),
            size_max_bytes: Some(1048576),
        };

        // If partlabel passed into the func is None, set PARTLABEL to
        // partition.id
        let repart_config = partition.generate_repart_config();
        assert_eq!(
            repart_config.get("Partition", "Type").unwrap(),
            "esp".to_owned()
        );

        assert!(repart_config.get("Partition", "Label").is_none());

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
        partition.label = Some("_empty".to_owned());
        let repart_config_label = partition.generate_repart_config();
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
        let partition = RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::LinuxGeneric,
            label: None,
            size_min_bytes: None,
            size_max_bytes: None,
        };

        let repart_config = partition.generate_repart_config();
        assert_eq!(
            repart_config.get("Partition", "Type").unwrap(),
            "linux-generic".to_owned()
        );

        assert!(repart_config.get("Partition", "Label").is_none());

        assert_eq!(repart_config.get("Partition", "SizeMinBytes"), None);

        assert_eq!(repart_config.get("Partition", "SizeMaxBytes"), None);
    }

    #[test]
    fn test_generate_repart_config() {
        let partitions = vec![
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::Esp,
                label: None,
                size_min_bytes: Some(8388608), // 8 MiB
                size_max_bytes: Some(8388608),
            },
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::Esp,
                label: None,
                size_min_bytes: Some(10737418240), // 10 GiB
                size_max_bytes: Some(10737418240),
            },
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::LinuxGeneric,
                label: Some("testpart".into()),
                size_min_bytes: None,
                size_max_bytes: None,
            },
        ];

        let repart_root = tempfile::tempdir()
            .expect("Failed to create temporary directory for systemd-repart files");

        SystemdRepartInvoker::new("/dev/null", RepartMode::Force)
            .with_partition_entries(partitions.clone())
            .generate_config(repart_root.path())
            .expect("Failed to generate systemd-repart config");

        let mut ini_files = std::fs::read_dir(repart_root.path())
            .expect("Failed to read systemd-repart config directory")
            .map(|entry| {
                let entry = entry.expect("Failed to read systemd-repart config directory entry");
                let mut ini = Ini::new_cs();
                ini.load(entry.path()).expect("Failed to load ini file");
                let contents = std::fs::read_to_string(entry.path()).expect("Failed to read file");
                (entry.file_name(), ini, contents)
            })
            .collect::<Vec<_>>();

        // Sort ini files by name, this way it SHOULD match the order in which we added the partitions
        ini_files.sort_by(|a, b| a.0.cmp(&b.0));

        // Check length
        assert_eq!(
            ini_files.len(),
            partitions.len(),
            "Wrong number of ini files"
        );

        partitions.iter().zip(ini_files).enumerate().for_each(
            |(index, (partition, (filename, ini, contents)))| {
                println!("{}:\n{}", filename.to_str().unwrap(), contents);
                assert_eq!(
                    filename.to_str().unwrap(),
                    format!("{:03}.conf", index),
                    "Wrong filename"
                );

                assert_eq!(
                    ini.get("Partition", "Type").unwrap(),
                    partition.partition_type.to_str(),
                    "Type mismatch in {}",
                    filename.to_string_lossy()
                );

                assert_eq!(
                    ini.get("Partition", "Label"),
                    partition.label,
                    "Label mismatch in {}",
                    filename.to_string_lossy()
                );

                // Assert that the Label field is not present if the label is None
                if partition.label.is_none() {
                    assert!(
                        !contents.contains("\nLabel"),
                        "Label field found in {}",
                        filename.to_string_lossy()
                    );
                }

                assert_eq!(
                    ini.get("Partition", "SizeMinBytes"),
                    partition.size_min_bytes.map(|v| v.to_string()),
                    "SizeMinBytes mismatch in {}",
                    filename.to_string_lossy()
                );

                // Assert that the SizeMinBytes field is not present if the label is None
                if partition.size_min_bytes.is_none() {
                    assert!(
                        !contents.contains("\nSizeMinBytes"),
                        "Label field found in {}",
                        filename.to_string_lossy()
                    );
                }

                assert_eq!(
                    ini.get("Partition", "SizeMaxBytes"),
                    partition.size_max_bytes.map(|v| v.to_string()),
                    "SizeMaxBytes mismatch in {}",
                    filename.to_string_lossy()
                );

                // Assert that the SizeMaxBytes field is not present if the label is None
                if partition.size_max_bytes.is_none() {
                    assert!(
                        !contents.contains("\nSizeMaxBytes"),
                        "Label field found in {}",
                        filename.to_string_lossy()
                    );
                }
            },
        );
    }

    #[test]
    fn test_partition_limit() {
        let mut partitions = Vec::new();
        for _ in 0..1001 {
            partitions.push(RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::Esp,
                label: None,
                size_min_bytes: Some(8388608), // 8 MiB
                size_max_bytes: Some(8388608),
            });
        }

        let repart_root = tempfile::tempdir()
            .expect("Failed to create temporary directory for systemd-repart files");

        let result = SystemdRepartInvoker::new("/dev/null", RepartMode::Force)
            .with_partition_entries(partitions)
            .generate_config(repart_root.path());

        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "Too many partitions (1001), this library only supports up to 1000 partitions"
        );
    }
}

// assumes at least 2 sata connected 16 GB disk setup (and specifically the
// second disk needs to be 16 GiB in size)
#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use std::path::PathBuf;

    use pytest_gen::functional_test;

    use crate::lsblk::{self, BlockDevice};
    use crate::udevadm;

    use super::*;

    const DISK_SIZE: u64 = 16 * 1024 * 1024 * 1024; // 16 GiB
    const PART1_SIZE: u64 = 50 * 1024 * 1024; // 50 MiB
    const DISK_BUS_PATH: &str = "/dev/sdb";

    fn generate_partition_definition() -> Vec<RepartPartitionEntry> {
        vec![
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::Esp,
                label: None,
                size_min_bytes: Some(PART1_SIZE),
                size_max_bytes: Some(PART1_SIZE),
            },
            RepartPartitionEntry {
                partition_type: DiscoverablePartitionType::LinuxGeneric,
                label: None,
                // When min==max==None, it's a grow partition
                size_min_bytes: None,
                size_max_bytes: None,
            },
        ]
    }

    #[functional_test(feature = "helpers")]
    fn test_execute_and_resulting_layout() {
        let unchanged_disk_bus_path = PathBuf::from("/dev/sda");
        let unchanged_block_device_list = lsblk::run(&unchanged_disk_bus_path).unwrap();

        let partition_definition = generate_partition_definition();

        let disk_bus_path = PathBuf::from(DISK_BUS_PATH);

        let repart = SystemdRepartInvoker::new(&disk_bus_path, RepartMode::Force)
            .with_partition_entries(partition_definition.clone());

        let partitions = repart.execute().unwrap();

        assert_eq!(partitions.len(), 2);

        let part1 = &partitions[0];
        let part1_start = 1024 * 1024;
        assert_eq!(part1.start, part1_start);
        assert_eq!(part1.size, PART1_SIZE);

        let part2 = &partitions[1];
        assert_eq!(part2.start, part1_start + PART1_SIZE);
        assert_eq!(
            part2.size,
            16 * 1024 * 1024 * 1024 - part1_start - PART1_SIZE - 20 * 1024 // 16 GiB disk - 1 MiB prefix - 50 MiB ESP - 20 KiB (rounding?)
        );

        udevadm::settle().unwrap();

        let unchanged_block_device_list_after = lsblk::run(&unchanged_disk_bus_path).unwrap();
        assert_eq!(
            unchanged_block_device_list,
            unchanged_block_device_list_after
        );

        let expected_block_device_list = vec![BlockDevice {
            name: "/dev/sdb".into(),
            fstype: None,
            fssize: None,
            part_uuid: None,
            size: DISK_SIZE,
            parent_kernel_name: None,
            children: Some(vec![
                BlockDevice {
                    name: "/dev/sdb1".into(),
                    fstype: None,
                    fssize: None,
                    part_uuid: Some(part1.uuid),
                    size: part1.size,
                    parent_kernel_name: Some(PathBuf::from("/dev/sdb")),
                    children: None,
                },
                BlockDevice {
                    name: "/dev/sdb2".into(),
                    fstype: None,
                    fssize: None,
                    part_uuid: Some(part2.uuid),
                    size: part2.size,
                    parent_kernel_name: Some(PathBuf::from("/dev/sdb")),
                    children: None,
                },
            ]),
        }];

        let block_device_list = lsblk::run(&disk_bus_path).unwrap();
        assert_eq!(expected_block_device_list, block_device_list);
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_execute_fails_on_non_block_device() {
        // Test that we can repartition /dev/null
        let repart = SystemdRepartInvoker::new("/dev/null", RepartMode::Force)
            .with_partition_entries(generate_partition_definition());
        assert_eq!(repart.execute().unwrap_err().root_cause().to_string(), "Process output:\nstderr:\nFailed to open file or determine backing device of /dev/null: Block device required\n\n");
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_execute_fails_on_missing_block_device() {
        // Test that we can repartition a non-existing device
        let repart = SystemdRepartInvoker::new("/dev/does-not-exist", RepartMode::Force)
            .with_partition_entries(generate_partition_definition());
        assert_eq!(repart.execute().unwrap_err().root_cause().to_string(), "Process output:\nstderr:\nFailed to open file or determine backing device of /dev/does-not-exist: No such file or directory\n\n");
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_execute_fails_if_partition_size_too_large() {
        // Test that asking for too much space fails
        let mut partition_definition = generate_partition_definition();
        // Make the second partition too big
        partition_definition[1].size_min_bytes = Some(DISK_SIZE);
        partition_definition[1].size_max_bytes = Some(DISK_SIZE);
        let disk_bus_path = PathBuf::from(DISK_BUS_PATH);
        let repart = SystemdRepartInvoker::new(disk_bus_path, RepartMode::Force)
            .with_partition_entries(partition_definition);
        assert_eq!(repart.execute().unwrap_err().root_cause().to_string(), "Process output:\nstderr:\nCan't fit requested partitions into available free space (15.9G), refusing.\nAutomatically determined minimal disk image size as 16.0G, current image size is 16.0G.\n\n");
    }
}
