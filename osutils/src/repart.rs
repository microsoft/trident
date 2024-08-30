use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, ensure, Context, Error, Ok};
use configparser::ini::Ini;
use serde::Deserialize;
use uuid::Uuid;

use crate::{exe::RunAndCheck, partition_types::DiscoverablePartitionType};

/// Representation of a partition created by `systemd-repart`.
#[derive(Debug, Deserialize)]
pub struct RepartPartition {
    /// Type of the partition
    #[serde(alias = "type")]
    pub partition_type: DiscoverablePartitionType,

    /// Label of the partition
    pub label: Option<String>,

    /// UUID of the partition
    pub uuid: Uuid,

    /// Definition file of the partition
    pub file: PathBuf,

    /// Node of the partition
    pub node: PathBuf,

    /// Offset of the partition in bytes
    #[serde(alias = "offset")]
    pub start: u64,

    /// Size of the partition in bytes
    #[serde(alias = "raw_size")]
    pub size: u64,

    /// systemd-repart's activity
    pub activity: RepartActivity,

    /// Internal ID used only for tracking, not used by `systemd-repart`.
    /// It will match the ID of the partition entry in the configuration.
    #[serde(skip)]
    pub id: String,
}

/// Representation of a partition created by `systemd-repart`.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum RepartActivity {
    /// The partition was created
    /// https://github.com/systemd/systemd/blob/762412f/src/partition/repart.c#L3037
    Create,

    /// The partition was unchanged
    /// https://github.com/systemd/systemd/blob/762412f/src/partition/repart.c#L3080
    Unchanged,

    /// The partition was resized
    /// https://github.com/systemd/systemd/blob/762412f/src/partition/repart.c#L3039
    Resize,
}

impl RepartPartition {
    pub fn is_new(&self) -> bool {
        matches!(self.activity, RepartActivity::Create)
    }

    pub fn path_by_uuid(&self) -> PathBuf {
        Path::new("/dev/disk/by-partuuid").join(self.uuid.hyphenated().to_string())
    }
}

/// Representation of a partition entry in the `systemd-repart` configuration.
#[derive(Debug, Clone)]
pub struct RepartPartitionEntry {
    /// Internal ID used only for tracking, not used by `systemd-repart`.
    pub id: String,

    /// Type of the partition to be passed to `systemd-repart`.
    pub partition_type: DiscoverablePartitionType,

    /// Label of the partition to be passed to `systemd-repart`.
    pub label: Option<String>,

    /// Minimum size of the partition in bytes to be passed to `systemd-repart`.
    pub size_min_bytes: Option<u64>,

    /// Maximum size of the partition in bytes to be passed to `systemd-repart`.
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
pub enum RepartEmptyMode {
    /// Use `--empty=refuse` when calling `systemd-repart`.
    /// > The command requires that the block device it shall operate on already
    /// > carries a partition table and refuses operation if none is found
    Refuse,

    /// Use `--empty=allow` when calling `systemd-repart`.
    /// > The command will extend an existing partition table or create a new
    /// > one if none exists.
    Allow,

    /// Use `--empty=require` (the default) when calling `systemd-repart`.
    /// > The command will create a new partition table if none exists so far,
    /// > and refuse operation if one already exists.
    Require,

    /// Use `--empty=force` when calling `systemd-repart`.
    /// > The command will create a fresh partition table unconditionally,
    /// > erasing the disk fully in effect. No existing partitions will be taken
    /// > into account or survive the operation.
    Force,

    /// Use `--empty=create` when calling `systemd-repart`.
    /// > A new loopback file is create under the path passed via the device
    /// > node parameter
    Create,
}

impl RepartEmptyMode {
    pub fn to_str(&self) -> &'static str {
        match self {
            RepartEmptyMode::Refuse => "refuse",
            RepartEmptyMode::Allow => "allow",
            RepartEmptyMode::Require => "require",
            RepartEmptyMode::Force => "force",
            RepartEmptyMode::Create => "create",
        }
    }
}

/// Invokes `systemd-repart` to create partitions on a disk.
pub struct SystemdRepartInvoker {
    disk: PathBuf,
    mode: RepartEmptyMode,
    partition_entries: Vec<RepartPartitionEntry>,
}

impl SystemdRepartInvoker {
    /// Create a new `SystemdRepartInvoker` for the given disk.
    pub fn new<S>(disk: S, mode: RepartEmptyMode) -> Self
    where
        S: AsRef<Path>,
    {
        Self {
            mode,
            disk: disk.as_ref().to_path_buf(),
            partition_entries: Vec::new(),
        }
    }

    /// Set the partition entries for the `systemd-repart` invocation.
    pub fn with_partition_entries(mut self, partition_entries: Vec<RepartPartitionEntry>) -> Self {
        self.partition_entries = partition_entries;
        self
    }

