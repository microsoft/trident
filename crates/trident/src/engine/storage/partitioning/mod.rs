use anyhow::{ensure, Context, Error};

use osutils::block_devices;
use trident_api::constants::internal_params::RAW_COSI_STORAGE;

use crate::engine::EngineContext;

mod adoption;
mod raw_mode;
pub mod repart_mode;
mod safety_check;

/// Given a Host Configuration, adopt and create partitions on the disks.
#[tracing::instrument(name = "partitions_creation", skip_all)]
pub fn create_partitions(ctx: &mut EngineContext) -> Result<(), Error> {
    // Resolve the disk paths to ensure that all disks in the configuration exist.
    let resolved_disks =
        block_devices::get_resolved_disks(&ctx.spec).context("Failed to resolve disk paths")?;

    // Do a non-destructive first pass of adoption to detect any issues before
    // we start making changes.
    safety_check::partitioning_safety_check(&resolved_disks)
        .context("Partitioning safety check failed")?;

    if !ctx.spec.internal_params.get_flag(RAW_COSI_STORAGE) {
        // Regular Host Configuration flow.
        for disk in &resolved_disks {
            repart_mode::create_partitions_on_disk(
                disk,
                &mut ctx.partition_paths,
                &mut ctx.disk_uuids,
            )
            .with_context(|| format!("Failed to create partitions for disk '{}'", disk.id))?;
        }
    } else {
        // In raw COSI storage flow.
        ensure!(
            resolved_disks.len() == 1,
            "Expected exactly one disk in raw COSI storage mode, found {}",
            resolved_disks.len()
        );

        raw_mode::create_partitions_for_raw_cosi_storage(ctx, &resolved_disks[0])
            .context("Failed to create partitions for raw COSI storage")?;
    }

    Ok(())
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::{path::PathBuf, str::FromStr};

    use osutils::{testutils::repart::TEST_DISK_DEVICE_PATH, wipefs};
    use pytest_gen::functional_test;
    use trident_api::config::{
        Disk, HostConfiguration, Partition, PartitionSize, PartitionTableType, PartitionType,
        Storage,
    };

    #[functional_test]
    fn test_create_partitions() {
        let host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk".to_string(),
                    device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                    partitions: vec![
                        Partition {
                            id: "part1".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1M").unwrap(),
                            uuid: None,
                            label: None,
                        },
                        Partition {
                            id: "part2".to_string(),
                            partition_type: PartitionType::Swap,
                            size: PartitionSize::from_str("2M").unwrap(),
                            uuid: None,
                            label: None,
                        },
                        Partition {
                            id: "part3".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::Grow,
                            uuid: None,
                            label: None,
                        },
                    ],
                    partition_table_type: PartitionTableType::Gpt,
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let mut ctx = EngineContext {
            spec: host_config.clone(),
            ..Default::default()
        };

        create_partitions(&mut ctx).unwrap();

        assert_eq!(ctx.partition_paths.len(), 3);

        let check_part = |name: &str| {
            ctx.partition_paths
                .get(name)
                .unwrap_or_else(|| panic!("Failed to find block device '{name}' in status"));
        };

        check_part("part1");
        check_part("part2");
        check_part("part3");

        wipefs::all(TEST_DISK_DEVICE_PATH).unwrap();
    }
}
