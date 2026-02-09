use std::{
    collections::{BTreeMap, HashMap},
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use gpt::{DiskDevice, GptConfig, GptDisk};
use log::{debug, trace};
use uuid::Uuid;

use osutils::{
    block_devices::{self, ResolvedDisk},
    udevadm,
};

use crate::engine::EngineContext;

/// This function handles the partitioning logic for raw COSI storage mode. In
/// this mode, we expect the image to be carrying a raw GPT that we can directly
/// replicate onto the disk, so instead of doing any kind of intelligent
/// partition creation based on the Host Configuration, we will just take the
/// GPT from the image and write it to disk as-is.
pub(super) fn create_partitions_for_raw_cosi_storage(
    ctx: &mut EngineContext,
    disk: &ResolvedDisk,
) -> Result<(), Error> {
    let partitioning_info = ctx
        .image
        .as_mut()
        .context("An image is needed for raw partitioning mode")?
        .partitioning_info()
        .context("Failed to get GPT data from image for raw partitioning mode")?
        .context("Image does not provide raw GPT data")?;

    // Before we actually touch the disk, stage the disk and partition
    // information we will add to EngineContext, so that we may catch
    // correspondence issues early. Note: we won't store these into the
    // EngineContext until after we've successfully created the GPT on disk,
    // since that's the point of no return for making changes to the disk.

    // First, the disk DeviceId -> UUID mapping.
    let staged = stage_new_block_devices(disk, partitioning_info.gpt)
        .context("Failed to stage new block devices for raw partitioning mode")?;

    // Do the actual replication.
    replicate_partitioning(
        partitioning_info.lba0,
        partitioning_info.gpt,
        &disk.dev_path,
    )
    .context("Failed to replicate GPT from image to disk in raw partitioning mode")?;

    // Now force the kernel to re-read the partition table, so that the new
    // partitions show up in /dev. This is gated behind a check for whether we
    // actually created any partitions because partx --update will fail if there
    // are no partitions.
    if !staged.partitions.is_empty() {
        block_devices::partx_update(&disk.dev_path)
            .context("Failed to run partx --update after writing GPT in raw partitioning mode")?;
    }

    // After writing the GPT to disk, we need to wait for the new partition
    // devices to appear before we can proceed, since the rest of the Engine
    // logic expects the partition devices to be present.
    for staged_partition_path in staged.partition_paths() {
        trace!(
            "Waiting for partition device at path '{}' to appear",
            staged_partition_path.display()
        );
        udevadm::wait(staged_partition_path).context(format!(
            "Failed waiting for '{}' to appear",
            staged_partition_path.display()
        ))?;

        ensure!(
            staged_partition_path.exists(),
            "Expected partition device at path '{}' does not exist after waiting",
            staged_partition_path.display()
        );
    }

    // If we got here, then the GPT has been successfully written to disk, so we
    // can now commit the staged disk and partition information to the
    // EngineContext.
    debug!(
        "Disk '{}' has been repartitioned successfully from the raw GPT in the image",
        disk.dev_path.display()
    );
    staged.commit_to_context(ctx);

    Ok(())
}

/// Takes in a GptDisk object and a target disk device path, and replicates the
/// GPT from the image onto the target disk. This will overwrite any existing
/// partitions on the target disk.
fn replicate_partitioning(
    lba0: &[u8],
    raw_gpt: &GptDisk<impl DiskDevice>,
    target_device: impl AsRef<Path>,
) -> Result<(), Error> {
    // First, do some basic validation to make sure the lba0 is of the expected
    // size.
    ensure!(
        lba0.len() == raw_gpt.logical_block_size().as_usize(),
        "Invalid protective MBR (LBA 0) data from image for raw partitioning mode: expected {} bytes, got {} bytes",
        raw_gpt.logical_block_size().as_usize(),
        lba0.len()
    );

    trace!(
        "Opening disk device at path '{}' for raw partitioning mode",
        target_device.as_ref().display()
    );
    let mut disk_device = OpenOptions::new()
        .read(true)
        .write(true)
        .open(target_device.as_ref())
        .with_context(|| {
            format!(
                "Failed to open disk device at path '{}' for repartitioning",
                target_device.as_ref().display()
            )
        })?;

    // Paranoid seek to start f the disk.
    disk_device
        .seek(SeekFrom::Start(0))
        .context("Failed to seek to start of disk device")?;

    // Write the protective MBR (LBA 0) from the image to the disk.
    trace!(
        "Writing protective MBR (LBA 0) to disk device at path '{}' for raw partitioning mode",
        target_device.as_ref().display()
    );
    disk_device
        .write_all(lba0)
        .context("Failed to write protective MBR (LBA 0) to disk")?;

    // Return to the start of the disk.
    disk_device
        .seek(SeekFrom::Start(0))
        .context("Failed to seek to start of disk device")?;

    // Create the new GPT on the disk using the raw GPT data from the image.
    // This will overwrite any existing partitions on the disk.
    trace!(
        "Creating new GPT on disk device at path '{}'",
        target_device.as_ref().display()
    );
    let mut new_gpt = GptConfig::new()
        .writable(true)
        .change_partition_count(true)
        .logical_block_size(*raw_gpt.logical_block_size())
        .create_from_device(&mut disk_device, Some(*raw_gpt.guid()))
        .context("Failed to create GPT from disk device in raw partitioning mode")?;

    // Now start replicating partitions!
    new_gpt
        .update_partitions(raw_gpt.partitions().clone())
        .context("Failed to update partitions in raw partitioning mode")?;

    trace!(
        "Writing new GPT to disk device at path '{}'",
        target_device.as_ref().display()
    );
    new_gpt
        .write()
        .context("Failed to write new GPT to disk in raw partitioning mode")?;

    disk_device
        .sync_all()
        .context("Failed to sync disk device after writing GPT in raw partitioning mode")?;

    // NOTE:
    //
    // There is an implicit but very important drop of the file handle to the
    // disk at the end of this function. Closing the file descriptor is
    // important because it seems to re-trigger udev rules, so we need to do it
    // at a controlled point in time.

    Ok(())
}

struct StagedBlockDevices {
    disk: (String, Uuid),
    partitions: HashMap<String, PathBuf>,
}

impl StagedBlockDevices {
    /// Store the staged block device information into the EngineContext. This
    /// should only be called after the GPT has been successfully written to
    /// disk.
    fn commit_to_context(self, ctx: &mut EngineContext) {
        let (staged_disk_id, staged_disk_uuid) = self.disk;
        ctx.disk_uuids.insert(staged_disk_id, staged_disk_uuid);
        ctx.partition_paths.extend(self.partitions);
    }

    /// Returns an iterator over the partition device paths of the staged block devices.
    fn partition_paths(&self) -> impl Iterator<Item = &PathBuf> {
        self.partitions.values()
    }
}

fn stage_new_block_devices<T>(
    disk: &ResolvedDisk,
    raw_gpt: &GptDisk<T>,
) -> Result<StagedBlockDevices, Error> {
    // Generate a mapping from partition UUID to partition ID for the disk in the Host Configuration.
    let device_id_by_part_uuid = disk
        .spec
        .partitions
        .iter()
        .filter_map(|part| part.uuid.as_ref().map(|uuid| (*uuid, &part.id)))
        .collect::<BTreeMap<_, _>>();

    // Before we actually touch the disk, stage the disk and partition
    // information we will add to EngineContext, so that we may catch
    // correspondence issues early. Note: we won't store these into the
    // EngineContext until after we've successfully created the GPT on disk,
    // since that's the point of no return for making changes to the disk.

    // First, the disk DeviceId -> UUID mapping.
    let staged_disk = (disk.id.clone(), *raw_gpt.guid());
    trace!(
        "Staged disk mapping for raw partitioning mode: {:#?}",
        staged_disk
    );

    // Then, the partition DeviceId -> disk by partition UUID mapping.
    let staged_partitions = {
        let mut tmp = HashMap::new();
        for raw_part in raw_gpt.partitions().values() {
            let part_device_id = device_id_by_part_uuid
                .get(&raw_part.part_guid)
                .with_context(|| {
                    format!(
                        "Partition with UUID '{}' from raw GPT does not match any partition UUID in the Host Configuration",
                        raw_part.part_guid
                    )
                })?;

            trace!(
                "Staging partition with DeviceId '{}' for raw partitioning mode, mapped from UUID '{}'",
                part_device_id,
                raw_part.part_guid
            );

            tmp.insert(
                part_device_id.to_owned().to_owned(),
                block_devices::part_uuid_path(raw_part.part_guid),
            );
        }

        tmp
    };

    Ok(StagedBlockDevices {
        disk: staged_disk,
        partitions: staged_partitions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, io::Cursor};

    use gpt::{disk::LogicalBlockSize, mbr::ProtectiveMBR, GptConfig};
    use trident_api::config::{Disk, Partition, PartitionSize, PartitionTableType};

    /// Creates a mock GPT disk in memory with the specified partitions.
    /// Returns a tuple of (lba0, GptDisk, disk_size) where GptDisk contains
    /// the parsed GPT structure. Each partition is defined by (name, size_bytes).
    fn create_mock_gpt_disk(
        partitions: &[(&str, u64)],
    ) -> (Vec<u8>, GptDisk<Cursor<Vec<u8>>>, u64) {
        let disk_size: u64 = 10 * 1024 * 1024; // 10 MB
        let lba_size: u32 = 512;

        let mut disk_buffer = vec![0u8; disk_size as usize];

        // Write protective MBR.
        {
            let mut cursor = Cursor::new(&mut disk_buffer[..]);
            let mbr = ProtectiveMBR::with_lb_size(
                u32::try_from((disk_size / lba_size as u64) - 1).unwrap_or(0xFFFFFFFF),
            );
            mbr.overwrite_lba0(&mut cursor).unwrap();
        }

        // Create and write GPT with partitions.
        {
            let cursor = Cursor::new(&mut disk_buffer[..]);
            let mut gpt_disk = GptConfig::new()
                .writable(true)
                .logical_block_size(LogicalBlockSize::Lb512)
                .create_from_device(cursor, None)
                .expect("Failed to create GPT disk");

            for (name, size) in partitions {
                gpt_disk
                    .add_partition(name, *size, gpt::partition_types::LINUX_FS, 0, None)
                    .expect("Failed to add partition");
            }

            gpt_disk.write().expect("Failed to write GPT");
        }

        // Re-open the GPT for reading.
        let cursor = Cursor::new(disk_buffer);
        let gpt_disk = GptConfig::new()
            .writable(false)
            .logical_block_size(LogicalBlockSize::Lb512)
            .open_from_device(cursor)
            .expect("Failed to open GPT disk");

        let mbr = ProtectiveMBR::with_lb_size(
            u32::try_from((disk_size / lba_size as u64) - 1).unwrap_or(0xFF_FF_FF_FF),
        );
        let mut lba0 = vec![0u8; lba_size as usize];
        mbr.overwrite_lba0(&mut Cursor::new(&mut lba0)).unwrap();

        (lba0, gpt_disk, disk_size)
    }

    /// Creates an empty disk file suitable as a target for GPT replication.
    fn create_empty_disk_file(disk_size: u64) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().expect("Failed to create temp file");

        // Write zeroed disk content.
        let disk_buffer = vec![0u8; disk_size as usize];
        std::fs::write(file.path(), &disk_buffer).expect("Failed to write empty disk");

        // Write protective MBR so GptConfig can create from device.
        let mut f = OpenOptions::new()
            .read(true)
            .write(true)
            .open(file.path())
            .expect("Failed to open temp file");
        let mbr =
            ProtectiveMBR::with_lb_size(u32::try_from((disk_size / 512) - 1).unwrap_or(0xFFFFFFFF));
        mbr.overwrite_lba0(&mut f).unwrap();

        file
    }

    /// Verifies that `replicate_partitioning` faithfully copies a multi-partition
    /// GPT layout (disk GUID, partition GUIDs, type GUIDs, LBA ranges, names)
    /// and the protective MBR from a source disk onto an empty target file.
    #[test]
    fn test_replicate_gpt_copies_partitions_and_guid() {
        let (lba0, source_gpt, disk_size) =
            create_mock_gpt_disk(&[("esp", 64 * 1024), ("root", 128 * 1024)]);

        let source_guid = *source_gpt.guid();
        let source_partitions = source_gpt.partitions().clone();

        let target_file = create_empty_disk_file(disk_size);

        replicate_partitioning(&lba0, &source_gpt, target_file.path())
            .expect("replicate_gpt failed");

        // Re-open the target and verify the GPT was replicated.
        let target_gpt = GptConfig::new()
            .writable(false)
            .logical_block_size(LogicalBlockSize::Lb512)
            .open(target_file.path())
            .expect("Failed to open replicated GPT");

        // Verify the disk GUID matches.
        assert_eq!(
            *target_gpt.guid(),
            source_guid,
            "Disk GUID should match after replication"
        );

        // Verify partition count matches.
        let target_partitions = target_gpt.partitions();
        assert_eq!(
            target_partitions.len(),
            source_partitions.len(),
            "Partition count should match after replication"
        );

        // Verify each partition's attributes match.
        for (id, source_part) in &source_partitions {
            let target_part = target_partitions
                .get(id)
                .unwrap_or_else(|| panic!("Partition {} not found in replicated GPT", id));

            assert_eq!(
                target_part.part_guid, source_part.part_guid,
                "Partition GUID should match for partition {}",
                id
            );
            assert_eq!(
                target_part.part_type_guid, source_part.part_type_guid,
                "Partition type GUID should match for partition {}",
                id
            );
            assert_eq!(
                target_part.first_lba, source_part.first_lba,
                "First LBA should match for partition {}",
                id
            );
            assert_eq!(
                target_part.last_lba, source_part.last_lba,
                "Last LBA should match for partition {}",
                id
            );
            assert_eq!(
                target_part.name, source_part.name,
                "Partition name should match for partition {}",
                id
            );
        }

        let file_data = fs::read(target_file.path()).expect("Failed to read target disk file");
        assert_eq!(
            &file_data[0..lba0.len()],
            lba0,
            "LBA 0 data should match the source protective MBR"
        );
    }

    /// Verifies that `replicate_partitioning` works correctly with a single
    /// partition, ensuring the disk GUID, partition count, and protective MBR
    /// are all replicated.
    #[test]
    fn test_replicate_gpt_single_partition() {
        let (lba0, source_gpt, disk_size) = create_mock_gpt_disk(&[("data", 256 * 1024)]);

        let target_file = create_empty_disk_file(disk_size);

        replicate_partitioning(&lba0, &source_gpt, target_file.path())
            .expect("replicate_gpt failed");
        let target_gpt = GptConfig::new()
            .writable(false)
            .logical_block_size(LogicalBlockSize::Lb512)
            .open(target_file.path())
            .expect("Failed to open replicated GPT");

        assert_eq!(*target_gpt.guid(), *source_gpt.guid());
        assert_eq!(target_gpt.partitions().len(), 1);

        let file_data = fs::read(target_file.path()).expect("Failed to read target disk file");
        assert_eq!(
            &file_data[0..lba0.len()],
            lba0,
            "LBA 0 data should match the source protective MBR"
        );
    }

    /// Verifies that `replicate_partitioning` returns an error when the target
    /// device path does not exist, rather than panicking.
    #[test]
    fn test_replicate_gpt_fails_on_nonexistent_target() {
        let (lba0, source_gpt, _) = create_mock_gpt_disk(&[("test", 64 * 1024)]);

        let result = replicate_partitioning(&lba0, &source_gpt, "/nonexistent/path/to/disk");
        assert!(
            result.is_err(),
            "replicate_gpt should fail when target device does not exist"
        );
    }

    /// Creates a `ResolvedDisk` whose partition spec UUIDs match the partitions
    /// in the given `GptDisk`. Each partition gets a `BlockDeviceId` of the form
    /// `"partition-N"`.
    fn create_resolved_disk_matching_gpt<T>(gpt: &GptDisk<T>) -> ResolvedDisk {
        let partitions: Vec<Partition> = gpt
            .partitions()
            .iter()
            .enumerate()
            .map(|(i, (_id, part))| {
                let mut p = Partition::new(
                    format!("partition-{}", i),
                    PartitionSize::Fixed(4096.into()),
                );
                p.uuid = Some(part.part_guid);
                p
            })
            .collect();

        ResolvedDisk {
            id: "disk-0".to_string(),
            spec: Disk {
                id: "disk-0".to_string(),
                device: PathBuf::from("/dev/sda"),
                partition_table_type: PartitionTableType::Gpt,
                partitions,
                adopted_partitions: vec![],
            },
            dev_path: PathBuf::from("/dev/sda"),
        }
    }

    /// Verifies that `stage_new_block_devices` correctly maps a multi-partition
    /// GPT to `StagedBlockDevices`, producing the right disk UUID and one
    /// `/dev/disk/by-partuuid/` path entry per partition, and that
    /// `commit_to_context` transfers the staged information into an
    /// `EngineContext`.
    #[test]
    fn test_stage_new_block_devices_maps_partitions() {
        let (_lba0, gpt, _) = create_mock_gpt_disk(&[("esp", 64 * 1024), ("root", 128 * 1024)]);
        let disk = create_resolved_disk_matching_gpt(&gpt);

        let staged = stage_new_block_devices(&disk, &gpt).expect("stage_new_block_devices failed");

        // Disk UUID should match the GPT's GUID.
        assert_eq!(staged.disk.0, "disk-0");
        assert_eq!(staged.disk.1, *gpt.guid());

        // Should have one staged partition per GPT partition.
        assert_eq!(staged.partitions.len(), gpt.partitions().len());

        // Each staged partition path should be under /dev/disk/by-partuuid/
        // and match the corresponding GPT partition UUID.
        for gpt_part in gpt.partitions().values() {
            let expected_path = block_devices::part_uuid_path(gpt_part.part_guid);
            assert!(
                staged.partition_paths().any(|p| *p == expected_path),
                "Expected staged partition path '{}' not found",
                expected_path.display()
            );
        }

        // Collect expected values before committing (which consumes staged).
        let expected_partition_paths: BTreeMap<_, _> = staged
            .partitions
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        // Verify commit_to_context transfers everything into the EngineContext.
        let mut ctx = EngineContext::default();
        assert!(ctx.disk_uuids.is_empty());
        assert!(ctx.partition_paths.is_empty());

        staged.commit_to_context(&mut ctx);

        // Disk UUID should be present in the context.
        assert_eq!(
            ctx.disk_uuids.get("disk-0"),
            Some(gpt.guid()),
            "Disk UUID should be committed to EngineContext"
        );

        // All partition paths should be present in the context.
        assert_eq!(
            ctx.partition_paths.len(),
            expected_partition_paths.len(),
            "All partition paths should be committed to EngineContext"
        );
        for (id, expected_path) in &expected_partition_paths {
            assert_eq!(
                ctx.partition_paths.get(id),
                Some(expected_path),
                "Partition path for '{}' should be committed to EngineContext",
                id
            );
        }
    }

    /// Verifies that `stage_new_block_devices` works correctly with a single
    /// partition, producing exactly one partition mapping.
    #[test]
    fn test_stage_new_block_devices_single_partition() {
        let (_lba0, gpt, _) = create_mock_gpt_disk(&[("data", 256 * 1024)]);
        let disk = create_resolved_disk_matching_gpt(&gpt);

        let staged = stage_new_block_devices(&disk, &gpt).expect("stage_new_block_devices failed");

        assert_eq!(staged.partitions.len(), 1);

        let gpt_part = gpt.partitions().values().next().unwrap();
        let expected_path = block_devices::part_uuid_path(gpt_part.part_guid);
        assert!(staged.partitions.values().any(|p| *p == expected_path));
    }

    /// Verifies that `stage_new_block_devices` returns an error when the GPT
    /// contains a partition whose UUID does not appear in the disk spec, since
    /// there is no `BlockDeviceId` to map it to.
    #[test]
    fn test_stage_new_block_devices_fails_on_uuid_mismatch() {
        let (_lba0, gpt, _) = create_mock_gpt_disk(&[("root", 128 * 1024)]);

        // Build a ResolvedDisk whose partition UUIDs do NOT match the GPT.
        let disk = ResolvedDisk {
            id: "disk-0".to_string(),
            spec: Disk {
                id: "disk-0".to_string(),
                device: PathBuf::from("/dev/sda"),
                partition_table_type: PartitionTableType::Gpt,
                partitions: vec![{
                    let mut p = Partition::new("partition-0", PartitionSize::Fixed(4096.into()));
                    // Use a random UUID that won't match any GPT partition.
                    p.uuid = Some(Uuid::new_v4());
                    p
                }],
                adopted_partitions: vec![],
            },
            dev_path: PathBuf::from("/dev/sda"),
        };

        let result = stage_new_block_devices(&disk, &gpt);
        assert!(
            result.is_err(),
            "stage_new_block_devices should fail when GPT partition UUIDs don't match the disk spec"
        );
    }

    /// Verifies that `stage_new_block_devices` returns an error when the disk
    /// spec has no partitions at all, leaving no UUIDs to match against.
    #[test]
    fn test_stage_new_block_devices_fails_on_empty_disk_spec() {
        let (_lba0, gpt, _) = create_mock_gpt_disk(&[("root", 128 * 1024)]);

        let disk = ResolvedDisk {
            id: "disk-0".to_string(),
            spec: Disk {
                id: "disk-0".to_string(),
                device: PathBuf::from("/dev/sda"),
                partition_table_type: PartitionTableType::Gpt,
                partitions: vec![],
                adopted_partitions: vec![],
            },
            dev_path: PathBuf::from("/dev/sda"),
        };

        let result = stage_new_block_devices(&disk, &gpt);
        assert!(
            result.is_err(),
            "stage_new_block_devices should fail when disk spec has no partitions"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use crate::osimage::{
        mock::{MockOsImage, MockPartitioningInfo},
        OsImage,
    };

    use super::*;

    use osutils::{block_devices::ResolvedDisk, testutils::repart::TEST_DISK_DEVICE_PATH, wipefs};
    use pytest_gen::functional_test;
    use trident_api::config::{Disk, Partition, PartitionSize, PartitionTableType};

    #[functional_test]
    fn test_create_partitions_for_raw_cosi_storage() {
        let mut part_info =
            MockPartitioningInfo::new_protective_mbr_and_gpt().expect("mock partitioning info");

        part_info
            .gpt
            .add_partition("esp", 64 * 1024, gpt::partition_types::EFI, 0, None)
            .unwrap();
        part_info
            .gpt
            .add_partition("root", 128 * 1024, gpt::partition_types::LINUX_FS, 0, None)
            .unwrap();

        let resolved_disk = ResolvedDisk {
            id: "disk-0".to_string(),
            spec: Disk {
                id: "disk-0".to_string(),
                device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                partition_table_type: PartitionTableType::Gpt,
                partitions: part_info
                    .gpt
                    .partitions()
                    .values()
                    .enumerate()
                    .map(|(i, part)| {
                        // Create partitions with matching UUIDs in the HC.
                        let mut p = Partition::new(
                            format!("partition-{}", i),
                            PartitionSize::Fixed(4096.into()),
                        );
                        p.uuid = Some(part.part_guid);
                        p
                    })
                    .collect(),
                adopted_partitions: vec![],
            },
            dev_path: PathBuf::from(TEST_DISK_DEVICE_PATH),
        };

        let mut ctx = EngineContext {
            image: Some(OsImage::mock(
                MockOsImage::new().with_partitioning_info(part_info.clone()),
            )),
            ..Default::default()
        };

        create_partitions_for_raw_cosi_storage(&mut ctx, &resolved_disk)
            .expect("Failed to replicate GPT for raw COSI storage");

        assert_eq!(
            ctx.disk_uuids.get(&resolved_disk.id),
            Some(part_info.gpt.guid())
        );
        assert_eq!(ctx.partition_paths.len(), part_info.gpt.partitions().len());

        for id in resolved_disk.spec.partitions.iter().map(|p| &p.id) {
            let path = ctx
                .partition_paths
                .get(id)
                .unwrap_or_else(|| panic!("Missing partition path for '{id}'"));
            assert!(
                path.exists(),
                "Partition device path '{}' does not exist",
                path.display()
            );
        }

        wipefs::all(TEST_DISK_DEVICE_PATH).expect("Failed to wipe test disk");
    }
}
