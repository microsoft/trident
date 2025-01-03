use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use serde::Deserialize;

use crate::{dependencies::Dependency, osuuid::OsUuid, partition_types::DiscoverablePartitionType};

#[derive(Debug, PartialEq, Deserialize)]
struct SfdiskOutput {
    partitiontable: SfDisk,
}

#[derive(Debug, PartialEq, Deserialize)]
pub struct SfDisk {
    /// Disk label type
    pub label: SfDiskLabel,

    /// Disk UUID
    pub id: OsUuid,

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

#[derive(Debug, PartialEq, Eq, Deserialize, Clone, Hash)]
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
    pub id: OsUuid,

    /// Partition name
    pub name: Option<String>,

    /// Partition size in bytes
    #[serde(skip)]
    pub size: u64,

    /// Parent disk path
    #[serde(skip)]
    pub parent: PathBuf,

    /// Partition number in the partition table
    #[serde(skip)]
    pub number: usize,
}

#[derive(Debug, PartialEq, Deserialize)]
pub enum SfDiskLabel {
    #[serde(rename = "gpt")]
    Gpt,

    /// Master Boot Record
    #[serde(rename = "mbr", alias = "dos")]
    Mbr,
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
        let sfdisk_output_json = Dependency::Sfdisk
            .cmd()
            .arg("-J")
            .arg(disk_bus_path.as_ref())
            .output_and_check()
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
        disk.partitions.iter_mut().try_for_each(|part| {
            part.size = part.size_sectors * disk.sectorsize;
            part.parent = disk.device.clone();
            part.number = part
                .node
                .as_os_str()
                .to_string_lossy()
                .rsplit_once(|c: char| !c.is_ascii_digit())
                .map(|(_, n)| n)
                .context(format!(
                    "Failed to extract partition number from {}",
                    part.node.display()
                ))?
                .parse()
                .context(format!(
                    "Failed to parse partition number from {}",
                    part.node.display()
                ))?;
            Ok::<(), Error>(())
        })?;

        Ok(disk)
    }
}

impl SfPartition {
    pub fn path_by_uuid(&self) -> PathBuf {
        Path::new("/dev/disk/by-partuuid").join(self.id.to_string())
    }

    pub fn delete(&self) -> Result<(), Error> {
        Dependency::Sfdisk
            .cmd()
            .arg("--delete")
            .arg(&self.parent)
            .arg(self.number.to_string())
            .run_and_check()
            .context(format!(
                "Failed to delete partition {}",
                self.node.display()
            ))?;
        Ok(())
    }
}

/// Gets the UUID of the disk using sfdisk, returns None if the disk has no UUID
/// set.
pub fn get_disk_uuid(disk: &Path) -> Result<Option<OsUuid>, Error> {
    let output = Dependency::Sfdisk
        .cmd()
        .arg("--disk-id")
        .arg(disk)
        .output()
        .context("Failed to execute sfdisk command")?;

    let output_str = output.output();

    if output_str.trim().is_empty() {
        return Ok(None);
    }

    let uuid = OsUuid::from(output_str.trim());

    Ok(Some(uuid))
}

#[cfg(test)]
mod tests {
    use super::*;

