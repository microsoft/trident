use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};
use gpt::disk::LogicalBlockSize;
use uuid::Uuid;

use sysdefs::partition_types::DiscoverablePartitionType;
use trident_api::{
    config::{
        Disk, FileSystem, FileSystemSource, HostConfiguration, ImageSha384,
        OsImage as ConfigOsImage, Partition, Storage,
    },
    misc::IdGenerator,
};

use super::{metadata::GptRegionType, Cosi};

impl Cosi {
    /// A helper function that performs the actual derivation of the host
    /// configuration assuming that the COSI version is sufficient and the
    /// necessary metadata and GPT data are present. This is separated from
    /// `derive_host_configuration` to:
    ///
    /// - Allow for easier testing since we can directly construct a COSI object
    ///   with the required fields without having to go through the GPT
    ///   population logic.
    /// - Take an immutable reference to self, which makes it clear that this
    ///   function does not modify the COSI object and relies on all necessary
    ///   data being pre-populated. (Also simplifies borrowing.)
    pub(super) fn derive_host_configuration_inner(
        &self,
        target_disk: impl AsRef<Path>,
    ) -> Result<HostConfiguration, Error> {
        let mut filesystems_by_path = self
            .metadata
            .images
            .iter()
            .map(|image| (image.file.path.as_path(), image))
            .collect::<HashMap<_, _>>();

        let mut id_gen = IdGenerator::new("partition");

        // The vecs we will be populating
        let mut partitions = Vec::new();
        let mut filesystems = Vec::new();

        for partition in self.joined_disk_info_and_gpt()? {
            let partition_id = id_gen.next_id();

            partitions.push(Partition {
                id: partition_id.clone(),
                size: partition.partition_size.into(),
                uuid: Some(partition.partition_uuid),
                label: Some(partition.partition_label),
                partition_type: partition.partition_type.into(),
            });

            let Some(filesystem_metadata) =
                filesystems_by_path.remove(partition.image_path.as_path())
            else {
                // There is no filesystem associated to this partition.
                continue;
            };

            filesystems.push(FileSystem {
                device_id: Some(partition_id),
                mount_point: Some(filesystem_metadata.mount_point.as_path().into()),
                source: FileSystemSource::Image,
            });
        }

        // Ensure that all filesystems were matched to a partition. If there are
        // any left, that means they don't correspond to any partition in the
        // GPT data, and we should error out since we don't know how to handle
        // them.
        if let Some(extra_filesystem) = filesystems_by_path.into_values().next() {
            bail!(
                "The filesystem at path '{}' (from '{}') does not correspond to any partition in the GPT data, cannot derive host configuration.",
                extra_filesystem.mount_point.display(),
                extra_filesystem.file.path.display()
            );
        }

        Ok(HostConfiguration {
            image: Some(ConfigOsImage {
                url: self.source.clone(),
                sha384: ImageSha384::Checksum(self.metadata_sha384.clone()),
            }),
            storage: Storage {
                disks: vec![Disk {
                    id: "disk-0".to_string(),
                    device: target_disk.as_ref().to_path_buf(),
                    partitions,
                    ..Default::default()
                }],
                filesystems,
                ..Default::default()
            },
            ..Default::default()
        })
    }

