use std::path::Path;

use anyhow::Error;

use sysdefs::partition_types::DiscoverablePartitionType;

use crate::{dependencies::Dependency, repart::RepartPartitionEntry};

pub const DISK_SIZE: u64 = 16 * 1024 * 1024 * 1024; // 16 GiB
pub const PART1_SIZE: u64 = 50 * 1024 * 1024; // 50 MiB
pub const PART2_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB disk - 1 MiB prefix - 50 MiB ESP - 20 KiB (rounding?)
pub const PART3_SIZE: u64 = 1024 * 1024 * 1024; // 1 GiB disk - 1 MiB prefix - 50 MiB ESP - 2 GiB root - 20 KiB (rounding?)

pub const SIZE_100MIB: u64 = 100 * 1024 * 1024;

pub const OS_DISK_DEVICE_PATH: &str = "/dev/sda";
pub const TEST_DISK_DEVICE_PATH: &str = "/dev/sdb";
pub const CDROM_DEVICE_PATH: &str = "/dev/sr0";
pub const CDROM_MOUNT_PATH: &str = "/mnt/cdrom";

pub fn generate_partition_definition_esp_generic() -> Vec<RepartPartitionEntry> {
    vec![
        RepartPartitionEntry {
            id: "esp".to_string(),
            partition_type: DiscoverablePartitionType::Esp,
            label: None,
            size_min_bytes: Some(PART1_SIZE),
            size_max_bytes: Some(PART1_SIZE),
        },
        RepartPartitionEntry {
            id: "root".to_string(),
            partition_type: DiscoverablePartitionType::LinuxGeneric,
            label: None,
            // When min==max==None, it's a grow partition
            size_min_bytes: None,
            size_max_bytes: None,
        },
    ]
}

pub fn generate_partition_definition_esp_root_generic() -> Vec<RepartPartitionEntry> {
    vec![
        RepartPartitionEntry {
            id: "esp".to_string(),
            partition_type: DiscoverablePartitionType::Esp,
            label: None,
            size_min_bytes: Some(PART1_SIZE),
            size_max_bytes: Some(PART1_SIZE),
        },
        RepartPartitionEntry {
            id: "root".to_string(),
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(PART2_SIZE),
            size_max_bytes: Some(PART2_SIZE),
        },
        RepartPartitionEntry {
            id: "generic".to_string(),
            partition_type: DiscoverablePartitionType::LinuxGeneric,
            label: None,
            // When min==max==None, it's a grow partition
            size_min_bytes: None,
            size_max_bytes: None,
        },
    ]
}

pub fn generate_partition_definition_boot_root_verity() -> Vec<RepartPartitionEntry> {
    vec![
        RepartPartitionEntry {
            id: "boot".to_string(),
            partition_type: DiscoverablePartitionType::Xbootldr,
            label: None,
            size_min_bytes: Some(SIZE_100MIB),
            size_max_bytes: Some(SIZE_100MIB),
        },
        RepartPartitionEntry {
            id: "root-verity".to_string(),
            partition_type: DiscoverablePartitionType::RootVerity,
            label: None,
            size_min_bytes: Some(SIZE_100MIB),
            size_max_bytes: Some(SIZE_100MIB),
        },
        RepartPartitionEntry {
            id: "root".to_string(),
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(SIZE_100MIB),
            size_max_bytes: Some(SIZE_100MIB),
        },
        // For tests that do ab-update stuff and require these to exist
        RepartPartitionEntry {
            id: "root-verity-b".to_string(),
            partition_type: DiscoverablePartitionType::RootVerity,
            label: None,
            size_min_bytes: Some(SIZE_100MIB),
            size_max_bytes: Some(SIZE_100MIB),
        },
        RepartPartitionEntry {
            id: "root-b".to_string(),
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(SIZE_100MIB),
            size_max_bytes: Some(SIZE_100MIB),
        },
    ]
}

pub fn generate_partition_definition_esp_root_raid_single_disk() -> Vec<RepartPartitionEntry> {
    vec![
        RepartPartitionEntry {
            id: "esp".to_string(),
            partition_type: DiscoverablePartitionType::Esp,
            label: None,
            size_min_bytes: Some(PART1_SIZE),
            size_max_bytes: Some(PART1_SIZE),
        },
        RepartPartitionEntry {
            id: "root-a".to_string(),
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(PART2_SIZE),
            size_max_bytes: Some(PART2_SIZE),
        },
        RepartPartitionEntry {
            id: "root-b".to_string(),
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(PART2_SIZE),
            size_max_bytes: Some(PART2_SIZE),
        },
        RepartPartitionEntry {
            id: "generic".to_string(),
            partition_type: DiscoverablePartitionType::LinuxGeneric,
            label: None,
            // When min==max==None, it's a grow partition
            size_min_bytes: None,
            size_max_bytes: None,
        },
    ]
}

pub fn generate_partition_definition_esp_root_raid_single_disk_unequal() -> Vec<RepartPartitionEntry>
{
    vec![
        RepartPartitionEntry {
            id: "esp".to_string(),
            partition_type: DiscoverablePartitionType::Esp,
            label: None,
            size_min_bytes: Some(PART1_SIZE),
            size_max_bytes: Some(PART1_SIZE),
        },
        RepartPartitionEntry {
            id: "root-a".to_string(),
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(PART2_SIZE),
            size_max_bytes: Some(PART2_SIZE),
        },
        RepartPartitionEntry {
            id: "root-b".to_string(),
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(PART3_SIZE),
            size_max_bytes: Some(PART3_SIZE),
        },
        RepartPartitionEntry {
            id: "generic".to_string(),
            partition_type: DiscoverablePartitionType::LinuxGeneric,
            label: None,
            // When min==max==None, it's a grow partition
            size_min_bytes: None,
            size_max_bytes: None,
        },
    ]
}

pub fn clear_disk(disk_path: &Path) -> Result<(), Error> {
    Dependency::Dd
        .cmd()
        .arg("if=/dev/zero")
        .arg(format!("of={}", disk_path.to_string_lossy()))
        .arg("bs=1M")
        .arg("count=1")
        .run_and_check()?;
    Ok(())
}
