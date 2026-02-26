use std::{collections::HashMap, path::Path};

use anyhow::{bail, Context, Error};
use url::Url;

use trident_api::{
    config::{
        Disk, FileSystem, FileSystemSource, HostConfiguration, ImageSha384, MountOptions,
        MountPoint, OsImage as ConfigOsImage, Partition, Storage, VerityDevice,
    },
    constants::{
        ROOT_MOUNT_POINT_PATH, ROOT_VERITY_DEVICE_NAME, USR_MOUNT_POINT_PATH,
        USR_VERITY_DEVICE_NAME,
    },
    misc::IdGenerator,
    primitives::hash::Sha384Hash,
};

use super::{metadata::Image, CosiPartitioningInfo};

/// A helper function that performs the actual derivation of the host
/// configuration assuming that the COSI version is sufficient and the
/// necessary metadata and GPT data are present. This is separated from
/// `derive_host_configuration` to allow for easier testing since we can
/// directly construct a COSI object with the required fields without having
/// to go through the GPT population logic.
pub(super) fn derive_host_configuration_inner(
    source_url: &Url,
    metadata_sha384: &Sha384Hash,
    target_disk: impl AsRef<Path>,
    filesystems: &[Image],
    partitioning_info: &CosiPartitioningInfo,
) -> Result<HostConfiguration, Error> {
    let mut filesystems_by_path = filesystems
        .iter()
        .map(|image| (image.file.path.as_path(), image))
        .collect::<HashMap<_, _>>();

    let mut partition_id_gen = IdGenerator::new_with_start("partition", 1);
    let mut verity_id_gen = IdGenerator::new_with_start("verity", 1);

    let partition_ids_by_file = partitioning_info
        .partitions
        .values()
        .map(|part| (part.image_file.path.as_path(), partition_id_gen.next_id()))
        .collect::<HashMap<_, _>>();

    // The vecs we will be populating
    let mut partitions = Vec::new();
    let mut filesystems = Vec::new();
    let mut verity = Vec::new();

    for part in partitioning_info.partitions.values() {
        let partition_id = partition_ids_by_file
            .get(part.image_file.path.as_path())
            .with_context(|| {
                format!(
                    "Failed to find partition ID for partition image file: {}",
                    part.image_file.path.display()
                )
            })?;

        partitions.push(Partition {
            id: partition_id.clone(),
            // Ensure size is aligned to 4096 bytes.
            size: part.info.size.next_multiple_of(4096).into(),
            uuid: Some(part.info.part_uuid),
            label: Some(part.info.name.clone()),
            partition_type: part.info.part_type.into(),
        });

        let Some(filesystem_metadata) = filesystems_by_path.remove(part.image_file.path.as_path())
        else {
            // There is no filesystem associated to this partition.
            continue;
        };

        if let Some(verity_device) = filesystem_metadata.verity.as_ref() {
            // This partition has verity, so we need to derive the verity device from it.

            // First, get the id of the hash partition.
            let hash_partition_id = partition_ids_by_file
                .get(verity_device.file.path.as_path())
                .with_context(|| {
                    format!(
                        "Failed to find hash partition for verity device: {}",
                        verity_device.file.path.display()
                    )
                })?;

            let verity_id = verity_id_gen.next_id();

            let verity_name = match filesystem_metadata.mount_point.as_path() {
                // Verity devices in / and /usr have a strict requirement on the name.
                s if s == Path::new(ROOT_MOUNT_POINT_PATH) => ROOT_VERITY_DEVICE_NAME.to_string(),
                s if s == Path::new(USR_MOUNT_POINT_PATH) => USR_VERITY_DEVICE_NAME.to_string(),
                other => bail!(
                    "dm-verity at path '{}' is currently unsupported. Only '{}' and '{}' are supported.",
                    other.display(),
                    ROOT_MOUNT_POINT_PATH,
                    USR_MOUNT_POINT_PATH
                ),
            };

            verity.push(VerityDevice {
                id: verity_id.clone(),
                name: verity_name,
                data_device_id: partition_id.clone(),
                hash_device_id: hash_partition_id.clone(),
                corruption_option: Default::default(),
            });

            // Add this filesystem on top of the verity device since it has verity.
            filesystems.push(FileSystem {
                device_id: Some(verity_id.clone()),
                mount_point: Some(MountPoint {
                    path: filesystem_metadata.mount_point.clone(),
                    options: MountOptions::defaults().with("ro"),
                }),
                source: FileSystemSource::Image,
            });
        } else {
            // Add this filesystem directly on top of the partition since there is no verity.
            filesystems.push(FileSystem {
                device_id: Some(partition_id.clone()),
                mount_point: Some(filesystem_metadata.mount_point.as_path().into()),
                source: FileSystemSource::Image,
            });
        };
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
            url: source_url.clone(),
            sha384: ImageSha384::Checksum(metadata_sha384.clone()),
        }),
        storage: Storage {
            disks: vec![Disk {
                id: "disk-0".to_string(),
                device: target_disk.as_ref().to_path_buf(),
                partitions,
                ..Default::default()
            }],
            filesystems,
            verity,
            ..Default::default()
        },
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{io::Cursor, path::PathBuf};

    use gpt::{disk::LogicalBlockSize, mbr::ProtectiveMBR, GptConfig};
    use url::Url;
    use uuid::Uuid;

    use osutils::osrelease::OsRelease;
    use sysdefs::{
        arch::SystemArchitecture, osuuid::OsUuid, partition_types::DiscoverablePartitionType,
    };
    use trident_api::{config::FileSystemSource, primitives::hash::Sha384Hash};

    use crate::{
        io_utils::file_reader::FileReader,
        osimage::{
            cosi::{
                self,
                entries::CosiEntries,
                metadata::{
                    CosiMetadata, DiskInfo, GptDiskRegion, GptRegionType, Image, ImageFile,
                    PartitionTableType, VerityMetadata,
                },
                Cosi, KnownMetadataVersion,
            },
            OsImageFileSystemType,
        },
    };

    /// Creates a mock GPT disk in memory with the specified partitions.
    ///
    /// Returns a tuple of (gpt_region_raw, disk_size, lba_size) where
    /// `gpt_region_raw` is the raw bytes of the GPT region that can be used to
    /// populate the COSI metadata, `disk_size` is the total size of the disk,
    /// and `lba_size` is the logical block size used in the GPT.
    ///
    /// Each partition is defined by (name, size_bytes).
    fn create_mock_gpt_disk(partitions: &[(&str, u64)]) -> (Vec<u8>, u64, u32) {
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

        (disk_buffer, disk_size, lba_size)
    }

    /// Creates a minimal Cosi instance for testing with the given metadata and GPT.
    fn create_test_cosi(metadata: CosiMetadata, raw_gpt: Option<Vec<u8>>) -> Cosi {
        let partitioning_info = raw_gpt.map(|gpt_data| {
            cosi::create_cosi_partitioning_info(gpt_data, metadata.disk.as_ref().unwrap().clone())
                .unwrap()
        });

        Cosi {
            metadata,
            partitioning_info,
            source: Url::parse("file:///test/image.cosi").unwrap(),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
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
        let (raw_gpt, disk_size, lba_size) =
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

        let cosi = create_test_cosi(metadata, Some(raw_gpt));
        let target_disk = "/dev/sda";
        let result = derive_host_configuration_inner(
            &cosi.source,
            &cosi.metadata_sha384,
            target_disk,
            &cosi.metadata.images,
            cosi.partitioning_info.as_ref().unwrap(),
        );
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

        // Verify partitions have sequential IDs (starting at 1).
        assert_eq!(hc.storage.disks[0].partitions[0].id, "partition-1");
        assert_eq!(hc.storage.disks[0].partitions[1].id, "partition-2");

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
            Some("partition-1".to_string())
        );
        assert_eq!(
            hc.storage.filesystems[0].mount_point,
            Some("/boot/efi".into())
        );
        assert_eq!(hc.storage.filesystems[0].source, FileSystemSource::Image);

        // Second filesystem (root).
        assert_eq!(
            hc.storage.filesystems[1].device_id,
            Some("partition-2".to_string())
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
        let (raw_gpt, disk_size, lba_size) = create_mock_gpt_disk(&[
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

        let cosi = create_test_cosi(metadata, Some(raw_gpt));

        let result = derive_host_configuration_inner(
            &cosi.source,
            &cosi.metadata_sha384,
            "/dev/sda",
            &cosi.metadata.images,
            cosi.partitioning_info.as_ref().unwrap(),
        );
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
        let (raw_gpt, disk_size, lba_size) = create_mock_gpt_disk(&[("root", 128 * 1024)]);

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

        let cosi = create_test_cosi(metadata, Some(raw_gpt));

        let result = derive_host_configuration_inner(
            &cosi.source,
            &cosi.metadata_sha384,
            "/dev/sda",
            &cosi.metadata.images,
            cosi.partitioning_info.as_ref().unwrap(),
        );
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
    /// Verifies that when GPT data is not available, the partitioning info is
    /// `None`. This would be caught by the caller before
    /// `derive_host_configuration_inner` is invoked.
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

        // When no GPT data is provided, partitioning_info should be None.
        // The caller is responsible for checking this before invoking
        // derive_host_configuration_inner.
        assert!(
            cosi.partitioning_info.is_none(),
            "partitioning_info should be None without GPT data"
        );
    }

    /// Tests [`derive_host_configuration_inner`] with verity-enabled filesystems.
    ///
    /// Creates a COSI setup with a root data partition, a root hash partition,
    /// and a /usr data partition with its hash partition. Verifies that:
    /// - Verity devices are created with correct names ("root" for /, "usr" for /usr).
    /// - Verity devices reference the correct data and hash partition IDs.
    /// - Filesystems are mounted on top of verity devices with read-only options.
    /// - Non-verity partitions (ESP) are handled normally alongside verity ones.
    #[test]
    fn test_derive_host_configuration_inner_with_verity() {
        let (raw_gpt, disk_size, lba_size) = create_mock_gpt_disk(&[
            ("esp", 64 * 1024),
            ("root", 256 * 1024),
            ("root-hash", 32 * 1024),
            ("usr", 256 * 1024),
            ("usr-hash", 32 * 1024),
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
                    image: sample_image_file("images/root.img.zst"),
                    region_type: GptRegionType::Partition { number: 2 },
                },
                GptDiskRegion {
                    image: sample_image_file("images/root-hash.img.zst"),
                    region_type: GptRegionType::Partition { number: 3 },
                },
                GptDiskRegion {
                    image: sample_image_file("images/usr.img.zst"),
                    region_type: GptRegionType::Partition { number: 4 },
                },
                GptDiskRegion {
                    image: sample_image_file("images/usr-hash.img.zst"),
                    region_type: GptRegionType::Partition { number: 5 },
                },
            ],
        };

        // Root filesystem with verity pointing to the root-hash partition.
        let root_image = Image {
            file: sample_image_file("images/root.img.zst"),
            mount_point: PathBuf::from("/"),
            fs_type: OsImageFileSystemType::Ext4,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::LinuxGeneric,
            verity: Some(VerityMetadata {
                file: sample_image_file("images/root-hash.img.zst"),
                roothash: "abcd1234".to_string(),
            }),
        };

        // /usr filesystem with verity pointing to the usr-hash partition.
        let usr_image = Image {
            file: sample_image_file("images/usr.img.zst"),
            mount_point: PathBuf::from("/usr"),
            fs_type: OsImageFileSystemType::Ext4,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::LinuxGeneric,
            verity: Some(VerityMetadata {
                file: sample_image_file("images/usr-hash.img.zst"),
                roothash: "efgh5678".to_string(),
            }),
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![
                sample_image("images/esp.img.zst", "/boot/efi"),
                root_image,
                usr_image,
            ],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, Some(raw_gpt));
        let result = derive_host_configuration_inner(
            &cosi.source,
            &cosi.metadata_sha384,
            "/dev/sda",
            &cosi.metadata.images,
            cosi.partitioning_info.as_ref().unwrap(),
        );
        assert!(
            result.is_ok(),
            "derive_host_configuration_inner with verity should succeed: {:?}",
            result.err()
        );

        let hc = result.unwrap();

        // 5 partitions: esp, root, root-hash, usr, usr-hash.
        assert_eq!(
            hc.storage.disks[0].partitions.len(),
            5,
            "Should have 5 partitions"
        );

        // 2 verity devices: root and usr.
        assert_eq!(hc.storage.verity.len(), 2, "Should have 2 verity devices");

        // Verify root verity device.
        assert_eq!(hc.storage.verity[0].name, "root");
        // Root data partition is partition-2, root hash partition is partition-3.
        assert_eq!(hc.storage.verity[0].data_device_id, "partition-2");
        assert_eq!(hc.storage.verity[0].hash_device_id, "partition-3");

        // Verify usr verity device.
        assert_eq!(hc.storage.verity[1].name, "usr");
        // Usr data partition is partition-4, usr hash partition is partition-5.
        assert_eq!(hc.storage.verity[1].data_device_id, "partition-4");
        assert_eq!(hc.storage.verity[1].hash_device_id, "partition-5");

        // 3 filesystems: ESP (on partition), root (on verity), usr (on verity).
        assert_eq!(hc.storage.filesystems.len(), 3, "Should have 3 filesystems");

        // ESP filesystem is on the partition directly.
        assert_eq!(
            hc.storage.filesystems[0].device_id,
            Some("partition-1".to_string())
        );
        assert_eq!(
            hc.storage.filesystems[0].mount_point,
            Some("/boot/efi".into())
        );

        // Root filesystem is on the verity device.
        assert_eq!(
            hc.storage.filesystems[1].device_id,
            Some(hc.storage.verity[0].id.clone())
        );
        let root_mp = hc.storage.filesystems[1].mount_point.as_ref().unwrap();
        assert_eq!(root_mp.path, Path::new("/"));
        assert!(
            root_mp.options.contains("ro"),
            "Root verity filesystem should be read-only"
        );

        // Usr filesystem is on the verity device.
        assert_eq!(
            hc.storage.filesystems[2].device_id,
            Some(hc.storage.verity[1].id.clone())
        );
        let usr_mp = hc.storage.filesystems[2].mount_point.as_ref().unwrap();
        assert_eq!(usr_mp.path, Path::new("/usr"));
        assert!(
            usr_mp.options.contains("ro"),
            "Usr verity filesystem should be read-only"
        );
    }

    /// Tests [`derive_host_configuration_inner`] with verity at unsupported mount point.
    ///
    /// Verifies that an error is returned when a verity-enabled filesystem has a
    /// mount point other than "/" or "/usr".
    #[test]
    fn test_derive_host_configuration_inner_verity_unsupported_mount_point() {
        let (raw_gpt, disk_size, lba_size) =
            create_mock_gpt_disk(&[("var", 256 * 1024), ("var-hash", 32 * 1024)]);

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
                    image: sample_image_file("images/var.img.zst"),
                    region_type: GptRegionType::Partition { number: 1 },
                },
                GptDiskRegion {
                    image: sample_image_file("images/var-hash.img.zst"),
                    region_type: GptRegionType::Partition { number: 2 },
                },
            ],
        };

        let var_image = Image {
            file: sample_image_file("images/var.img.zst"),
            mount_point: PathBuf::from("/var"),
            fs_type: OsImageFileSystemType::Ext4,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::LinuxGeneric,
            verity: Some(VerityMetadata {
                file: sample_image_file("images/var-hash.img.zst"),
                roothash: "badhash".to_string(),
            }),
        };

        let metadata = CosiMetadata {
            version: KnownMetadataVersion::V1_2.as_version(),
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            images: vec![var_image],
            bootloader: None,
            disk: Some(disk_info),
            compression: None,
        };

        let cosi = create_test_cosi(metadata, Some(raw_gpt));
        let result = derive_host_configuration_inner(
            &cosi.source,
            &cosi.metadata_sha384,
            "/dev/sda",
            &cosi.metadata.images,
            cosi.partitioning_info.as_ref().unwrap(),
        );
        assert!(
            result.is_err(),
            "Should fail with verity at unsupported mount point"
        );

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("unsupported"),
            "Error should mention unsupported verity path: {}",
            err_msg
        );
    }
}
