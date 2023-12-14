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

        repart_config.set(repart_partition_section, "Label", self.label.clone());

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
                (entry.file_name(), ini)
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
            |(index, (partition, (filename, ini)))| {
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

                assert_eq!(
                    ini.get("Partition", "SizeMinBytes"),
                    partition.size_min_bytes.map(|v| v.to_string()),
                    "SizeMinBytes mismatch in {}",
                    filename.to_string_lossy()
                );

                assert_eq!(
                    ini.get("Partition", "SizeMaxBytes"),
                    partition.size_max_bytes.map(|v| v.to_string()),
                    "SizeMaxBytes mismatch in {}",
                    filename.to_string_lossy()
                );
            },
        );
    }
}
