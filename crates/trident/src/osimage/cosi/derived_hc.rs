// use std::{collections::HashMap, path::Path};

// use anyhow::{bail, Error};

// use sysdefs::partition_types::DiscoverablePartitionType;
// use trident_api::{
//     config::{Disk, FileSystem, FileSystemSource, Partition, Storage},
//     misc::IdGenerator,
// };

// use crate::osimage::cosi::metadata::KnownMetadataVersion;

// use super::metadata::CosiMetadata;

// impl CosiMetadata {
//     /// Derives a host configuration from the COSI >= v1.2 metadata.
//     pub(super) fn derive_host_configuration_storage(
//         &self,
//         _target_disk: impl AsRef<Path>,
//     ) -> Result<Storage, Error> {
//         // if self.version < KnownMetadataVersion::V1_2 {
//         //     bail!("Host configuration derivation requires COSI metadata version {} or higher, found {}", KnownMetadataVersion::V1_2, self.version);
//         // }

//         // let disk_metadata = {
//         //     let Some(mut partition_metadata) = self.disk.clone() else {
//         //         // This should be caught during validation.
//         //         bail!(
//         //             "COSI metadata version is {}, but partitions metadata is missing",
//         //             self.version
//         //         );
//         //     };

//         //     // Sort partitions by number to ensure consistent ordering.
//         //     partition_metadata.sort_by_key(|a| a.number);

//         //     partition_metadata
//         // };

//         // let mut partitions = vec![];
//         // let mut filesystems = vec![];
//         // let mut id_gen = IdGenerator::new("partition");

//         // let filesystems_by_image = self
//         //     .images
//         //     .iter()
//         //     .map(|image| (image.file.path.as_path(), image))
//         //     .collect::<HashMap<_, _>>();

//         // for part in partition_metadata {
//         //     let partition_id = id_gen.next_id();
//         //     partitions.push(Partition {
//         //         id: partition_id.clone(),
//         //         partition_type: DiscoverablePartitionType::from_uuid(&part.part_type).into(),
//         //         size: part.original_size.into(),
//         //         uuid: Some(part.part_uuid),
//         //         label: Some(part.label),
//         //     });

//         //     let Some(path) = &part.path else {
//         //         continue;
//         //     };

//         //     let Some(fs_metadata) = filesystems_by_image.get(path.as_path()) else {
//         //         bail!("No image metadata found for partition at path {:?}", path);
//         //     };

//         //     filesystems.push(FileSystem {
//         //         device_id: Some(partition_id),
//         //         source: FileSystemSource::Image,
//         //         mount_point: Some(fs_metadata.mount_point.as_path().into()),
//         //     });
//         // }

//         Ok(Storage {
//             ..Default::default()
//         })
//     }
// }

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