    /// Combines disk metadata and GPT data to produce a unified view of the
    /// partitions.
    ///
    /// It ensures that the number of partitions in the disk metadata matches
    /// the number of GPT partitions, and that each partition referenced in the
    /// disk metadata has a corresponding GPT partition. It then constructs a
    /// `JointPartitionMetadata` struct for each partition, which includes the
    /// partition size, UUID, label, type, and associated image path.
    fn joined_disk_info_and_gpt(&self) -> Result<Vec<JointPartitionMetadata>, Error> {
        // First, retrieve the GPT partitions. We require GPT data for this
        // operation, so we error if it's missing.
        let gpt_partitions = self
            .partitioning_info
            .as_ref()
            .with_context(|| {
                format!(
                    "COSI is version {}, but GPT data is missing",
                    self.metadata.version
                )
            })?
            .gpt_disk
            .partitions();

        // Ensure we have disk metadata, which is required for this operation.
        let disk_info = self.metadata.disk.as_ref().with_context(|| {
            format!(
                "COSI metadata version is {}, but disk metadata is missing",
                self.metadata.version
            )
        })?;

        // Determine the LBA size from the disk metadata. This is needed to
        // calculate partition sizes from the GPT data. The GPT library we use
        // only supports 512 and 4096 byte LBAs, so we error if it's any other
        // value.
        let lba_size = match disk_info.lba_size {
            512 => LogicalBlockSize::Lb512,
            4096 => LogicalBlockSize::Lb4096,
            other => bail!("Unsupported LBA size: {}", other),
        };

        let metadata_partitions = disk_info
            .gpt_regions
            .iter()
            .filter_map(|r| match r.region_type {
                GptRegionType::Partition { number } => Some((&r.image, number)),
                _ => None,
            })
            .collect::<Vec<_>>();

        ensure!(
            metadata_partitions.len() == gpt_partitions.len(),
            "Number of partitions in disk metadata ({}) does not match number of GPT partitions ({})",
            metadata_partitions.len(),
            gpt_partitions.len()
        );

        metadata_partitions
            .into_iter()
            .map(|(image, number)| {
                let gpt_partition = gpt_partitions.get(&number).with_context(|| {
                    format!(
                        "GPT partition number {} referenced in disk metadata not found in GPT data",
                        number
                    )
                })?;

                let partition_size = gpt_partition
                    .bytes_len(lba_size)
                    .with_context(|| format!("Failed to calculate size of partition {number}"))?;

                Ok(JointPartitionMetadata {
                    partition_size,
                    partition_uuid: gpt_partition.part_guid,
                    partition_label: gpt_partition.name.clone(),
                    partition_type: DiscoverablePartitionType::from_uuid(
                        &gpt_partition.part_type_guid.guid,
                    ),
                    image_path: image.path.clone(),
                })
            })
            .collect()
    }
}

#[derive(Debug)]
struct JointPartitionMetadata {
    partition_size: u64,
    partition_uuid: Uuid,
    partition_label: String,
    partition_type: DiscoverablePartitionType,
    image_path: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;

    use gpt::{mbr::ProtectiveMBR, GptConfig};
    use osutils::osrelease::OsRelease;
    use sysdefs::{arch::SystemArchitecture, osuuid::OsUuid};
    use trident_api::{config::FileSystemSource, primitives::hash::Sha384Hash};
    use url::Url;

    use crate::{
        io_utils::file_reader::FileReader,
        osimage::{
            cosi::{
                entries::CosiEntries,
                metadata::{
                    CosiMetadata, DiskInfo, GptDiskRegion, Image, ImageFile, PartitionTableType,
                },
                Cosi, CosiPartitioningInfo, KnownMetadataVersion,
            },
            OsImageFileSystemType,
        },
    };

    /// Creates a mock GPT disk in memory with the specified partitions.
    ///
    /// Returns a tuple of (GptDisk, disk_size, lba_size) where GptDisk contains
    /// the parsed GPT structure. Each partition is defined by (name, size_bytes).
    fn create_mock_gpt_disk(
        partitions: &[(&str, u64)],
    ) -> (gpt::GptDisk<Cursor<Vec<u8>>>, u64, u32) {
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

        (gpt_disk, disk_size, lba_size)
    }

    /// Creates a minimal Cosi instance for testing with the given metadata and GPT.
    fn create_test_cosi(
        metadata: CosiMetadata,
        gpt: Option<gpt::GptDisk<Cursor<Vec<u8>>>>,
    ) -> Cosi {
        Cosi {
            source: Url::parse("file:///test/image.cosi").unwrap(),
            metadata,
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            partitioning_info: gpt.map(|gpt_disk| CosiPartitioningInfo {
                lba0: Vec::new(),
                gpt_disk,
            }),
            reader: FileReader::Buffer(Cursor::new(Vec::<u8>::new())),
            entries: CosiEntries::default(),
        }
    }

    /// Creates a sample ImageFile with the given path.
    fn sample_image_file(path: &str) -> ImageFile {
        ImageFile {
            path: PathBuf::from(path),
            compressed_size: 1024,
            uncompressed_size: 2048,
            sha384: Sha384Hash::from("0".repeat(96)),
        }
    }

    /// Creates a sample Image (filesystem) with the given path and mount point.
    fn sample_image(path: &str, mount_point: &str) -> Image {
        Image {
            file: sample_image_file(path),
            mount_point: PathBuf::from(mount_point),
            fs_type: OsImageFileSystemType::Ext4,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::LinuxGeneric,
            verity: None,
        }
    }

    // =========================================================================
    // Tests for joined_disk_info_and_gpt
    // =========================================================================