    use uuid::Uuid;

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
                     "node": "/dev/sda3",
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
                id: Uuid::parse_str("3E6494F9-91E1-426B-A25A-0A8101E464A4")
                    .unwrap()
                    .into(),
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
                        id: Uuid::parse_str("F764E91F-9D15-4F6E-8508-0AFC1D0DF0B5")
                            .unwrap()
                            .into(),
                        name: Some("esp".to_string()),
                        parent: PathBuf::from("/dev/sda"),
                        number: 1,
                    },
                    SfPartition {
                        node: PathBuf::from("/dev/sda3"),
                        start: 20480,
                        size_sectors: 266_315_776,
                        size: 136_353_677_312,
                        partition_type: DiscoverablePartitionType::LinuxGeneric,
                        id: Uuid::parse_str("4D8C2A88-1411-4021-804D-EB8C40F054AA")
                            .unwrap()
                            .into(),
                        name: Some("rootfs".to_string()),
                        parent: PathBuf::from("/dev/sda"),
                        number: 3,
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

        let parsed = SfDisk::parse_sfdisk_output(sfdisk_output_json).unwrap();
        assert_eq!(
            parsed.id,
            OsUuid::Relaxed("3E6494F9-91E1-426B-A25A-0A81".into()),
            "malformed UUID should be nil"
        );

        // missing firstlba
        let sfdisk_output_json = r#"{
            "partitiontable": {
                "id": "a1b2c3d4-e5f6-4a5b-8c9d-0e1f2ac5d",
                "lastlba": 67108830,
                "sectorsize": 512,
                "unit": "sectors"
            }
        }"#;

        SfDisk::parse_sfdisk_output(sfdisk_output_json).unwrap_err();

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

        SfDisk::parse_sfdisk_output(sfdisk_output_json).unwrap_err();

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

        SfDisk::parse_sfdisk_output(sfdisk_output_json).unwrap_err();

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

        SfDisk::parse_sfdisk_output(sfdisk_output_json).unwrap_err();
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::path::PathBuf;

    use uuid::Uuid;

    use pytest_gen::functional_test;

    /// Functional test for `SfDisk::get_info`
    ///
    /// This test requires `sfdisk` to be installed on the system.
    ///
    /// It assumes the disk layout requested in functional_tests/trident-setup.yaml.
    ///
    /// The output of `sfdisk -J /dev/sda` is expected to be:
    ///
    ///  ```json
    /// {
    ///   "partitiontable": {
    ///     "label": "gpt",
    ///     "id": "71F5C3EB-6D53-414B-9FF4-0953E6291577",
    ///     "device": "/dev/sda",
    ///     "unit": "sectors",
    ///     "firstlba": 2048,
    ///     "lastlba": 33554398,
    ///     "sectorsize": 512,
    ///     "partitions": [
    ///        {
    ///           "node": "/dev/sda1",
    ///           "start": 2048,
    ///           "size": 102400,
    ///           "type": "C12A7328-F81F-11D2-BA4B-00A0C93EC93B",
    ///           "uuid": "8D738FD1-9B6F-4B6D-B174-021954453D68",
    ///           "name": "esp"
    ///        },{
    ///           "node": "/dev/sda2",
    ///           "start": 104448,
    ///           "size": 8388608,
    ///           "type": "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709",
    ///           "uuid": "71982B79-7759-449F-8D68-ACA7625AC1E2",
    ///           "name": "root-a",
    ///           "attrs": "GUID:59"
    ///        },{
    ///           "node": "/dev/sda3",
    ///           "start": 8493056,
    ///           "size": 8388608,
    ///           "type": "4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709",
    ///           "uuid": "1864172F-3594-4F7A-B447-EBCA0B115DC6",
    ///           "name": "root-b",
    ///           "attrs": "GUID:59"
    ///        },{
    ///           "node": "/dev/sda4",
    ///           "start": 16881664,
    ///           "size": 4194304,
    ///           "type": "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F",
    ///           "uuid": "ED608DB8-58D6-484B-B309-B03CD3615037",
    ///           "name": "swap"
    ///        },{
    ///           "node": "/dev/sda5",
    ///           "start": 21075968,
    ///           "size": 204800,
    ///           "type": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
    ///           "uuid": "7DE2DA6E-4512-4091-B0B7-EC432DA971AA",
    ///           "name": "trident"
    ///        }
    ///     ]
    ///  }
    /// }
    /// ```
    ///
    #[functional_test(feature = "helpers")]
    fn test_get() {
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
                id: Uuid::nil().into(),
                name: Some("esp".to_string()),
                parent: PathBuf::from("/dev/sda"),
                number: 1,
            },
            SfPartition {
                node: PathBuf::from("/dev/sda2"),
                start: 104448,
                size_sectors: 8388608,
                size: 4294967296,
                partition_type: DiscoverablePartitionType::Root.resolve(),
                id: Uuid::nil().into(),
                name: Some("root-a".to_string()),
                parent: PathBuf::from("/dev/sda"),
                number: 2,
            },
            SfPartition {
                node: PathBuf::from("/dev/sda3"),
                start: 8493056,
                size_sectors: 8388608,
                size: 4294967296,
                partition_type: DiscoverablePartitionType::Root.resolve(),
                id: Uuid::nil().into(),
                name: Some("root-b".to_string()),
                parent: PathBuf::from("/dev/sda"),
                number: 3,
            },
            SfPartition {
                node: PathBuf::from("/dev/sda4"),
                start: 16881664,
                size_sectors: 4194304,
                size: 2147483648,
                partition_type: DiscoverablePartitionType::Swap.resolve(),
                id: Uuid::nil().into(),
                name: Some("swap".to_string()),
                parent: PathBuf::from("/dev/sda"),
                number: 4,
            },
            SfPartition {
                node: PathBuf::from("/dev/sda5"),
                start: 21075968,
                size_sectors: 204800,
                size: 104857600,
                partition_type: DiscoverablePartitionType::LinuxGeneric.resolve(),
                id: Uuid::nil().into(),
                name: Some("trident".to_string()),
                parent: PathBuf::from("/dev/sda"),
                number: 5,
            },
        ];

        assert_eq!(
            disk.partitions.len(),
            expected_partitions.len(),
            "Expected {} partitions, found {}",
            disk.partitions.len(),
            expected_partitions.len()
        );

        for (expected, found) in expected_partitions.iter().zip(disk.partitions.iter()) {
            assert_eq!(
                found.node,
                expected.node,
                "Expected node {}, found {}",
                expected.node.display(),
                found.node.display()
            );
            assert_eq!(
                found.start, expected.start,
                "Expected node to start at {}, but it starts at {}",
                expected.start, found.start
            );
            assert_eq!(
                found.size_sectors, expected.size_sectors,
                "Expected node to have size_sectors {}, but it has {}",
                expected.size_sectors, found.size_sectors
            );
            assert_eq!(
                found.size, expected.size,
                "Expected node to have size {}, but it has {}",
                expected.size, found.size
            );
            assert_eq!(
                found.partition_type, expected.partition_type,
                "Expected node to have partition type {:?}, but it has partition {:?}",
                expected.partition_type, found.partition_type
            );
            // Skip UUID check as it is not expected to be the same
            assert_eq!(
                found.name, expected.name,
                "Expected node to have name {:?}, but it has name {:?}",
                expected.name, found.name
            );
        }
    }
}
