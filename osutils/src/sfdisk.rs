use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use duct::cmd;
use serde::Deserialize;
use uuid::Uuid;

use crate::partition_types::DiscoverablePartitionType;

#[derive(Debug, PartialEq, Deserialize)]
struct SfdiskOutput {
    partitiontable: SfDisk,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct SfDisk {
    /// Disk label type
    pub label: SfDiskLabel,

    /// Disk UUID
    pub id: Uuid,

    /// Disk device path
    pub device: PathBuf,

    /// Disk size unit (always "sectors")
    pub unit: SfDiskUnit,

    /// First LBA
    pub firstlba: u64,

    /// Last LBA
    pub lastlba: u64,

    /// Sector size
    #[serde(default = "SfDisk::default_sectorsize")]
    pub sectorsize: u64,

    /// List of partitions
    #[serde(default)]
    pub partitions: Vec<SfPartition>,

    /// Disk capacity
    #[serde(skip)]
    pub capacity: u64,
}

impl SfDisk {
    fn default_sectorsize() -> u64 {
        512
    }
}

#[derive(Debug, PartialEq, Deserialize, Clone)]
pub struct SfPartition {
    /// Partition device path
    pub node: PathBuf,

    /// Partition start offset
    pub start: u64,

    /// Partition size in sectors
    #[serde(rename = "size")]
    pub size_sectors: u64,

    /// Partition type
    #[serde(rename = "type")]
    pub partition_type: DiscoverablePartitionType,

    /// Partition UUID
    #[serde(rename = "uuid")]
    pub id: Uuid,

    /// Partition name
    pub name: Option<String>,

    /// Partition size in bytes
    #[serde(skip)]
    pub size: u64,
}

#[derive(Debug, PartialEq, Deserialize)]
pub enum SfDiskLabel {
    #[serde(rename = "gpt")]
    Gpt,
}

#[derive(Debug, PartialEq, Deserialize)]
pub enum SfDiskUnit {
    #[serde(rename = "sectors")]
    Sectors,
}

impl SfDisk {
    pub fn get_info<S>(disk_bus_path: S) -> Result<Self, Error>
    where
        S: AsRef<Path>,
    {
        let sfdisk_output_json = cmd!("sfdisk", "-J", disk_bus_path.as_ref())
            .read()
            .context(format!(
                "Failed to fetch disk information for {}",
                disk_bus_path.as_ref().display()
            ))?;

        SfDisk::parse_sfdisk_output(&sfdisk_output_json).context(format!(
            "Failed to extract disk information for {}",
            disk_bus_path.as_ref().display()
        ))
    }

    fn parse_sfdisk_output(output: &str) -> Result<Self, Error> {
        let mut disk = serde_json::from_str::<SfdiskOutput>(output)
            .context("Failed to parse disk information")?
            .partitiontable;

        // Update capacity and partition sizes
        disk.capacity = (disk.lastlba - disk.firstlba + 1) * disk.sectorsize;
        disk.partitions.iter_mut().for_each(|part| {
            part.size = part.size_sectors * disk.sectorsize;
        });

        Ok(disk)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_disk() {
        let sfdisk_output_json = r#"
        {
            "partitiontable": {
               "label": "gpt",
               "id": "3E6494F9-91E1-426B-A25A-0A8101E464A4",
               "device": "/dev/sda",
               "unit": "sectors",
               "firstlba": 34,
               "lastlba": 266338270,
               "sectorsize": 512,
               "partitions": [
                  {
                     "node": "/dev/sda1",
                     "start": 2048,
                     "size": 16384,
                     "type": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
                     "uuid": "F764E91F-9D15-4F6E-8508-0AFC1D0DF0B5",
                     "name": "esp"
                  },{
                     "node": "/dev/sda2",
                     "start": 20480,
                     "size": 266315776,
                     "type": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
                     "uuid": "4D8C2A88-1411-4021-804D-EB8C40F054AA",
                     "name": "rootfs"
                  }
               ]
            }
         }
         "#;
        let parsed = SfDisk::parse_sfdisk_output(sfdisk_output_json).unwrap();
        assert_eq!(
            parsed,
            SfDisk {
                label: SfDiskLabel::Gpt,
                id: Uuid::parse_str("3E6494F9-91E1-426B-A25A-0A8101E464A4").unwrap(),
                device: PathBuf::from("/dev/sda"),
                unit: SfDiskUnit::Sectors,
                firstlba: 34,
                lastlba: 266338270,
                sectorsize: 512,
                capacity: 136_365_177_344,
                partitions: vec![
                    SfPartition {
                        node: PathBuf::from("/dev/sda1"),
                        start: 2048,
                        size_sectors: 16_384,
                        size: 8_388_608,
                        partition_type: DiscoverablePartitionType::Esp,
                        id: Uuid::parse_str("F764E91F-9D15-4F6E-8508-0AFC1D0DF0B5").unwrap(),
                        name: Some("esp".to_string()),
                    },
                    SfPartition {
                        node: PathBuf::from("/dev/sda2"),
                        start: 20480,
                        size_sectors: 266_315_776,
                        size: 136_353_677_312,
                        partition_type: DiscoverablePartitionType::LinuxGeneric,
                        id: Uuid::parse_str("4D8C2A88-1411-4021-804D-EB8C40F054AA").unwrap(),
                        name: Some("rootfs".to_string()),
                    }
                ],
            },
            "parsed disk is wrong"
        );

        // malformed UUID
        let sfdisk_output_json = r#"
        {
            "partitiontable": {
                "label": "gpt",
                "id": "3E6494F9-91E1-426B-A25A-0A81",
                "device": "/dev/sda",
                "firstlba": 2048,
                "lastlba": 67108830,
                "sectorsize": 512,
                "unit": "sectors"
            }
        }"#;

        assert!(SfDisk::parse_sfdisk_output(sfdisk_output_json).is_err());

        // missing firstlba
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "id": "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2ac5d",
                "lastlba": 67108830,
                "sectorsize": 512,
                "unit": "sectors"
            }
        }"#;