    /// Set the empty mode for the `systemd-repart` invocation.
    pub fn set_empty_mode(&mut self, mode: RepartEmptyMode) {
        self.mode = mode;
    }

    /// Add a partition entry to the `systemd-repart` invocation.
    pub fn push_partition_entry(&mut self, partition_entry: RepartPartitionEntry) {
        self.partition_entries.push(partition_entry);
    }

    /// Get current partition entries.
    pub fn partition_entries(&self) -> &[RepartPartitionEntry] {
        &self.partition_entries
    }

    /// Execute the `systemd-repart` command.
    ///
    /// Returns the list of partitions in the disk after repart is done in
    /// logical order.
    pub fn execute(&self) -> Result<Vec<RepartPartition>, Error> {
        self.check_unique_ids()
            .context("Repart configuration has duplicate IDs")?;

        self.execute_inner().with_context(|| {
            format!(
                "Failed to execute systemd-repart on {}",
                self.disk.display()
            )
        })
    }

    fn check_unique_ids(&self) -> Result<(), Error> {
        // Check that all partition entries have unique IDs
        let mut unique_ids: HashSet<String> = HashSet::new();
        for partition_entry in &self.partition_entries {
            ensure!(
                unique_ids.insert(partition_entry.id.clone()),
                "Duplicate partition ID: {}",
                partition_entry.id
            );
        }
        Ok(())
    }

    fn execute_inner(&self) -> Result<Vec<RepartPartition>, Error> {
        let repart_root = tempfile::tempdir()
            .context("Failed to create temporary directory for systemd-repart files")?;

        // Populate a temporary directory with the partition definitions
        let id_mapping = self
            .generate_config(repart_root.path())
            .context("Failed to generate systemd-repart config")?;

        // Generate a random UUID to use for the 'seed' argument. This way, 'systemd-repart' will
        // hash a unique PTUUID for each disk/partition table.
        let seed = Uuid::new_v4();

        let repart_output_json = Command::new("systemd-repart")
            .arg(self.disk.as_os_str())
            .arg("--dry-run=no")
            .arg(format!("--empty={}", self.mode.to_str()))
            .arg(format!("--seed={}", seed))
            .arg("--json=short")
            .arg("--definitions")
            .arg(repart_root.path())
            .output_and_check()
            .context("Failed to execute systemd-repart")?;

        let mut repart_output: Vec<RepartPartition> = serde_json::from_str(&repart_output_json)
            .context("Failed to deserialize output of systemd-repart")?;

        // Update the IDs of the partitions to match the IDs of the partition entries
        repart_output
            .iter_mut()
            .try_for_each(|partition| {
                partition.id = id_mapping
                    .get(&partition.file)
                    .with_context(|| {
                        format!(
                            "Failed to find ID mapping for partition definition file {}, existing mappings:\n{:#?}",
                            partition.file.display(),
                            id_mapping.iter().map(|(k, v)| format!("{} -> {}", k.display(), v)).collect::<Vec<_>>().join("\n")
                        )
                    })?
                    .clone();
                Ok(())
            })
            .context("Failed to synchronize partition IDs")?;

        Ok(repart_output)
    }

