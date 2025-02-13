use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use trident_api::{
    config::{
        Disk, FileSystem, FileSystemSource, FileSystemType, HostConfiguration, Image, ImageFormat,
        ImageSha256, MountOptions, MountPoint, Partition, PartitionSize, PartitionTableType,
        PartitionType,
    },
    misc::IdGenerator,
};

use crate::{
    commands::partition::{FsType, PartitionMount},
    data::ParsedData,
    SetsailError,
};

pub fn translate(input: &ParsedData, hc: &mut HostConfiguration, errors: &mut Vec<SetsailError>) {
    let mut disk_id_gen = IdGenerator::new("disk");

    // Create a HashMap of all disks
    // The /dev/? path is to be used as the key
    let mut disks: HashMap<String, Disk> = HashMap::new();

    // List of all filesystems
    let mut filesystems: Vec<FileSystem> = Vec::new();

    // Go over all parsed partitions
    for part in input.partitions.iter() {
        // Get the /dev/? path of the target disk
        let target_disk = match part
            .ondisk
            .as_ref()
            .cloned()
            .unwrap_or("/dev/sda".to_string())
            .clone()
        {
            s if s.starts_with("/dev/") => s,
            s => format!("/dev/{}", s),
        };

        // Get/insert the disk in the HashMap
        let disk = disks.entry(target_disk.clone()).or_insert_with(|| Disk {
            // We haven't seen this disk before, generate a new ID
            id: disk_id_gen.next_id(),
            // Use the /dev/? path as the device
            device: PathBuf::from(target_disk),
            // We only support GPT
            partition_table_type: PartitionTableType::Gpt,
            // No partitions yet
            partitions: Vec::new(),
            ..Default::default()
        });

        // Generate a new ID for this partition of the form <disk_id>-<partition_count>
        let partition_id = format!("{}-{}", &disk.id, disk.partitions.len());

        // Push this partition into the disk list
        disk.partitions.push(Partition {
            id: partition_id.clone(),
            partition_type: match part.mntpoint {
                PartitionMount::Path(ref s) => path_to_partition_type(s),
                PartitionMount::Swap => PartitionType::Swap,
                _ => {
                    errors.push(SetsailError::new_translation(
                        part.line.clone(),
                        format!("Unsupported partition type: {}", part.mntpoint),
                    ));
                    continue;
                }
            },
            size: {
                // The parser ensures that only one of grow or size is set
                if part.grow {
                    PartitionSize::Grow
                } else if let Some(size) = part.size {
                    // Transform MiB to bytes
                    PartitionSize::Fixed((size << 20).into())
                } else {
                    errors.push(SetsailError::new_translation(
                        part.line.clone(),
                        "Partition size not specified".to_string(),
                    ));
                    continue;
                }
            },
        });

        filesystems.push(FileSystem {
            device_id: Some(partition_id.clone()),
            fs_type: part.fstype.into(),
            // TODO(5989): Figure out how to bridge the gap between how kickstart
            // handles images and how Trident handles them
            source: match part.image.as_ref() {
                Some(img) => FileSystemSource::Image(Image {
                    url: img.clone(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                }),
                None => FileSystemSource::New,
            },
            mount_point: match part.mntpoint {
                PartitionMount::Path(ref s) => Some(MountPoint {
                    path: s.clone(),
                    options: MountOptions(part.fsoptions.join(",")),
                }),
                PartitionMount::Swap => None,
                _ => {
                    errors.push(SetsailError::new_translation(
                        part.line.clone(),
                        "Unsupported mountpoint".to_string(),
                    ));
                    continue;
                }
            },
        });
    }

    hc.storage.disks = disks.into_values().collect();
    hc.storage.filesystems = filesystems;
}

impl From<FsType> for FileSystemType {
    fn from(value: FsType) -> Self {
        match value {
            FsType::Ext4 => FileSystemType::Ext4,
            FsType::Vfat => FileSystemType::Vfat,
            FsType::Efi => FileSystemType::Vfat,
            FsType::Swap => FileSystemType::Swap,
        }
    }
}

fn path_to_partition_type(value: &Path) -> PartitionType {
    let p = value.as_os_str();
    // We need to do an if/else chain as opposed to a match because
    // we want to use OsStr::PartialEq<str>
    if p == "/" {
        PartitionType::Root
    } else if p == "/boot/efi" {
        PartitionType::Esp
    } else if p == "/var" {
        PartitionType::Var
    } else if p == "/home" {
        PartitionType::Home
    } else if p == "/usr" {
        PartitionType::Usr
    } else if p == "/tmp" {
        PartitionType::Tmp
    } else {
        PartitionType::LinuxGeneric
    }
}