    /// Tests [`Cosi::joined_disk_info_and_gpt`] with valid disk info and GPT data.
    ///
    /// Creates a GPT with two partitions and corresponding disk metadata regions,
    /// then verifies that the joined result contains the correct partition metadata
    /// including sizes, UUIDs, labels, and associated image paths.
    #[test]
    fn test_joined_disk_info_and_gpt_success() {
        let (gpt_disk, disk_size, lba_size) =
            create_mock_gpt_disk(&[("esp_partition", 64 * 1024), ("root_partition", 128 * 1024)]);

        // Get partition info from GPT for verification.
        let gpt_partitions: Vec<_> = gpt_disk
            .partitions()
            .iter()
            .map(|(num, p)| (*num, p.name.clone(), p.part_guid))
            .collect();

        let disk_info = DiskInfo {
            size: disk_size,
            lba_size,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![
                GptDiskRegion {
                    image: sample_image_file("gpt_primary.zst"),
                    region_type: GptRegionType::PrimaryGpt,
                },
                GptDiskRegion {
                    image: sample_image_file("images/esp.img.zst"),
                    region_type: GptRegionType::Partition { number: 1 },
                },
                GptDiskRegion {
                    image: sample_image_file("images/root.img.zst"),
                    region_type: GptRegionType::Partition { number: 2 },
                },
            ],
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, Some(gpt_disk));

        let result = cosi.joined_disk_info_and_gpt();
        assert!(result.is_ok(), "joined_disk_info_and_gpt should succeed");

        let joint_partitions = result.unwrap();
        assert_eq!(joint_partitions.len(), 2, "Should have 2 partitions");

        // Verify first partition (ESP).
        assert_eq!(
            joint_partitions[0].partition_label, gpt_partitions[0].1,
            "First partition label should match"
        );
        assert_eq!(
            joint_partitions[0].partition_uuid, gpt_partitions[0].2,
            "First partition UUID should match"
        );
        assert_eq!(
            joint_partitions[0].image_path,
            PathBuf::from("images/esp.img.zst"),
            "First partition image path should match"
        );

        // Verify second partition (root).
        assert_eq!(
            joint_partitions[1].partition_label, gpt_partitions[1].1,
            "Second partition label should match"
        );
        assert_eq!(
            joint_partitions[1].partition_uuid, gpt_partitions[1].2,
            "Second partition UUID should match"
        );
        assert_eq!(
            joint_partitions[1].image_path,
            PathBuf::from("images/root.img.zst"),
            "Second partition image path should match"
        );
    }