    /// Generate the configuration files for the partitions.
    ///
    /// It expects the root directory where the configuration files will be
    /// written to. Then writes the configuration files with names that will
    /// force repart to read them in the same order as they are declared in the
    /// local list.
    ///
    /// Returns a mapping of the paths to the generated configuration files to
    /// the IDs of the partitions.
    fn generate_config(&self, root: &Path) -> Result<HashMap<PathBuf, String>, Error> {
        if self.partition_entries.len() > 1000 {
            bail!(
                "Too many partitions ({}), this library only supports up to 1000 partitions",
                self.partition_entries.len()
            )
        }

        let mut id_mapping = HashMap::new();

        self.partition_entries
            .iter()
            .enumerate()
            .try_for_each(|(index, partition_entry)| {
                let path = root.join(format!("{:03}.conf", index));
                id_mapping.insert(path.clone(), partition_entry.id.clone());
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
                    })?;

                log::trace!("Generated systemd-repart config for partition #{}:\n {}", index,
                    std::fs::read_to_string(path).unwrap_or("(error)".to_string())
                );
                Ok(())
            }).context("Failed to generate systemd-repart config")?;

        Ok(id_mapping)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_partitions() {
        let partitions_status = serde_json::json!([
            {
                "type": "esp",
                "label": "esp-a",
                "uuid": "30bd5cbc-a4b8-437e-88ae-04944fdaf792",
                "file": "/tmp/.tmp2jMRKF/000.conf",
                "node": "/dev/sda1",
                "offset": 1048576usize,
                "old_size": 52428800usize,
                "raw_size": 52428800usize,
                "old_padding": 0,
                "raw_padding": 0,
                "activity": "unchanged"
            },
            {
                "type": "root-x86-64",
                "label": "root-a",
                "uuid": "84f91880-2765-4a24-a7fa-cda1328a9214",
                "file": "/tmp/.tmp2jMRKF/001.conf",
                "node": "/dev/sda2",
                "offset": 53477376,
                "old_size": 5368709120usize,
                "raw_size": 5368709120usize,
                "old_padding": 28937531392usize,
                "raw_padding": 23516393472usize,
                "activity": "create"
            },
        ]);

        let partitions = serde_json::from_value::<Vec<RepartPartition>>(partitions_status).unwrap();
        assert_eq!(partitions.len(), 2);
        assert_eq!(
            partitions[0].uuid.to_string(),
            "30bd5cbc-a4b8-437e-88ae-04944fdaf792"
        );
        assert_eq!(partitions[0].start, 1048576);
        assert_eq!(partitions[0].size, 52428800);

        // input is not an array
        let partition_status = serde_json::json!({
            "uuid": "84f91880-2765-4a24-a7fa-cda1328a9214",
            "offset": 2048,
            "raw_size": 1048576,
        });

        assert!(serde_json::from_value::<Vec<RepartPartition>>(partition_status).is_err());
    }

    #[test]
    fn test_parse_partition() {
        let partition_status = serde_json::json!({
            "type": "esp",
            "label": "esp-a",
            "uuid": "30bd5cbc-a4b8-437e-88ae-04944fdaf792",
            "file": "/tmp/.tmp2jMRKF/000.conf",
            "node": "/dev/sda1",
            "offset": 1048576,
            "old_size": 52428800,
            "raw_size": 52428800,
            "old_padding": 0,
            "raw_padding": 0,
            "activity": "unchanged"
        });

        let partition = serde_json::from_value::<RepartPartition>(partition_status).unwrap();
        assert_eq!(
            partition.uuid.to_string(),
            "30bd5cbc-a4b8-437e-88ae-04944fdaf792"
        );
        assert_eq!(partition.start, 1048576);
        assert_eq!(partition.size, 52428800);
        assert_eq!(partition.activity, RepartActivity::Unchanged);
        assert_eq!(partition.partition_type, DiscoverablePartitionType::Esp);
        assert_eq!(partition.label.as_deref(), Some("esp-a"));
        assert_eq!(partition.file, PathBuf::from("/tmp/.tmp2jMRKF/000.conf"));

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
            id: "test".to_owned(),
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
            id: "test".to_owned(),
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
                id: "test1".to_owned(),
                partition_type: DiscoverablePartitionType::Esp,
                label: None,
                size_min_bytes: Some(8388608), // 8 MiB
                size_max_bytes: Some(8388608),
            },
            RepartPartitionEntry {
                id: "test2".to_owned(),
                partition_type: DiscoverablePartitionType::Esp,
                label: None,
                size_min_bytes: Some(10737418240), // 10 GiB
                size_max_bytes: Some(10737418240),
            },
            RepartPartitionEntry {
                id: "test3".to_owned(),
                partition_type: DiscoverablePartitionType::LinuxGeneric,
                label: Some("testpart".into()),
                size_min_bytes: None,
                size_max_bytes: None,
            },
        ];

        let repart_root = tempfile::tempdir()
            .expect("Failed to create temporary directory for systemd-repart files");

        SystemdRepartInvoker::new("/dev/null", RepartEmptyMode::Force)
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
                id: "test".to_owned(),
                partition_type: DiscoverablePartitionType::Esp,
                label: None,
                size_min_bytes: Some(8388608), // 8 MiB
                size_max_bytes: Some(8388608),
            });
        }

        let repart_root = tempfile::tempdir()
            .expect("Failed to create temporary directory for systemd-repart files");

        let result = SystemdRepartInvoker::new("/dev/null", RepartEmptyMode::Force)
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

    use crate::lsblk::{self, BlockDevice, BlockDeviceType, PartitionTableType};
    use crate::testutils::repart::{
        self, DISK_SIZE, OS_DISK_DEVICE_PATH, PART1_SIZE, TEST_DISK_DEVICE_PATH,
    };
    use crate::udevadm;

    use super::*;

    #[functional_test(feature = "helpers")]
    fn test_execute_and_resulting_layout() {
        let unchanged_disk_bus_path = PathBuf::from(OS_DISK_DEVICE_PATH);
        let unchanged_block_device_list = lsblk::run(&unchanged_disk_bus_path).unwrap();

        let partition_definition = repart::generate_partition_definition_esp_generic();

        let disk_bus_path = PathBuf::from(TEST_DISK_DEVICE_PATH);

        let repart = SystemdRepartInvoker::new(&disk_bus_path, RepartEmptyMode::Force)
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

        let block_device = lsblk::run(&disk_bus_path).unwrap();
        let expected_block_device = BlockDevice {
            name: TEST_DISK_DEVICE_PATH.into(),
            fstype: None,
            fssize: None,
            ptuuid: block_device.ptuuid.clone(),
            part_uuid: None,
            size: DISK_SIZE,
            parent_kernel_name: None,
            partition_table_type: Some(PartitionTableType::Gpt),
            mountpoint: None,
            mountpoints: vec![],
            readonly: false,
            blkdev_type: BlockDeviceType::Disk,
            children: vec![
                BlockDevice {
                    name: format!("{TEST_DISK_DEVICE_PATH}1"),
                    fstype: None,
                    fssize: None,
                    ptuuid: None,
                    part_uuid: Some(part1.uuid.into()),
                    size: part1.size,
                    parent_kernel_name: Some(PathBuf::from(TEST_DISK_DEVICE_PATH)),
                    partition_table_type: None,
                    children: vec![],
                    mountpoint: None,
                    mountpoints: vec![],
                    readonly: false,
                    blkdev_type: BlockDeviceType::Partition,
                },
                BlockDevice {
                    name: format!("{TEST_DISK_DEVICE_PATH}2"),
                    fstype: None,
                    fssize: None,
                    ptuuid: None,
                    part_uuid: Some(part2.uuid.into()),
                    size: part2.size,
                    parent_kernel_name: Some(PathBuf::from(TEST_DISK_DEVICE_PATH)),
                    partition_table_type: None,
                    children: vec![],
                    mountpoint: None,
                    mountpoints: vec![],
                    readonly: false,
                    blkdev_type: BlockDeviceType::Partition,
                },
            ],
        };

        assert_eq!(expected_block_device, block_device);
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_execute_fails_on_non_block_device() {
        // Test that we can repartition /dev/null
        let repart = SystemdRepartInvoker::new("/dev/null", RepartEmptyMode::Force)
            .with_partition_entries(repart::generate_partition_definition_esp_generic());
        assert_eq!(repart.execute().unwrap_err().root_cause().to_string(), "Process output:\nstderr:\nFailed to open file or determine backing device of /dev/null: Block device required\n\n");
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_execute_fails_on_missing_block_device() {
        // Test that we can repartition a non-existing device
        let repart = SystemdRepartInvoker::new("/dev/does-not-exist", RepartEmptyMode::Force)
            .with_partition_entries(repart::generate_partition_definition_esp_generic());
        assert_eq!(repart.execute().unwrap_err().root_cause().to_string(), "Process output:\nstderr:\nFailed to open file or determine backing device of /dev/does-not-exist: No such file or directory\n\n");
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_execute_fails_if_partition_size_too_large() {
        // Test that asking for too much space fails
        let mut partition_definition = repart::generate_partition_definition_esp_generic();
        // Make the second partition too big
        partition_definition[1].size_min_bytes = Some(DISK_SIZE);
        partition_definition[1].size_max_bytes = Some(DISK_SIZE);
        let disk_bus_path = PathBuf::from(TEST_DISK_DEVICE_PATH);
        let repart = SystemdRepartInvoker::new(disk_bus_path, RepartEmptyMode::Force)
            .with_partition_entries(partition_definition);
        assert_eq!(repart.execute().unwrap_err().root_cause().to_string(), "Process output:\nstderr:\nCan't fit requested partitions into available free space (15.9G), refusing.\nAutomatically determined minimal disk image size as 16.0G, current image size is 16.0G.\n\n");
    }

    #[functional_test(feature = "helpers")]
    fn test_execute_unique_ptuuid() {
        // Run execute() once
        let partition_definition = repart::generate_partition_definition_esp_generic();
        let disk_bus_path = PathBuf::from(TEST_DISK_DEVICE_PATH);
        let repart = SystemdRepartInvoker::new(disk_bus_path.clone(), RepartEmptyMode::Force)
            .with_partition_entries(partition_definition.clone());
        repart.execute().unwrap();
        udevadm::settle().unwrap();
        // Run lsblk and extract PTUUID of the disk/partition table
        let block_device = lsblk::run(&disk_bus_path).unwrap();
        let ptuuid = block_device.ptuuid.unwrap();

        // Run execute() again on the same disk
        let repart = SystemdRepartInvoker::new(&disk_bus_path, RepartEmptyMode::Force)
            .with_partition_entries(partition_definition);
        repart.execute().unwrap();
        udevadm::settle().unwrap();
        // Run lsblk and extract PTUUID of the disk/partition table after the repartitioning
        let block_device = lsblk::run(&disk_bus_path).unwrap();
        let ptuuid_after = block_device.ptuuid.unwrap();

        // Ensure that the two PTUUIDs are unique
        assert_ne!(ptuuid, ptuuid_after);
    }
}