        assert!(SfDisk::parse_sfdisk_output(sfdisk_output_json).is_err());

        // missing lastlba
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "label": "gpt",
                "id": "3E6494F9-91E1-426B-A25A-0A8101E464A4",
                "device": "/dev/sda",
                "firstlba": 2048,
                "sectorsize": 512,
                "unit": "sectors"
            }
        }"#;

        assert!(SfDisk::parse_sfdisk_output(sfdisk_output_json).is_err());

        // missing sector size
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "label": "gpt",
                "id": "3E6494F9-91E1-426B-A25A-0A8101E464A4",
                "device": "/dev/sda",
                "firstlba": 2048,
                "lastlba": 67108830,
                "unit": "sectors"
            }
        }"#;

        assert_eq!(
            SfDisk::parse_sfdisk_output(sfdisk_output_json)
                .unwrap()
                .sectorsize,
            SfDisk::default_sectorsize(),
            "default sector size is wrong"
        );

        // missing unit
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "label": "gpt",
                "id": "3E6494F9-91E1-426B-A25A-0A8101E464A4",
                "device": "/dev/sda",
                "firstlba": 2048,
                "lastlba": 67108830,
                "sectorsize": 512
            }
        }"#;

        assert!(SfDisk::parse_sfdisk_output(sfdisk_output_json).is_err());

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

        assert!(SfDisk::parse_sfdisk_output(sfdisk_output_json).is_err());
    }
}

#[cfg(all(test, feature = "functional-tests"))]
mod functional_tests {
    use std::path::PathBuf;

    use super::*;

