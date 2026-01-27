use std::{collections::HashMap, path::Path};

use anyhow::{bail, Error};
use sysdefs::partition_types::DiscoverablePartitionType;
use trident_api::{
    config::{Disk, FileSystem, FileSystemSource, HostConfiguration, Partition, Storage},
    misc::IdGenerator,
};

use crate::osimage::cosi::metadata::KnownMetadataVersion;

use super::metadata::CosiMetadata;

impl CosiMetadata {
    /// Derives a host configuration from the COSI >= v1.2 metadata.
    pub fn derive_host_configuration(
        &self,
        target_disk: impl AsRef<Path>,
    ) -> Result<HostConfiguration, Error> {
        if self.version < KnownMetadataVersion::V1_2 {
            bail!("Host configuration derivation requires COSI metadata version 1.2 or higher, found {}", self.version);
        }

        let partition_metadata = {
            let Some(mut partition_metadata) = self.partitions.clone() else {
                bail!(
                    "COSI metadata version is {}, but partitions metadata is missing",
                    self.version
                );
            };

            // Sort partitions by number to ensure consistent ordering.
            partition_metadata.sort_by_key(|a| a.number);

            partition_metadata
        };

        let mut partitions = vec![];
        let mut filesystems = vec![];
        let mut id_gen = IdGenerator::new("partition");

        let filesystems_by_image = self
            .images
            .iter()
            .map(|image| (image.file.path.as_path(), image))
            .collect::<HashMap<_, _>>();

        for part in partition_metadata {
            let partition_id = id_gen.next_id();
            partitions.push(Partition {
                id: partition_id.clone(),
                partition_type: DiscoverablePartitionType::from_uuid(&part.part_type).into(),
                size: part.original_size.into(),
                uuid: Some(part.part_uuid),
                label: Some(part.label),
            });

            let Some(path) = &part.path else {
                continue;
            };

            let Some(fs_metadata) = filesystems_by_image.get(path.as_path()) else {
                bail!("No image metadata found for partition at path {:?}", path);
            };

            filesystems.push(FileSystem {
                device_id: Some(partition_id),
                source: FileSystemSource::Image,
                mount_point: Some(fs_metadata.mount_point.as_path().into()),
            });
        }

        Ok(HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk0".to_string(),
                    device: target_disk.as_ref().into(),
                    partitions,
                    partition_table_type: Default::default(),
                    adopted_partitions: vec![],
                }],
                filesystems,
                ..Default::default()
            },
            ..Default::default()
        })
    }
}
