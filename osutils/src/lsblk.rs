use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use serde::{Deserialize, Serialize};

use sysdefs::osuuid::OsUuid;
use trident_api::primitives::bytes::ByteCount;

use crate::dependencies::Dependency;

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct LsBlkOutput {
    pub blockdevices: Vec<BlockDevice>,
}

/// Represents a block device as returned by `lsblk --json`. See `man lsblk` for
/// more information. Descriptions are copied from the output of `lsblk --help`
/// in AzL 2.0.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
pub struct BlockDevice {
    /// Device name
    pub name: String,

    /// Filesystem type
    pub fstype: Option<String>,

    /// Filesystem size
    pub fssize: Option<ByteCount>,

    /// Filesystem UUID
    #[serde(rename = "uuid")]
    pub fsuuid: Option<OsUuid>,

    /// Partition table UUID
    pub ptuuid: Option<OsUuid>,

    /// Partition UUID
    #[serde(rename = "partuuid")]
    pub part_uuid: Option<OsUuid>,

    /// Size of the device
    pub size: u64,

    /// Internal parent kernel device name
    #[serde(rename = "pkname")]
    pub parent_kernel_name: Option<PathBuf>,

    /// List of children devices
    ///
    /// Not a column, only displayed if --json is specified. Contains a list of
    /// all children devices. (e.g. partitions of a disk device)
    #[serde(default)]
    pub children: Vec<BlockDevice>,

    /// Where the device is mounted
    #[serde(default)]
    pub mountpoint: Option<PathBuf>,

    /// All locations where device is mounted
    #[serde(default, deserialize_with = "skip_nulls")]
    pub mountpoints: Vec<PathBuf>,

    /// Partition table type
    #[serde(rename = "pttype")]
    pub partition_table_type: Option<PartitionTableType>,

    // Read-only device
    #[serde(default, rename = "ro")]
    pub readonly: bool,

    // Device type
    #[serde(default, rename = "type")]
    pub blkdev_type: BlockDeviceType,
}

impl BlockDevice {
    /// Gets a list of all mountpoints for this device and its children.
    pub fn get_all_mountpoints_recursive(&self) -> Vec<&Path> {
        self.mountpoints
            .iter()
            .map(|p| p.as_path())
            .chain(
                self.children
                    .iter()
                    .flat_map(|ch| ch.get_all_mountpoints_recursive()),
            )
            .collect()
    }
}

/// All possible device types returned by lsblk
/// https://github.com/util-linux/util-linux/blob/master/misc-utils/lsblk.c#L402-L456
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "kebab-case")]
pub enum BlockDeviceType {
    #[serde(alias = "part")]
    Partition,
    Lvm,
    Crypt,
    Dmraid,
    Mpath,
    Dm,
    Path,
    Md,
    Loop,
    Disk,
    Tape,
    Printer,
    Processor,
    Worm,
    Rom,
    Scanner,
    MoDisk,
    Charger,
    Comm,
    Raid,
    Enclosure,
    Rbc,
    Osd,
    NoLun,

    #[default]
    #[serde(other)]
    Unknown,
}

/// Partition table types recognized by `lsblk`
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionTableType {
    /// GUID Partition Table
    #[serde(rename = "gpt")]
    Gpt,

    /// Master Boot Record
    #[serde(rename = "mbr", alias = "dos")]
    Mbr,
}

/// Returns a list of all block devices on the system.
///
/// This function executes the `lsblk` command to retrieve detailed
/// information about all available block devices in JSON format. It
/// parses the command's output and returns a `Vec` of `BlockDevice`
/// structs.
///
pub fn list() -> Result<Vec<BlockDevice>, Error> {
    let result = Dependency::Lsblk
        .cmd()
        .arg("--json")
        .arg("--output-all")
        .arg("--bytes")
        .output_and_check()
        .context("Failed to execute lsblk")?;

    let parsed: Vec<BlockDevice> = parse_lsblk_output(result.as_str())?;

    Ok(parsed)
}

