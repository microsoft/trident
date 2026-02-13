use std::path::PathBuf;

use anyhow::{bail, Context, Error};

use osutils::lsblk::{self, BlockDevice, BlockDeviceType};

use trident_api::{
    config::HostConfiguration,
    error::{ReportError, TridentError, UnsupportedConfigurationError},
};

/// Strategy for selecting a disk from a list of candidates.
pub(super) enum DiskSelectionStrategy {
    /// Select the smallest disk that will fit the minimum size requirement.
    SmallestThatWillFit,
}

/// Updates the target disk path in the Host Configuration based on the given strategy.
pub(super) fn update_target_disk_path(
    host_config: &mut HostConfiguration,
    original_disk_size: u64,
    strategy: DiskSelectionStrategy,
) -> Result<(), TridentError> {
    update_target_disk_path_with_candidates(
        host_config,
        original_disk_size,
        strategy,
        get_candidates(),
    )
    .structured(UnsupportedConfigurationError::NoSuitableDisk)
}

/// Updates the target disk path in the Host Configuration based on the given strategy and candidates.
fn update_target_disk_path_with_candidates(
    host_config: &mut HostConfiguration,
    original_disk_size: u64,
    strategy: DiskSelectionStrategy,
    candidates: Vec<BlockDevice>,
) -> Result<(), Error> {
    let Some(disk) = host_config.storage.disks.get_mut(0) else {
        bail!("Host Configuration does not specify any target disks");
    };

    let selection = match strategy {
        DiskSelectionStrategy::SmallestThatWillFit => {
            smallest_that_will_fit(candidates, original_disk_size)
        }
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

/// Finds the smallest block device that will fit the required size.
fn smallest_that_will_fit(
    candidates: Vec<BlockDevice>,
    original_disk_size: u64,
) -> Result<PathBuf, Error> {
    candidates
        .iter()
        .filter(|b| b.size >= original_disk_size)
        .min_by_key(|b| b.size)
        .map(|b| b.device_path())
        .with_context(|| {
            format!(
                "No block device found with required size of at least {} bytes",
                original_disk_size
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use trident_api::config::{Disk, Partition, Storage};

    const GIGABYTE: u64 = 1024 * 1024 * 1024;

    #[test]
    fn test_smallest_that_will_fit() {
        let candidates = vec![
            BlockDevice {
                name: "sda".to_string(),
                size: 50 * GIGABYTE, // 50 GiB
                ..Default::default()
            },
            BlockDevice {
                name: "sdb".to_string(),
                size: 100 * GIGABYTE, // 100 GiB
                ..Default::default()
            },
            BlockDevice {
                name: "sdc".to_string(),
                size: 200 * GIGABYTE, // 200 GiB
                ..Default::default()
            },
        ];

        // Test 1: Require 10 GiB
        let selection = smallest_that_will_fit(candidates.clone(), 10 * GIGABYTE).unwrap();
        assert_eq!(selection, PathBuf::from("/dev/sda"));

        // Test 2: Require 60 GiB
        let selection = smallest_that_will_fit(candidates.clone(), 60 * GIGABYTE).unwrap();
        assert_eq!(selection, PathBuf::from("/dev/sdb"));

        // Test 3: Require 150 GiB
        let selection = smallest_that_will_fit(candidates.clone(), 150 * GIGABYTE).unwrap();
        assert_eq!(selection, PathBuf::from("/dev/sdc"));

        // Test 4: Require 250 GiB (no suitable disk)
        smallest_that_will_fit(candidates.clone(), 250 * GIGABYTE).unwrap_err();
    }

    #[test]
    fn test_update_target_disk_path_with_candidates() {
        let candidates = vec![
            BlockDevice {
                name: "sda".to_string(),
                size: 50 * GIGABYTE, // 50 GiB
                ..Default::default()
            },
            BlockDevice {
                name: "sdb".to_string(),
                size: 100 * GIGABYTE, // 100 GiB
                ..Default::default()
            },
        ];

        let mut host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    device: PathBuf::from("/dev/sdx"),
                    partitions: vec![Partition::new("part1", 60 * GIGABYTE)], // 60 GiB
                    id: "disk0".to_string(),
                    partition_table_type: Default::default(),
                    adopted_partitions: Default::default(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        // Original disk size of 60 GiB requires at least sdb (100 GiB)
        update_target_disk_path_with_candidates(
            &mut host_config,
            60 * GIGABYTE,
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
