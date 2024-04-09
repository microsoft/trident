use std::{path::Path, process::Command};

use anyhow::Error;

use crate::{
    exe::RunAndCheck, partition_types::DiscoverablePartitionType, repart::RepartPartitionEntry,
};

pub const DISK_SIZE: u64 = 16 * 1024 * 1024 * 1024; // 16 GiB
pub const PART1_SIZE: u64 = 50 * 1024 * 1024; // 50 MiB
pub const PART2_SIZE: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB disk - 1 MiB prefix - 50 MiB ESP - 20 KiB (rounding?)

pub const OS_DISK_DEVICE_PATH: &str = "/dev/sda";
pub const TEST_DISK_DEVICE_PATH: &str = "/dev/sdb";
pub const CDROM_DEVICE_PATH: &str = "/dev/sr0";
pub const CDROM_MOUNT_PATH: &str = "/mnt/cdrom";

pub fn generate_partition_definition_esp_generic() -> Vec<RepartPartitionEntry> {
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

pub fn generate_partition_definition_esp_root_generic() -> Vec<RepartPartitionEntry> {
    vec![
        RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::Esp,
            label: None,
            size_min_bytes: Some(PART1_SIZE),
            size_max_bytes: Some(PART1_SIZE),
        },
        RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(PART2_SIZE),
            size_max_bytes: Some(PART2_SIZE),
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

pub fn generate_partition_definition_boot_root_verity() -> Vec<RepartPartitionEntry> {
    vec![
        RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::Xbootldr,
            label: None,
            size_min_bytes: Some(1024 * 1024 * 1024),
            size_max_bytes: None,
        },
        RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::RootVerity,
            label: None,
            size_min_bytes: Some(1024 * 1024 * 1024),
            size_max_bytes: None,
        },
        RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            // When min==max==None, it's a grow partition
            size_min_bytes: None,
            size_max_bytes: None,
        },
    ]
}

pub fn generate_partition_definition_esp_root_raid_single_disk() -> Vec<RepartPartitionEntry> {
    vec![
        RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::Esp,
            label: None,
            size_min_bytes: Some(PART1_SIZE),
            size_max_bytes: Some(PART1_SIZE),
        },
        RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(PART2_SIZE),
            size_max_bytes: Some(PART2_SIZE),
        },
        RepartPartitionEntry {
            partition_type: DiscoverablePartitionType::Root,
            label: None,
            size_min_bytes: Some(PART2_SIZE),
            size_max_bytes: Some(PART2_SIZE),
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

pub fn clear_disk(disk_path: &Path) -> Result<(), Error> {
    Command::new("dd")
        .arg("if=/dev/zero")
        .arg(format!("of={}", disk_path.to_string_lossy()))
        .arg("bs=1M")
        .arg("count=1")
        .run_and_check()
}