/// Finds and returns all block devices (and their children) that match a
/// given predicate.
///
/// This function filters the list of block devices obtained from `list()`
/// based on a provided predicate function. It returns a `Vec` of
/// `BlockDevice` structs that satisfy the predicate, including their
/// child devices.
///
/// # Parameters
///
/// - `predicate`: A closure or function that takes a reference to a
///   `BlockDevice` and returns `true` if the device matches the desired
///   criteria, or `false` otherwise.
///
pub fn find(predicate: impl Fn(&BlockDevice) -> bool) -> Result<Vec<BlockDevice>, Error> {
    let block_devices = list().context("Failed to list block devices")?;
    let mut device_names_seen = HashSet::new();
    let mut matching_block_devices = Vec::new();

    find_recursive(
        &block_devices,
        &predicate,
        &mut device_names_seen,
        &mut matching_block_devices,
    );

    Ok(matching_block_devices)
}

/// Recursively searches for block devices that match the given predicate.
///
/// This helper function traverses a list of `BlockDevice` objects and
/// their children recursively, applying the provided predicate to each
/// device. Matching devices are added to the `matching_block_devices`
/// vector, ensuring that each device is only added once by using
/// `device_names_seen` to track already-seen device names.
///
/// # Parameters
///
/// - `block_devices`: A reference to the list of `BlockDevice` structs to
///   search through.
/// - `predicate`: A closure or function that takes a reference to a
///   `BlockDevice` and returns `true` if the device matches the desired
///   criteria, or `false` otherwise.
/// - `device_names_seen`: A mutable reference to a `HashSet` that keeps
///   track of device names that have already been processed.
/// - `matching_block_devices`: A mutable reference to a `Vec` that stores
///   the devices that match the predicate.
fn find_recursive(
    block_devices: &Vec<BlockDevice>,
    predicate: &impl Fn(&BlockDevice) -> bool,
    device_names_seen: &mut HashSet<String>,
    matching_block_devices: &mut Vec<BlockDevice>,
) {
    for block_device in block_devices {
        if predicate(block_device) && device_names_seen.insert(block_device.name.clone()) {
            matching_block_devices.push(block_device.clone());
        }

        find_recursive(
            &block_device.children,
            predicate,
            device_names_seen,
            matching_block_devices,
        );
    }
}

/// Retrieves detailed information for a specific block device at a the
/// specified path, if it exists.
pub fn try_get(device_path: impl AsRef<Path>) -> Result<Option<BlockDevice>, Error> {
    let result = Dependency::Lsblk
        .cmd()
        .arg("--json")
        .arg("--path")
        .arg(device_path.as_ref())
        .arg("--output-all")
        .arg("--bytes")
        .output_and_check()
        .context("Failed to execute lsblk")?;

    let parsed =
        parse_lsblk_output(result.as_str()).context("Failed to parse output from lsblk")?;

    if parsed.len() > 1 {
        bail!(
            "Unexpected number of block devices returned for device '{}': {}",
            device_path.as_ref().display(),
            parsed.len()
        );
    }

    Ok(parsed.into_iter().next())
}

/// Retrieves detailed information about a specific block device at a
/// given path. It is a wrapper around `get_opt` that returns an error if
/// no device is found.
pub fn get(device_path: impl AsRef<Path>) -> Result<BlockDevice, Error> {
    try_get(device_path.as_ref())
        .with_context(|| {
            format!(
                "Failed to get block device information for '{}'",
                device_path.as_ref().display()
            )
        })?
        .with_context(|| {
            format!(
                "No block device found at '{}'",
                device_path.as_ref().display()
            )
        })
}

fn parse_lsblk_output(output: &str) -> Result<Vec<BlockDevice>, Error> {
    let parsed: LsBlkOutput =
        serde_json::from_str(output).context("Failed to parse lsblk output")?;

    Ok(parsed.blockdevices)
}

