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

use super::{
    metadata::{GptRegionType, KnownMetadataVersion},
    Cosi,
};

impl Cosi {
    /// Derives the `image` and `storage` sections of the host configuration
    /// from the COSI file. This requires COSI >= 1.2.
    pub(super) fn derive_host_configuration(
        &mut self,
        target_disk: impl AsRef<Path>,
    ) -> Result<HostConfiguration, Error> {
        ensure!(
            self.metadata.version >= KnownMetadataVersion::V1_2,
            "Host configuration derivation requires COSI version {} or higher, found {}",
            KnownMetadataVersion::V1_2,
            self.metadata.version
        );

        // If we don't have GPT data, attempt to populate it from the disk metadata.
        if self.gpt.is_none() {
            self.populate_gpt_data()
                .context("Failed to populate GPT data for COSI version >= 1.2")?;
        }

        self.derive_host_configuration_inner(target_disk)
            .context("Failed to derive host configuration from COSI metadata and GPT data")
    }

    /// A helper function that performs the actual derivation of the host
    /// configuration assuming that the necessary metadata and GPT data are
    /// present. This is separated from `derive_host_configuration` to:
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

        let mut id_gen = IdGenerator::new("partition-");

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
            .gpt
            .as_ref()
            .with_context(|| {
                format!(
                    "COSI is version {}, but GPT data is missing",
                    self.metadata.version
                )
            })?
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

struct JointPartitionMetadata {
    partition_size: u64,
    partition_uuid: Uuid,
    partition_label: String,
    partition_type: DiscoverablePartitionType,
    image_path: PathBuf,
}

// #[cfg(test)]
// mod tests {
//     use super::*;

//     use std::path::PathBuf;

//     use itertools::izip;
//     use sysdefs::arch::SystemArchitecture;
//     use uuid::Uuid;

//     use trident_api::{config::HostConfiguration, primitives::hash::Sha384Hash};

//     use crate::osimage::{
//         cosi::metadata::{Image, ImageFile, Partition as CosiPartition},
//         OsImageFileSystemType,
//     };

//     #[test]
//     fn test_derive_host_configuration_ok() {
//         let metadata = CosiMetadata {
//             version: KnownMetadataVersion::V1_2.as_version(),
//             os_arch: SystemArchitecture::Amd64,
//             partitions: Some(vec![
//                 CosiPartition {
//                     path: Some(PathBuf::from("/images/root.img")),
//                     number: 2,
//                     part_type: DiscoverablePartitionType::Root.to_uuid(),
//                     part_uuid: Uuid::parse_str("11111111-2222-3333-4444-666666666666").unwrap(),
//                     label: "root_part".to_string(),
//                     original_size: 16 * 1024 * 1024, // 16 MiB
//                 },
//                 CosiPartition {
//                     path: Some(PathBuf::from("/images/esp.img")),
//                     number: 1,
//                     part_type: DiscoverablePartitionType::Esp.to_uuid(),
//                     part_uuid: Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap(),
//                     label: "esp_part".to_string(),
//                     original_size: 4 * 1024 * 1024, // 4 MiB
//                 },
//             ]),
//             images: vec![
//                 Image {
//                     file: ImageFile {
//                         path: PathBuf::from("/images/esp.img"),
//                         compressed_size: 4096,
//                         uncompressed_size: 8192,
//                         sha384: Sha384Hash::from("1"),
//                         entry: Default::default(),
//                     },
//                     mount_point: "/boot/efi".into(),
//                     fs_type: OsImageFileSystemType::Ext4,
//                     fs_uuid: Uuid::parse_str("66666666-7777-8888-9999-aaaaaaaaaaaa")
//                         .unwrap()
//                         .into(),
//                     part_type: DiscoverablePartitionType::Esp,
//                     verity: None,
//                 },
//                 Image {
//                     file: ImageFile {
//                         path: PathBuf::from("/images/root.img"),
//                         compressed_size: 8192,
//                         uncompressed_size: 16384,
//                         sha384: Sha384Hash::from("1"),
//                         entry: Default::default(),
//                     },
//                     mount_point: "/".into(),
//                     fs_type: OsImageFileSystemType::Ext4,
//                     fs_uuid: Uuid::parse_str("66666666-7777-8888-9999-aaaaaaaaaaaa")
//                         .unwrap()
//                         .into(),
//                     part_type: DiscoverablePartitionType::Root,
//                     verity: None,
//                 },
//             ],
//             os_release: Default::default(),
//             os_packages: Default::default(),
//             id: Default::default(),
//             bootloader: Default::default(),
//         };

//         let target_disk = "/dev/sda";

//         let hc = HostConfiguration {
//             storage: metadata
//                 .derive_host_configuration_storage(target_disk)
//                 .unwrap(),
//             ..Default::default()
//         };

//         hc.validate().unwrap();

//         assert_eq!(hc.storage.disks.len(), 1);
//         assert_eq!(hc.storage.disks[0].device, Path::new(target_disk));
//         assert_eq!(hc.storage.disks[0].partitions.len(), 2);
//         assert_eq!(hc.storage.filesystems.len(), 2);

//         for (original_partition, original_fs, partition, filesystem) in izip!(
//             // Reversed to match partition number ordering, this tests that the
//             // number was used instead of the order in the vec.
//             metadata.partitions.as_ref().unwrap().iter().rev(),
//             metadata.images.iter(),
//             hc.storage.disks[0].partitions.iter(),
//             hc.storage.filesystems.iter()
//         ) {
//             assert_eq!(
//                 partition.size.to_bytes(),
//                 Some(original_partition.original_size)
//             );
//             assert_eq!(partition.uuid.unwrap(), original_partition.part_uuid,);
//             assert_eq!(partition.label, Some(original_partition.label.to_string()));
//             assert_eq!(
//                 partition.partition_type,
//                 DiscoverablePartitionType::from_uuid(&original_partition.part_type).into()
//             );

//             assert_eq!(
//                 filesystem.mount_point,
//                 Some(original_fs.mount_point.as_path().into())
//             );
//             assert_eq!(filesystem.source, FileSystemSource::Image);
//             assert_eq!(filesystem.device_id, Some(partition.id.clone()));
//         }
//     }

//     #[test]
//     fn test_derive_host_configuration_missing_image() {
//         let metadata = CosiMetadata {
//             version: KnownMetadataVersion::V1_2.as_version(),
//             os_arch: SystemArchitecture::Amd64,
//             partitions: Some(vec![CosiPartition {
//                 path: Some(PathBuf::from("/images/root.img")),
//                 number: 1,
//                 part_type: DiscoverablePartitionType::LinuxGeneric.to_uuid(),
//                 part_uuid: Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap(),
//                 label: "root".to_string(),
//                 original_size: 4 * 1024 * 1024, // 4 MiB
//             }]),
//             images: vec![],
//             os_release: Default::default(),
//             os_packages: Default::default(),
//             id: Default::default(),
//             bootloader: Default::default(),
//         };

//         let target_disk = "/dev/sda";
//         let err = metadata
//             .derive_host_configuration_storage(target_disk)
//             .unwrap_err();

//         assert!(err
//             .to_string()
//             .contains("No image metadata found for partition at path"));
//     }

//     #[test]
//     fn test_derive_host_configuration_unsupported_version() {
//         let metadata = CosiMetadata {
//             version: KnownMetadataVersion::V1_1.as_version(),
//             os_arch: SystemArchitecture::Amd64,
//             partitions: None,
//             images: vec![],
//             os_release: Default::default(),
//             os_packages: Default::default(),
//             id: Default::default(),
//             bootloader: Default::default(),
//         };

//         let target_disk = "/dev/sda";
//         let err = metadata
//             .derive_host_configuration_storage(target_disk)
//             .unwrap_err();

//         assert!(err.to_string().contains(
//             "Host configuration derivation requires COSI metadata version 1.2 or higher"
//         ));
//     }
// }
