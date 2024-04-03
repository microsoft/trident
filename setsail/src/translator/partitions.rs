use std::path::Path;
use std::{collections::HashMap, path::PathBuf};

use trident_api::config::{
    Disk, FileSystemType, HostConfiguration, Image, ImageFormat, ImageSha256, MountPoint,
    Partition, PartitionSize, PartitionTableType, PartitionType,
};

use crate::commands::partition::{FsType, PartitionMount};
use crate::{data::ParsedData, SetsailError};

use super::misc::IdGenerator;

pub fn translate(input: &ParsedData, hc: &mut HostConfiguration, errors: &mut Vec<SetsailError>) {
    let mut disk_id_gen = IdGenerator::new("disk".into());

    // Create a HashMap of all disks
    // The /dev/? path is to be used as the key
    let mut disks: HashMap<String, Disk> = HashMap::new();

    // Create a list of all mount points
    let mut mount_points: Vec<MountPoint> = Vec::new();

    // Create a list of all images to be mounted on partitions
    let mut images: Vec<Image> = Vec::new();

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
            id: disk_id_gen.next(),
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
                    PartitionSize::Fixed(size << 20)
                } else {
                    errors.push(SetsailError::new_translation(
                        part.line.clone(),
                        "Partition size not specified".to_string(),
                    ));
                    continue;
                }
            },
        });

        mount_points.push(MountPoint {
            path: match part.mntpoint {
                PartitionMount::Path(ref s) => s.clone(),
                PartitionMount::Swap => PathBuf::from("swap"),
                _ => {
                    errors.push(SetsailError::new_translation(
                        part.line.clone(),
                        "Unsupported mountpoint".to_string(),
                    ));
                    continue;
                }
            },
            filesystem: part.fstype.into(),
            options: part.fsoptions.clone(),
            target_id: partition_id.clone(),
        });

        // TODO(5989): Figure out how to bridge the gap between how kickstart
        // handles images and how Trident handles them
        if let Some(img) = part.image.as_ref() {
            images.push(Image {
                url: img.clone(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
                target_id: partition_id.clone(),
            });
        }
    }

    hc.storage.disks = disks.into_values().collect();
    hc.storage.mount_points = mount_points;
    hc.storage.images = images;
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