fn skip_nulls<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    let v: Vec<Option<T>> = serde::Deserialize::deserialize(deserializer)?;
    Ok(v.into_iter().flatten().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Output obtained from running `lsblk --json --bytes --output-all --path /dev/sda`
    /// on the functional test VM AzL 2.0, lsblk from util-linux 2.37.4
    const SAMPLE_LSBLK_OUTPUT: &str = indoc::indoc! {
        r#"
            {
                "blockdevices": [
                    {
                        "name": "/dev/sda",
                        "kname": "/dev/sda",
                        "path": "/dev/sda",
                        "maj:min": "8:0",
                        "fsavail": null,
                        "fssize": null,
                        "fstype": null,
                        "fsused": null,
                        "fsuse%": null,
                        "fsroots": [
                            null
                        ],
                        "fsver": null,
                        "mountpoint": null,
                        "mountpoints": [
                            null
                        ],
                        "label": null,
                        "uuid": null,
                        "ptuuid": "a8dbca6f-77a6-485c-8c67-b653758a8928",
                        "pttype": "gpt",
                        "parttype": null,
                        "parttypename": null,
                        "partlabel": null,
                        "partuuid": null,
                        "partflags": null,
                        "ra": 128,
                        "ro": false,
                        "rm": false,
                        "hotplug": false,
                        "model": "QEMU HARDDISK   ",
                        "serial": null,
                        "size": 17179869184,
                        "state": "running",
                        "owner": "root",
                        "group": "disk",
                        "mode": "brw-rw----",
                        "alignment": 0,
                        "min-io": 512,
                        "opt-io": 0,
                        "phy-sec": 512,
                        "log-sec": 512,
                        "rota": true,
                        "sched": "mq-deadline",
                        "rq-size": 64,
                        "type": "disk",
                        "disc-aln": 0,
                        "disc-gran": 512,
                        "disc-max": 2147450880,
                        "disc-zero": false,
                        "wsame": 0,
                        "wwn": null,
                        "rand": true,
                        "pkname": null,
                        "hctl": "1:0:0:0",
                        "tran": "sata",
                        "subsystems": "block:scsi:pci",
                        "rev": "2.5+",
                        "vendor": "ATA     ",
                        "zoned": "none",
                        "dax": false,
                        "children": [
                            {
                            "name": "/dev/sda1",
                            "kname": "/dev/sda1",
                            "path": "/dev/sda1",
                            "maj:min": "8:1",
                            "fsavail": "49911808",
                            "fssize": "52293632",
                            "fstype": "vfat",
                            "fsused": "2381824",
                            "fsuse%": "5%",
                            "fsroots": [
                                "/"
                            ],
                            "fsver": null,
                            "mountpoint": "/boot/efi",
                            "mountpoints": [
                                "/boot/efi"
                            ],
                            "label": null,
                            "uuid": "C19C-752D",
                            "ptuuid": null,
                            "pttype": null,
                            "parttype": "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
                            "parttypename": null,
                            "partlabel": "esp",
                            "partuuid": "24d90361-7b1f-47db-b5bb-7d3893ac6ab0",
                            "partflags": null,
                            "ra": 128,
                            "ro": false,
                            "rm": false,
                            "hotplug": false,
                            "model": null,
                            "serial": null,
                            "size": 52428800,
                            "state": null,
                            "owner": "root",
                            "group": "disk",
                            "mode": "brw-rw----",
                            "alignment": 0,
                            "min-io": 512,
                            "opt-io": 0,
                            "phy-sec": 512,
                            "log-sec": 512,
                            "rota": true,
                            "sched": "mq-deadline",
                            "rq-size": 64,
                            "type": "part",
                            "disc-aln": 0,
                            "disc-gran": 512,
                            "disc-max": 2147450880,
                            "disc-zero": false,
                            "wsame": 0,
                            "wwn": null,
                            "rand": true,
                            "pkname": "/dev/sda",
                            "hctl": null,
                            "tran": null,
                            "subsystems": "block:scsi:pci",
                            "rev": null,
                            "vendor": null,
                            "zoned": "none",
                            "dax": false
                            },{
                            "name": "/dev/sda2",
                            "kname": "/dev/sda2",
                            "path": "/dev/sda2",
                            "maj:min": "8:2",
                            "fsavail": "3427942400",
                            "fssize": "5264343040",
                            "fstype": "ext4",
                            "fsused": "1551220736",
                            "fsuse%": "29%",
                            "fsroots": [
                                "/"
                            ],
                            "fsver": null,
                            "mountpoint": "/",
                            "mountpoints": [
                                "/"
                            ],
                            "label": null,
                            "uuid": "278a7e61-8212-4c84-8103-c8b2fd299670",
                            "ptuuid": null,
                            "pttype": null,
                            "parttype": "4f68bce3-e8cd-4db1-96e7-fbcaf984b709",
                            "parttypename": null,
                            "partlabel": "root-a",
                            "partuuid": "13fe614e-f738-4025-bc7f-8c71a3b8242a",
                            "partflags": "0x800000000000000",
                            "ra": 128,
                            "ro": false,
                            "rm": false,
                            "hotplug": false,
                            "model": null,
                            "serial": null,
                            "size": 5368709120,
                            "state": null,
                            "owner": "root",
                            "group": "disk",
                            "mode": "brw-rw----",
                            "alignment": 0,
                            "min-io": 512,
                            "opt-io": 0,
                            "phy-sec": 512,
                            "log-sec": 512,
                            "rota": true,
                            "sched": "mq-deadline",
                            "rq-size": 64,
                            "type": "part",
                            "disc-aln": 0,
                            "disc-gran": 512,
                            "disc-max": 2147450880,
                            "disc-zero": false,
                            "wsame": 0,
                            "wwn": null,
                            "rand": true,
                            "pkname": "/dev/sda",
                            "hctl": null,
                            "tran": null,
                            "subsystems": "block:scsi:pci",
                            "rev": null,
                            "vendor": null,
                            "zoned": "none",
                            "dax": false
                            },{
                            "name": "/dev/sda3",
                            "kname": "/dev/sda3",
                            "path": "/dev/sda3",
                            "maj:min": "8:3",
                            "fsavail": null,
                            "fssize": null,
                            "fstype": null,
                            "fsused": null,
                            "fsuse%": null,
                            "fsroots": [
                                null
                            ],
                            "fsver": null,
                            "mountpoint": null,
                            "mountpoints": [
                                null
                            ],
                            "label": null,
                            "uuid": null,
                            "ptuuid": null,
                            "pttype": null,
                            "parttype": "4f68bce3-e8cd-4db1-96e7-fbcaf984b709",
                            "parttypename": null,
                            "partlabel": "root-b",
                            "partuuid": "8fa573dd-b810-4aa0-bdc6-736e157cf9be",
                            "partflags": "0x800000000000000",
                            "ra": 128,
                            "ro": false,
                            "rm": false,
                            "hotplug": false,
                            "model": null,
                            "serial": null,
                            "size": 2147483648,
                            "state": null,
                            "owner": "root",
                            "group": "disk",
                            "mode": "brw-rw----",
                            "alignment": 0,
                            "min-io": 512,
                            "opt-io": 0,
                            "phy-sec": 512,
                            "log-sec": 512,
                            "rota": true,
                            "sched": "mq-deadline",
                            "rq-size": 64,
                            "type": "part",
                            "disc-aln": 0,
                            "disc-gran": 512,
                            "disc-max": 2147450880,
                            "disc-zero": false,
                            "wsame": 0,
                            "wwn": null,
                            "rand": true,
                            "pkname": "/dev/sda",
                            "hctl": null,
                            "tran": null,
                            "subsystems": "block:scsi:pci",
                            "rev": null,
                            "vendor": null,
                            "zoned": "none",
                            "dax": false
                            },{
                            "name": "/dev/sda4",
                            "kname": "/dev/sda4",
                            "path": "/dev/sda4",
                            "maj:min": "8:4",
                            "fsavail": null,
                            "fssize": null,
                            "fstype": "swap",
                            "fsused": null,
                            "fsuse%": null,
                            "fsroots": [
                                null
                            ],
                            "fsver": null,
                            "mountpoint": "[SWAP]",
                            "mountpoints": [
                                "[SWAP]"
                            ],
                            "label": null,
                            "uuid": "fdb10022-0907-411c-b0a6-9847c8d2b32e",
                            "ptuuid": null,
                            "pttype": null,
                            "parttype": "0657fd6d-a4ab-43c4-84e5-0933c84b4f4f",
                            "parttypename": null,
                            "partlabel": "swap",
                            "partuuid": "84dfeee4-7225-4379-ac73-b4a20c0a178d",
                            "partflags": null,
                            "ra": 128,
                            "ro": false,
                            "rm": false,
                            "hotplug": false,
                            "model": null,
                            "serial": null,
                            "size": 2147483648,
                            "state": null,
                            "owner": "root",
                            "group": "disk",
                            "mode": "brw-rw----",
                            "alignment": 0,
                            "min-io": 512,
                            "opt-io": 0,
                            "phy-sec": 512,
                            "log-sec": 512,
                            "rota": true,
                            "sched": "mq-deadline",
                            "rq-size": 64,
                            "type": "part",
                            "disc-aln": 0,
                            "disc-gran": 512,
                            "disc-max": 2147450880,
                            "disc-zero": false,
                            "wsame": 0,
                            "wwn": null,
                            "rand": true,
                            "pkname": "/dev/sda",
                            "hctl": null,
                            "tran": null,
                            "subsystems": "block:scsi:pci",
                            "rev": null,
                            "vendor": null,
                            "zoned": "none",
                            "dax": false
                            },{
                            "name": "/dev/sda5",
                            "kname": "/dev/sda5",
                            "path": "/dev/sda5",
                            "maj:min": "8:5",
                            "fsavail": "85109760",
                            "fssize": "92500992",
                            "fstype": "ext4",
                            "fsused": "51200",
                            "fsuse%": "0%",
                            "fsroots": [
                                "/"
                            ],
                            "fsver": null,
                            "mountpoint": "/var/lib/trident",
                            "mountpoints": [
                                "/var/lib/trident"
                            ],
                            "label": null,
                            "uuid": "bd7cd9c1-3a16-4c75-a429-7540eb7f0c60",
                            "ptuuid": null,
                            "pttype": null,
                            "parttype": "0fc63daf-8483-4772-8e79-3d69d8477de4",
                            "parttypename": null,
                            "partlabel": "trident",
                            "partuuid": "60c8f863-0857-47c4-b427-ba44654c93fe",
                            "partflags": null,
                            "ra": 128,
                            "ro": false,
                            "rm": false,
                            "hotplug": false,
                            "model": null,
                            "serial": null,
                            "size": 104857600,
                            "state": null,
                            "owner": "root",
                            "group": "disk",
                            "mode": "brw-rw----",
                            "alignment": 0,
                            "min-io": 512,
                            "opt-io": 0,
                            "phy-sec": 512,
                            "log-sec": 512,
                            "rota": true,
                            "sched": "mq-deadline",
                            "rq-size": 64,
                            "type": "part",
                            "disc-aln": 0,
                            "disc-gran": 512,
                            "disc-max": 2147450880,
                            "disc-zero": false,
                            "wsame": 0,
                            "wwn": null,
                            "rand": true,
                            "pkname": "/dev/sda",
                            "hctl": null,
                            "tran": null,
                            "subsystems": "block:scsi:pci",
                            "rev": null,
                            "vendor": null,
                            "zoned": "none",
                            "dax": false
                            }
                        ]
                    }
                ]
            }
        "#,
    };

    #[test]
    fn test_find_recursive_single_match() {
        let block_devices = vec![
            BlockDevice {
                name: "sda".to_string(),
                children: vec![],
                ..Default::default()
            },
            BlockDevice {
                name: "sdb".to_string(),
                children: vec![],
                ..Default::default()
            },
        ];

        let mut device_names_seen = HashSet::new();
        let mut matching_block_devices = Vec::new();

        find_recursive(
            &block_devices,
            &|device: &BlockDevice| device.name == "sdb",
            &mut device_names_seen,
            &mut matching_block_devices,
        );

        assert_eq!(matching_block_devices.len(), 1);
        assert_eq!(matching_block_devices[0].name, "sdb");
    }

    #[test]
    fn test_find_recursive_nested_match() {
        let block_devices = vec![BlockDevice {
            name: "sda".to_string(),
            children: vec![BlockDevice {
                name: "sda1".to_string(),
                children: vec![],
                ..Default::default()
            }],
            ..Default::default()
        }];

        let mut device_names_seen = HashSet::new();
        let mut matching_block_devices = Vec::new();

        find_recursive(
            &block_devices,
            &|device: &BlockDevice| device.name == "sda1",
            &mut device_names_seen,
            &mut matching_block_devices,
        );

        assert_eq!(matching_block_devices.len(), 1);
        assert_eq!(matching_block_devices[0].name, "sda1");
    }

    #[test]
    fn test_find_recursive_no_duplicates() {
        let block_devices = vec![
            BlockDevice {
                name: "sda".to_string(),
                children: vec![BlockDevice {
                    name: "sda1".to_string(),
                    children: vec![],
                    ..Default::default()
                }],
                ..Default::default()
            },
            BlockDevice {
                name: "sda".to_string(),
                children: vec![],
                ..Default::default()
            },
        ];

        let mut device_names_seen = HashSet::new();
        let mut matching_block_devices = Vec::new();

        find_recursive(
            &block_devices,
            &|device: &BlockDevice| device.name == "sda",
            &mut device_names_seen,
            &mut matching_block_devices,
        );

        assert_eq!(matching_block_devices.len(), 1); // "sda" should only appear once.
        assert_eq!(matching_block_devices[0].name, "sda");
    }

    #[test]
    fn test_parse_lsblk_output() {
        let expected_block_device_list = vec![BlockDevice {
            name: "/dev/sda".into(),
            fstype: None,
            fssize: None,
            fsuuid: None,
            ptuuid: Some("a8dbca6f-77a6-485c-8c67-b653758a8928".into()),
            part_uuid: None,
            size: 17179869184,
            parent_kernel_name: None,
            readonly: false,
            blkdev_type: BlockDeviceType::Disk,
            children: vec![
                BlockDevice {
                    name: "/dev/sda1".into(),
                    fstype: Some("vfat".into()),
                    fssize: Some(ByteCount(52293632)),
                    fsuuid: Some(OsUuid::Relaxed("C19C-752D".to_string())),
                    ptuuid: None,
                    part_uuid: Some("24d90361-7b1f-47db-b5bb-7d3893ac6ab0".into()),
                    size: 52428800,
                    parent_kernel_name: Some("/dev/sda".into()),
                    children: vec![],
                    mountpoint: Some("/boot/efi".into()),
                    mountpoints: vec!["/boot/efi".into()],
                    partition_table_type: None,
                    readonly: false,
                    blkdev_type: BlockDeviceType::Partition,
                },
                BlockDevice {
                    name: "/dev/sda2".into(),
                    fstype: Some("ext4".into()),
                    fssize: Some(ByteCount(5264343040)),
                    fsuuid: Some("278a7e61-8212-4c84-8103-c8b2fd299670".into()),
                    ptuuid: None,
                    part_uuid: Some("13fe614e-f738-4025-bc7f-8c71a3b8242a".into()),
                    size: 5368709120,
                    parent_kernel_name: Some("/dev/sda".into()),
                    children: vec![],
                    mountpoint: Some("/".into()),
                    mountpoints: vec!["/".into()],
                    partition_table_type: None,
                    readonly: false,
                    blkdev_type: BlockDeviceType::Partition,
                },
                BlockDevice {
                    name: "/dev/sda3".into(),
                    fstype: None,
                    fssize: None,
                    fsuuid: None,
                    ptuuid: None,
                    part_uuid: Some("8fa573dd-b810-4aa0-bdc6-736e157cf9be".into()),
                    size: 2147483648,
                    parent_kernel_name: Some("/dev/sda".into()),
                    children: vec![],
                    mountpoint: None,
                    mountpoints: vec![],
                    partition_table_type: None,
                    readonly: false,
                    blkdev_type: BlockDeviceType::Partition,
                },
                BlockDevice {
                    name: "/dev/sda4".into(),
                    fstype: Some("swap".into()),
                    fssize: None,
                    fsuuid: Some("fdb10022-0907-411c-b0a6-9847c8d2b32e".into()),
                    ptuuid: None,
                    part_uuid: Some("84dfeee4-7225-4379-ac73-b4a20c0a178d".into()),
                    size: 2147483648,
                    parent_kernel_name: Some("/dev/sda".into()),
                    children: vec![],
                    mountpoint: Some("[SWAP]".into()),
                    mountpoints: vec!["[SWAP]".into()],
                    partition_table_type: None,
                    readonly: false,
                    blkdev_type: BlockDeviceType::Partition,
                },
                BlockDevice {
                    name: "/dev/sda5".into(),
                    fstype: Some("ext4".into()),
                    fssize: Some(ByteCount(92500992)),
                    fsuuid: Some("bd7cd9c1-3a16-4c75-a429-7540eb7f0c60".into()),
                    ptuuid: None,
                    part_uuid: Some("60c8f863-0857-47c4-b427-ba44654c93fe".into()),
                    size: 104857600,
                    parent_kernel_name: Some("/dev/sda".into()),
                    children: vec![],
                    mountpoint: Some("/var/lib/trident".into()),
                    mountpoints: vec!["/var/lib/trident".into()],
                    partition_table_type: None,
                    readonly: false,
                    blkdev_type: BlockDeviceType::Partition,
                },
            ],
            mountpoint: None,
            mountpoints: vec![],
            partition_table_type: Some(PartitionTableType::Gpt),
        }];

        let block_device_list = parse_lsblk_output(SAMPLE_LSBLK_OUTPUT).unwrap();
        assert_eq!(block_device_list, expected_block_device_list);

        parse_lsblk_output("bad output").unwrap_err();
    }

    #[test]
    fn test_get_all_mountpoints_recursive() {
        let parsed = parse_lsblk_output(SAMPLE_LSBLK_OUTPUT).unwrap();
        assert_eq!(parsed.len(), 1);
        let block_device = &parsed[0];

        let mount_point_list = block_device.get_all_mountpoints_recursive();
        println!("{:#?}", mount_point_list);
        assert_eq!(mount_point_list.len(), 4, "Expected 4 mount points");

        assert!(mount_point_list.contains(&Path::new("/boot/efi")));
        assert!(mount_point_list.contains(&Path::new("/")));
        assert!(mount_point_list.contains(&Path::new("[SWAP]")));
        assert!(mount_point_list.contains(&Path::new("/var/lib/trident")));
    }

    #[test]
    fn test_skip_nulls() {
        #[derive(Debug, Deserialize, PartialEq, Eq)]
        struct TestStruct {
            #[serde(default, deserialize_with = "skip_nulls")]
            mountpoints: Vec<String>,
        }

        let actual = serde_json::from_str::<TestStruct>(indoc::indoc!(
            r#"
            {
                "mountpoints": [
                    "a",
                    null,
                    "b",
                    null
                ]
            }"#,
        ))
        .unwrap();

        let expected = TestStruct {
            mountpoints: vec!["a".into(), "b".into()],
        };

        assert_eq!(actual, expected);
    }

    #[test]
    fn test_bad_uuids_parsing() {
        let output = indoc::indoc!(
            r#"
            {
                "blockdevices": [
                    {
                        "name": "/dev/sda1",
                        "kname": "/dev/sda1",
                        "path": "/dev/sda1",
                        "maj:min": "8:1",
                        "fsavail": "49911808",
                        "fssize": "52293632",
                        "fstype": "vfat",
                        "fsused": "2381824",
                        "fsuse%": "5%",
                        "fsroots": [
                            "/"
                        ],
                        "fsver": null,
                        "mountpoint": "/boot/efi",
                        "mountpoints": [
                            "/boot/efi"
                        ],
                        "label": null,
                        "uuid": "B333-37D9",
                        "ptuuid": null,
                        "pttype": null,
                        "parttype": "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
                        "parttypename": null,
                        "partlabel": "esp",
                        "partuuid": "3a9c2054-02",
                        "partflags": null,
                        "ra": 128,
                        "ro": false,
                        "rm": false,
                        "hotplug": false,
                        "model": null,
                        "serial": null,
                        "size": 52428800,
                        "state": null,
                        "owner": "root",
                        "group": "disk",
                        "mode": "brw-rw----",
                        "alignment": 0,
                        "min-io": 512,
                        "opt-io": 0,
                        "phy-sec": 512,
                        "log-sec": 512,
                        "rota": true,
                        "sched": "mq-deadline",
                        "rq-size": 64,
                        "type": "part",
                        "disc-aln": 0,
                        "disc-gran": 512,
                        "disc-max": 2147450880,
                        "disc-zero": false,
                        "wsame": 0,
                        "wwn": null,
                        "rand": true,
                        "pkname": "/dev/sda",
                        "hctl": null,
                        "tran": null,
                        "subsystems": "block:scsi:pci",
                        "rev": null,
                        "vendor": null,
                        "zoned": "none",
                        "dax": false
                    }
                ]
            }
        "#
        );

        let block_device_list = parse_lsblk_output(output).unwrap();

        assert_eq!(block_device_list.len(), 1);

        let block_device = &block_device_list[0];

        assert_eq!(
            block_device.part_uuid,
            Some(OsUuid::Relaxed("3a9c2054-02".into()))
        );
    }

    #[test]
    fn test_azl3_lsblk_output() {
        let output = r#"{
            "blockdevices": [
               {
                  "alignment": 0,
                  "id-link": null,
                  "id": null,
                  "disc-aln": 0,
                  "dax": false,
                  "disc-gran": 512,
                  "disk-seq": 27,
                  "disc-max": 2147450880,
                  "disc-zero": false,
                  "fsavail": 1069228032,
                  "fsroots": [
                      "/"
                  ],
                  "fssize": 1071624192,
                  "fstype": "vfat",
                  "fsused": 2396160,
                  "fsuse%": "0%",
                  "fsver": null,
                  "group": "disk",
                  "hctl": null,
                  "hotplug": false,
                  "kname": "/dev/sda1",
                  "label": null,
                  "log-sec": 512,
                  "maj:min": "8:1",
                  "min-io": 512,
                  "mode": "brw-rw----",
                  "model": null,
                  "mq": "  1",
                  "name": "/dev/sda1",
                  "opt-io": 0,
                  "owner": "root",
                  "partflags": null,
                  "partlabel": "esp",
                  "partn": 1,
                  "parttype": "c12a7328-f81f-11d2-ba4b-00a0c93ec93b",
                  "parttypename": null,
                  "partuuid": "87df7565-0e88-4c95-91dd-ac5a3259afb5",
                  "path": "/dev/sda1",
                  "phy-sec": 512,
                  "pkname": "/dev/sda",
                  "pttype": null,
                  "ptuuid": null,
                  "ra": 128,
                  "rand": true,
                  "rev": null,
                  "rm": false,
                  "ro": false,
                  "rota": true,
                  "rq-size": 64,
                  "sched": "bfq",
                  "serial": null,
                  "size": 1073741824,
                  "start": 2048,
                  "state": null,
                  "subsystems": "block:scsi:pci",
                  "mountpoint": "/mnt/newroot/boot/efi",
                  "mountpoints": [
                      "/mnt/newroot/boot/efi"
                  ],
                  "tran": null,
                  "type": "part",
                  "uuid": "FCE7-4962",
                  "vendor": null,
                  "wsame": 0,
                  "wwn": null,
                  "zoned": "none",
                  "zone-sz": 0,
                  "zone-wgran": 0,
                  "zone-app": 0,
                  "zone-nr": 0,
                  "zone-omax": 0,
                  "zone-amax": 0
               }
            ]
         }"#;

        parse_lsblk_output(output).unwrap();
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use pytest_gen::functional_test;

    use super::*;

    #[functional_test(feature = "helpers")]
    fn test_find_disk_success() {
        let block_devices = super::find(|b| b.blkdev_type == BlockDeviceType::Disk).unwrap();

        assert_eq!(block_devices.len(), 2);
        assert_eq!(block_devices[0].name, "sda");
        assert_eq!(block_devices[0].children.len(), 5);
        assert_eq!(block_devices[1].name, "sdb");
        assert_eq!(block_devices[1].children.len(), 0);
    }

    #[functional_test(feature = "helpers")]
    fn test_find_partitions_success() {
        let block_devices = super::find(|b| b.blkdev_type == BlockDeviceType::Partition).unwrap();

        assert_eq!(block_devices.len(), 5);
        assert_eq!(block_devices[0].name, "sda1");
        assert_eq!(block_devices[0].children.len(), 0);
        assert_eq!(block_devices[1].name, "sda2");
        assert_eq!(block_devices[1].children.len(), 0);
        assert_eq!(block_devices[2].name, "sda3");
        assert_eq!(block_devices[2].children.len(), 0);
        assert_eq!(block_devices[3].name, "sda4");
        assert_eq!(block_devices[3].children.len(), 0);
        assert_eq!(block_devices[4].name, "sda5");
        assert_eq!(block_devices[4].children.len(), 0);
    }

    #[functional_test(feature = "helpers")]
    fn test_list_success() {
        let block_devices = super::list().unwrap();

        assert_ne!(block_devices.len(), 0);
        assert_ne!(block_devices[0].name, "");
    }

    #[functional_test(feature = "helpers")]
    fn test_get_success() {
        let block_device = super::get(Path::new("/dev/sda")).unwrap();

        assert_eq!(block_device.name, "/dev/sda");
        assert_eq!(block_device.children.len(), 5);
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_get_fail_on_non_block_file() {
        assert!(super::get(Path::new("/dev/null")).unwrap_err().root_cause().to_string().contains("stdout:\n{\n   \"blockdevices\": [\n\n   ]\n}\n\n\nstderr:\nlsblk: /dev/null: not a block device\n\n"));
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_get_fail_on_missing_file() {
        assert!(super::get(Path::new("/dev/does-not-exist")).unwrap_err().root_cause().to_string().contains("stdout:\n{\n   \"blockdevices\": [\n\n   ]\n}\n\n\nstderr:\nlsblk: /dev/does-not-exist: not a block device\n\n"));
    }
}
