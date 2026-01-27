use std::path::PathBuf;

use anyhow::{bail, Context, Error};

use log::warn;
use osutils::lsblk::{self, BlockDevice, BlockDeviceType};

use trident_api::{
    config::{Disk, HostConfiguration, Partition},
    error::{ReportError, TridentError, UnsupportedConfigurationError},
};

/// Extra bytes to account for GPT overhead. (34,304 bytes)
///
/// Calculated as:
///
/// Sectors:
///  - Protective MBR:                 1 sector
///  - Primary GPT Header:             1 sector
///  - Primary GPT Partition Entries: 32 sectors
///  - Backup GPT Header:              1 sector
///  - Backup GPT Partition Entries:  32 sectors
/// -----------------------------------------------
///    Total Sectors:                 67 sectors
///
/// Sector Size:
///  - 512 bytes/sector
/// -----------------------------------------------
///    Total Bytes:               34,304 bytes
///
/// Note: This assumes each GPT partition entry is 128 bytes and there are 128
/// entries (the default).
const GPT_EXTRA_BYTES: u64 = 512 * (1 + 33 + 33); // 34,304 bytes

/// Strategy for selecting a disk from a list of candidates.
pub(super) enum DiskSelectionStrategy {
    /// Select the smallest disk that will fit the minimum size requirement.
    SmallestThatWillFit,
}

/// Updates the target disk path in the Host Configuration based on the given strategy.
pub(super) fn update_target_disk_path(
    host_config: &mut HostConfiguration,
    strategy: DiskSelectionStrategy,
) -> Result<(), TridentError> {
    update_target_disk_path_with_candidates(host_config, strategy, get_candidates())
        .structured(UnsupportedConfigurationError::NoSuitableDisk)
}

/// Updates the target disk path in the Host Configuration based on the given strategy and candidates.
fn update_target_disk_path_with_candidates(
    host_config: &mut HostConfiguration,
    strategy: DiskSelectionStrategy,
    candidates: Vec<BlockDevice>,
) -> Result<(), Error> {
    let Some(disk) = host_config.storage.disks.get_mut(0) else {
        bail!("Host Configuration does not specify any target disks");
    };

    let selection = match strategy {
        DiskSelectionStrategy::SmallestThatWillFit => smallest_that_will_fit(candidates, disk),
    }
    .context("Failed to select target disk")?;

    disk.device = selection;

    Ok(())
}

/// Returns a list of candidate block devices.
fn get_candidates() -> Vec<BlockDevice> {
    let allowed_kinds = ["sd", "nvme", "vd", "hd", "mmcblk"];

    lsblk::list()
        .unwrap_or_default()
        .into_iter()
        // Limit to block devices of type 'disk'.
        .filter(|b| b.blkdev_type == BlockDeviceType::Disk)
        .filter_map(|b| {
            // Ensure the block device is of an allowed kind.
            allowed_kinds.iter().find(|k| b.name.starts_with(**k))?;
            Some(b)
        })
        .collect()
}

/// Computes the minimum required size in bytes of a disk.
fn required_size_bytes(disk: &Disk) -> Result<u64, Error> {
    let total_size: u64 = disk
        .partitions
        .iter()
        .map(pad_to_4k)
        .collect::<Result<Vec<u64>, Error>>()?
        .into_iter()
        .sum();

    Ok(total_size + GPT_EXTRA_BYTES)
}

/// Pads the partition size to the next 4KiB boundary if necessary.
fn pad_to_4k(part: &Partition) -> Result<u64, Error> {
    let size = part
        .size
        .to_bytes()
        .with_context(|| format!("Partition '{}' does not have a fixed size", part.id))?;
    if size % 4096 == 0 {
        Ok(size)
    } else {
        warn!(
            "Partition '{}' size {} is not aligned to 4KiB, padding to next 4KiB boundary",
            part.id, size
        );
        Ok(size + (4096 - (size % 4096)))
    }
}

/// Finds the smallest block device that will fit the required size.
fn smallest_that_will_fit(candidates: Vec<BlockDevice>, disk: &Disk) -> Result<PathBuf, Error> {
    let required_size = required_size_bytes(disk)?;

    candidates
        .iter()
        .filter(|b| b.size >= required_size)
        .min_by_key(|b| b.size)
        .map(|b| b.device_path())
        .with_context(|| {
            format!(
                "No block device found with required size of at least {} bytes",
                required_size
            )
        })
}

#[cfg(test)]
mod tests {
    use trident_api::config::{PartitionSize, Storage};

    use super::*;