    /// Functional test for `SfDisk::get_info`
    ///
    /// This test requires `sfdisk` to be installed on the system.
    ///
    /// It assumes the following disk layout:
    /// ```yaml
    /// # Host config
    /// partitions:
    /// - id: esp
    ///   size: 50M
    ///   type: esp
    /// - id: root-a
    ///   size: 3G
    ///   type: root
    /// - id: root-b
    ///   size: 2G
    ///   type: root
    /// - id: swap
    ///   size: 2G
    ///   type: swap
    /// - id: trident
    ///   size: 100M
    ///   type: linux-generic
    /// ```
    ///
    /// The output of `sfdisk -J /dev/sda` is expected to be:
    ///  
    /// ```json
    /// {
    ///   "partitiontable": {
    ///    "label": "gpt",
    ///    "id": "A8DBCA6F-77A6-485C-8C67-B653758A8928",
    ///    "device": "/dev/sda",
    ///    "unit": "sectors",
    ///    "firstlba": 2048,
    ///    "lastlba": 33554398,
    ///    "sectorsize": 512,
    ///    "partitions": [
    ///       {
    ///          "node": "/dev/sda1",
    ///          "start": 2048,
    ///          "size": 102400,
    ///          "type": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
    ///          "uuid": "7D77C4D1-8D4D-42BC-9968-2C1C0E242E0B",
    ///          "name": "esp"
    ///       },{
    ///          "node": "/dev/sda2",
    ///          "start": 104448,
    ///          "size": 6291456,
    ///          "type": "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709",
    ///          "uuid": "8B03BBD1-8A00-4E66-8315-71923929F2B5",
    ///          "name": "root-a",
    ///          "attrs": "GUID:59"
    ///       },{
    ///          "node": "/dev/sda3",
    ///          "start": 6395904,
    ///          "size": 4194304,
    ///          "type": "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709",
    ///          "uuid": "7B22B5BF-8A3C-439C-A639-19EA81E4D05A",
    ///          "name": "root-b",
    ///          "attrs": "GUID:59"
    ///       },{
    ///          "node": "/dev/sda4",
    ///          "start": 10590208,
    ///          "size": 4194304,
    ///          "type": "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F",
    ///          "uuid": "93F88CB4-B0F0-49E3-BD8A-369E8B380305",
    ///          "name": "swap"
    ///       },{
    ///          "node": "/dev/sda5",
    ///          "start": 14784512,
    ///          "size": 204800,
    ///          "type": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
    ///          "uuid": "AB6FA2BD-70F8-4C1A-83E4-9D704A865C3B",
    ///          "name": "trident"
    ///       }
    ///    ]
    /// }
    /// ```
    #[test]
    fn test_sfdisk_get() {
        let disk = SfDisk::get_info("/dev/sda").unwrap();
        assert_eq!(disk.device, PathBuf::from("/dev/sda"));
        assert_eq!(disk.label, SfDiskLabel::Gpt);
        assert_eq!(disk.unit, SfDiskUnit::Sectors);
        print!("disk: {:#?}", disk);
        assert_eq!(disk.firstlba, 2048);
        assert_eq!(disk.lastlba, 33554398);
        assert_eq!(disk.sectorsize, 512);
        assert_eq!(disk.capacity, 17_178_803_712);
        assert_eq!(disk.partitions.len(), 5);

        let expected_partitions = [
            SfPartition {
                node: PathBuf::from("/dev/sda1"),
                start: 2048,
                size_sectors: 102400,
                size: 52428800,
                partition_type: DiscoverablePartitionType::Esp.resolve(),
                id: Uuid::nil(),
                name: Some("esp".to_string()),
            },
            SfPartition {
                node: PathBuf::from("/dev/sda2"),
                start: 104448,
                size_sectors: 6291456,
                size: 3221225472,
                partition_type: DiscoverablePartitionType::Root.resolve(),
                id: Uuid::nil(),
                name: Some("root-a".to_string()),
            },
            SfPartition {
                node: PathBuf::from("/dev/sda3"),
                start: 6395904,
                size_sectors: 4194304,
                size: 2147483648,
                partition_type: DiscoverablePartitionType::Root.resolve(),
                id: Uuid::nil(),
                name: Some("root-b".to_string()),
            },
            SfPartition {
                node: PathBuf::from("/dev/sda4"),
                start: 10590208,
                size_sectors: 4194304,
                size: 2147483648,
                partition_type: DiscoverablePartitionType::Swap.resolve(),
                id: Uuid::nil(),
                name: Some("swap".to_string()),
            },
            SfPartition {
                node: PathBuf::from("/dev/sda5"),
                start: 14784512,
                size_sectors: 204800,
                size: 104857600,
                partition_type: DiscoverablePartitionType::LinuxGeneric.resolve(),
                id: Uuid::nil(),
                name: Some("trident".to_string()),
            },
        ];

        assert_eq!(
            disk.partitions.len(),
            expected_partitions.len(),
            "number of partitions is wrong"
        );

        for (expected, found) in expected_partitions.iter().zip(disk.partitions.iter()) {
            assert_eq!(found.node, expected.node, "node is wrong");
            assert_eq!(found.start, expected.start, "start is wrong");
            assert_eq!(
                found.size_sectors, expected.size_sectors,
                "size_sectors is wrong"
            );
            assert_eq!(found.size, expected.size, "size is wrong");
            assert_eq!(
                found.partition_type, expected.partition_type,
                "partition_type is wrong"
            );
            // Skip UUID check as it is not expected to be the same
            assert_eq!(found.name, expected.name, "name is wrong");
        }
    }
}