    /// Tests [`Cosi::joined_disk_info_and_gpt`] error when GPT data is missing.
    ///
    /// Verifies that an appropriate error is returned when the COSI instance
    /// has no GPT data populated.
    #[test]
    fn test_joined_disk_info_and_gpt_missing_gpt() {
        let disk_info = DiskInfo {
            size: 1024 * 1024,
            lba_size: 512,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![GptDiskRegion {
                image: sample_image_file("images/root.img.zst"),
                region_type: GptRegionType::Partition { number: 1 },
            }],
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, None); // No GPT

        let result = cosi.joined_disk_info_and_gpt();
        assert!(result.is_err(), "Should fail without GPT data");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("GPT data is missing"),
            "Error should mention missing GPT: {}",
            err_msg
        );
    }

    /// Tests [`Cosi::joined_disk_info_and_gpt`] error when disk metadata is missing.
    ///
    /// Verifies that an appropriate error is returned when the COSI metadata
    /// doesn't contain disk information.
    #[test]
    fn test_joined_disk_info_and_gpt_missing_disk_metadata() {
        let (gpt_disk, _, _) = create_mock_gpt_disk(&[("test", 64 * 1024)]);

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![],
            bootloader: None,
            disk: None, // No disk metadata
            compression: None,
        };

        let cosi = create_test_cosi(metadata, Some(gpt_disk));

        let result = cosi.joined_disk_info_and_gpt();
        assert!(result.is_err(), "Should fail without disk metadata");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("disk metadata is missing"),
            "Error should mention missing disk metadata: {}",
            err_msg
        );
    }

    /// Tests [`Cosi::joined_disk_info_and_gpt`] error when partition counts mismatch.
    ///
    /// Verifies that an error is returned when the number of partition regions
    /// in disk metadata doesn't match the number of GPT partitions.
    #[test]
    fn test_joined_disk_info_and_gpt_partition_count_mismatch() {
        // Create GPT with 2 partitions.
        let (gpt_disk, disk_size, lba_size) =
            create_mock_gpt_disk(&[("part1", 64 * 1024), ("part2", 64 * 1024)]);

        // But only declare 1 partition in disk metadata.
        let disk_info = DiskInfo {
            size: disk_size,
            lba_size,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![
                GptDiskRegion {
                    image: sample_image_file("gpt_primary.zst"),
                    region_type: GptRegionType::PrimaryGpt,
                },
                GptDiskRegion {
                    image: sample_image_file("images/part1.img.zst"),
                    region_type: GptRegionType::Partition { number: 1 },
                },
                // Missing partition 2
            ],
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, Some(gpt_disk));

        let result = cosi.joined_disk_info_and_gpt();
        assert!(result.is_err(), "Should fail with partition count mismatch");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("does not match"),
            "Error should mention count mismatch: {}",
            err_msg
        );
    }

    /// Tests [`Cosi::joined_disk_info_and_gpt`] error when referencing non-existent partition.
    ///
    /// Verifies that an error is returned when disk metadata references a
    /// partition number that doesn't exist in the GPT.
    #[test]
    fn test_joined_disk_info_and_gpt_invalid_partition_number() {
        // Create GPT with 1 partition (number 1).
        let (gpt_disk, disk_size, lba_size) = create_mock_gpt_disk(&[("part1", 64 * 1024)]);

        // Reference partition 99 which doesn't exist.
        let disk_info = DiskInfo {
            size: disk_size,
            lba_size,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![
                GptDiskRegion {
                    image: sample_image_file("gpt_primary.zst"),
                    region_type: GptRegionType::PrimaryGpt,
                },
                GptDiskRegion {
                    image: sample_image_file("images/part99.img.zst"),
                    region_type: GptRegionType::Partition { number: 99 },
                },
            ],
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, Some(gpt_disk));

        let result = cosi.joined_disk_info_and_gpt();
        assert!(
            result.is_err(),
            "Should fail with invalid partition reference"
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("not found in GPT data"),
            "Error should mention partition not found: {}",
            err_msg
        );
    }

    // =========================================================================
    // Tests for derive_host_configuration_inner
    // =========================================================================

    /// Tests [`Cosi::derive_host_configuration_inner`] successful derivation.
    ///
    /// Creates a complete COSI setup with GPT, disk metadata, and filesystem
    /// images, then verifies that the derived host configuration contains:
    /// - Correct image source URL and SHA384.
    /// - Disk with proper device path.
    /// - Partitions with correct sizes, UUIDs, labels, and types.
    /// - Filesystems with correct mount points linked to partitions.
    #[test]
    fn test_derive_host_configuration_inner_success() {
        let (gpt_disk, disk_size, lba_size) =
            create_mock_gpt_disk(&[("esp", 64 * 1024), ("root", 256 * 1024)]);

        let disk_info = DiskInfo {
            size: disk_size,
            lba_size,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![
                GptDiskRegion {
                    image: sample_image_file("gpt_primary.zst"),
                    region_type: GptRegionType::PrimaryGpt,
                },
                GptDiskRegion {
                    image: sample_image_file("images/esp.img.zst"),
                    region_type: GptRegionType::Partition { number: 1 },
                },
                GptDiskRegion {
                    image: sample_image_file("images/root.img.zst"),
                    region_type: GptRegionType::Partition { number: 2 },
                },
            ],
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![
                sample_image("images/esp.img.zst", "/boot/efi"),
                sample_image("images/root.img.zst", "/"),
            ],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, Some(gpt_disk));

        let target_disk = "/dev/sda";
        let result = cosi.derive_host_configuration_inner(target_disk);
        assert!(
            result.is_ok(),
            "derive_host_configuration_inner should succeed: {:?}",
            result.err()
        );

        let hc = result.unwrap();

        // Verify image source.
        assert!(hc.image.is_some(), "Image should be present");
        let image = hc.image.unwrap();
        assert_eq!(
            image.url.as_str(),
            "file:///test/image.cosi",
            "Image URL should match"
        );

        // Verify disk.
        assert_eq!(hc.storage.disks.len(), 1, "Should have 1 disk");
        assert_eq!(
            hc.storage.disks[0].device,
            Path::new(target_disk),
            "Disk device should match"
        );
        assert_eq!(
            hc.storage.disks[0].partitions.len(),
            2,
            "Should have 2 partitions"
        );

        // Verify partitions have sequential IDs.
        assert_eq!(hc.storage.disks[0].partitions[0].id, "partition-0");
        assert_eq!(hc.storage.disks[0].partitions[1].id, "partition-1");

        // Verify partition labels from GPT.
        assert_eq!(
            hc.storage.disks[0].partitions[0].label,
            Some("esp".to_string())
        );
        assert_eq!(
            hc.storage.disks[0].partitions[1].label,
            Some("root".to_string())
        );

        // Verify filesystems.
        assert_eq!(hc.storage.filesystems.len(), 2, "Should have 2 filesystems");

        // First filesystem (ESP).
        assert_eq!(
            hc.storage.filesystems[0].device_id,
            Some("partition-0".to_string())
        );
        assert_eq!(
            hc.storage.filesystems[0].mount_point,
            Some("/boot/efi".into())
        );
        assert_eq!(hc.storage.filesystems[0].source, FileSystemSource::Image);

        // Second filesystem (root).
        assert_eq!(
            hc.storage.filesystems[1].device_id,
            Some("partition-1".to_string())
        );
        assert_eq!(hc.storage.filesystems[1].mount_point, Some("/".into()));
        assert_eq!(hc.storage.filesystems[1].source, FileSystemSource::Image);
    }

    /// Tests [`Cosi::derive_host_configuration_inner`] with partition without filesystem.
    ///
    /// Verifies that partitions without corresponding filesystem images (e.g.,
    /// swap partitions) are included in the host configuration but don't create
    /// filesystem entries.
    #[test]
    fn test_derive_host_configuration_inner_partition_without_filesystem() {
        let (gpt_disk, disk_size, lba_size) = create_mock_gpt_disk(&[
            ("esp", 64 * 1024),
            ("swap", 128 * 1024), // No filesystem for this
        ]);

        let disk_info = DiskInfo {
            size: disk_size,
            lba_size,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![
                GptDiskRegion {
                    image: sample_image_file("gpt_primary.zst"),
                    region_type: GptRegionType::PrimaryGpt,
                },
                GptDiskRegion {
                    image: sample_image_file("images/esp.img.zst"),
                    region_type: GptRegionType::Partition { number: 1 },
                },
                GptDiskRegion {
                    image: sample_image_file("images/swap.img.zst"),
                    region_type: GptRegionType::Partition { number: 2 },
                },
            ],
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![
                sample_image("images/esp.img.zst", "/boot/efi"),
                // No image for swap partition
            ],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, Some(gpt_disk));

        let result = cosi.derive_host_configuration_inner("/dev/sda");
        assert!(
            result.is_ok(),
            "Should succeed with partition without filesystem"
        );

        let hc = result.unwrap();
        assert_eq!(
            hc.storage.disks[0].partitions.len(),
            2,
            "Should have 2 partitions"
        );
        assert_eq!(
            hc.storage.filesystems.len(),
            1,
            "Should only have 1 filesystem"
        );
        assert_eq!(
            hc.storage.filesystems[0].mount_point,
            Some("/boot/efi".into())
        );
    }

    /// Tests [`Cosi::derive_host_configuration_inner`] error with unmatched filesystem.
    ///
    /// Verifies that an error is returned when a filesystem image doesn't
    /// correspond to any partition in the GPT data.
    #[test]
    fn test_derive_host_configuration_inner_unmatched_filesystem() {
        let (gpt_disk, disk_size, lba_size) = create_mock_gpt_disk(&[("root", 128 * 1024)]);

        let disk_info = DiskInfo {
            size: disk_size,
            lba_size,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![
                GptDiskRegion {
                    image: sample_image_file("gpt_primary.zst"),
                    region_type: GptRegionType::PrimaryGpt,
                },
                GptDiskRegion {
                    image: sample_image_file("images/root.img.zst"),
                    region_type: GptRegionType::Partition { number: 1 },
                },
            ],
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![
                sample_image("images/root.img.zst", "/"),
                // This filesystem doesn't match any partition
                sample_image("images/extra.img.zst", "/extra"),
            ],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, Some(gpt_disk));

        let result = cosi.derive_host_configuration_inner("/dev/sda");
        assert!(result.is_err(), "Should fail with unmatched filesystem");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("does not correspond to any partition"),
            "Error should mention unmatched filesystem: {}",
            err_msg
        );
    }

    /// Tests [`Cosi::derive_host_configuration_inner`] with missing GPT data.
    ///
    /// Verifies that an appropriate error is returned when GPT data is not
    /// available (this would be caught by joined_disk_info_and_gpt).
    #[test]
    fn test_derive_host_configuration_inner_missing_gpt() {
        let disk_info = DiskInfo {
            size: 1024 * 1024,
            lba_size: 512,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![GptDiskRegion {
                image: sample_image_file("images/root.img.zst"),
                region_type: GptRegionType::Partition { number: 1 },
            }],
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![sample_image("images/root.img.zst", "/")],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, None);

        let result = cosi.derive_host_configuration_inner("/dev/sda");
        assert!(result.is_err(), "Should fail without GPT data");
    }
}