    #[test]
    fn test_pad_to_4k_aligned() {
        let part = Partition::new("part1", 8192);
        let padded_size = pad_to_4k(&part).unwrap();
        assert_eq!(padded_size, 8192);

        let part2 = Partition::new("part2", 16384);
        let padded_size2 = pad_to_4k(&part2).unwrap();
        assert_eq!(padded_size2, 16384);

        // Test a size that is not aligned to 4KiB
        let part3 = Partition::new("part3", 4097);
        let padded_size3 = pad_to_4k(&part3).unwrap();
        assert_eq!(padded_size3, 8192);

        let part4 = Partition::new("part4", PartitionSize::Grow);
        pad_to_4k(&part4).unwrap_err();
    }

    #[test]
    fn test_required_size_bytes() {
        let mut disk = Disk {
            device: PathBuf::from("/dev/sda"),
            partitions: vec![],
            id: "disk0".to_string(),
            partition_table_type: Default::default(),
            adopted_partitions: Default::default(),
        };

        disk.partitions.push(Partition::new("part1", 8192));
        let required_size = required_size_bytes(&disk).unwrap();
        // 8192 + 34304 (GPT overhead) = 42496
        assert_eq!(required_size, 8192 + GPT_EXTRA_BYTES);

        disk.partitions.push(Partition::new("part2", 16384));
        let required_size = required_size_bytes(&disk).unwrap();
        // 8192 + 16384 + 34304 (GPT overhead) = 58880
        assert_eq!(required_size, 8192 + 16384 + GPT_EXTRA_BYTES);

        disk.partitions
            .push(Partition::new("part3", PartitionSize::Grow));
        required_size_bytes(&disk).unwrap_err();
    }

    #[test]
    fn test_smallest_that_will_fit() {
        let candidates = vec![
            BlockDevice {
                name: "sda".to_string(),
                size: 50 * 1024 * 1024 * 1024, // 50 GiB
                ..Default::default()
            },
            BlockDevice {
                name: "sdb".to_string(),
                size: 100 * 1024 * 1024 * 1024, // 100 GiB
                ..Default::default()
            },
            BlockDevice {
                name: "sdc".to_string(),
                size: 200 * 1024 * 1024 * 1024, // 200 GiB
                ..Default::default()
            },
        ];

        let mut disk = Disk {
            device: PathBuf::from("/dev/sdx"),
            partitions: vec![],
            id: "disk0".to_string(),
            partition_table_type: Default::default(),
            adopted_partitions: Default::default(),
        };

        // Test 1: Require 10 GiB
        disk.partitions
            .push(Partition::new("part1", 10 * 1024 * 1024 * 1024)); // 10 GiB
        let selection = smallest_that_will_fit(candidates.clone(), &disk).unwrap();
        assert_eq!(selection, PathBuf::from("/dev/sda"));

        // Test 2: Require 60 GiB
        disk.partitions.clear();
        disk.partitions
            .push(Partition::new("part1", 60 * 1024 * 1024 * 1024)); // 60 GiB
        let selection = smallest_that_will_fit(candidates.clone(), &disk).unwrap();
        assert_eq!(selection, PathBuf::from("/dev/sdb"));

        // Test 3: Require 150 GiB
        disk.partitions.clear();
        disk.partitions
            .push(Partition::new("part1", 150 * 1024 * 1024 * 1024)); // 150 GiB
        let selection = smallest_that_will_fit(candidates.clone(), &disk).unwrap();
        assert_eq!(selection, PathBuf::from("/dev/sdc"));

        // Test 4: Require 250 GiB (no suitable disk)
        disk.partitions.clear();
        disk.partitions
            .push(Partition::new("part1", 250 * 1024 * 1024 * 1024)); // 250 GiB
        smallest_that_will_fit(candidates.clone(), &disk).unwrap_err();
    }

    #[test]
    fn test_update_target_disk_path_with_candidates() {
        let candidates = vec![
            BlockDevice {
                name: "sda".to_string(),
                size: 50 * 1024 * 1024 * 1024, // 50 GiB
                ..Default::default()
            },
            BlockDevice {
                name: "sdb".to_string(),
                size: 100 * 1024 * 1024 * 1024, // 100 GiB
                ..Default::default()
            },
        ];

        let mut host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    device: PathBuf::from("/dev/sdx"),
                    partitions: vec![Partition::new("part1", 60 * 1024 * 1024 * 1024)], // 60 GiB
                    id: "disk0".to_string(),
                    partition_table_type: Default::default(),
                    adopted_partitions: Default::default(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        update_target_disk_path_with_candidates(
            &mut host_config,
            DiskSelectionStrategy::SmallestThatWillFit,
            candidates,
        )
        .unwrap();

        assert_eq!(
            host_config.storage.disks[0].device,
            PathBuf::from("/dev/sdb")
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use pytest_gen::functional_test;

    #[functional_test]
    fn test_get_candidates() {
        let candidates = super::get_candidates();
        assert_eq!(candidates.len(), 2);
    }
}
