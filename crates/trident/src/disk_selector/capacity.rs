use std::{collections::HashMap, path::Path};

use anyhow::{bail, ensure, Context, Error};
use trident_api::{
    config::{Disk, Storage},
    storage_graph::{graph::StorageGraph, types::CalculatedSize},
    BlockDeviceId,
};

use crate::osimage::{OsImage, OsImageFileSystem};

const MIB: u64 = 1024 * 1024;
const GRAIN: u64 = 4096;
const GPT_RESERVE: u64 = 2 * MIB;
// systemd-repart's default SizeMinBytes for partitions without an explicit size.
const GROW_MINIMUM: u64 = 10 * MIB;

/// Computes layout requirements in bytes for the supplied disks.
///
/// Fixed partition sizes come exclusively from the HC. COSI metadata is queried
/// only for filesystems backed by grow partitions, to estimate their
/// minima including storage-layer overhead. Adds GPT reserves and accounts for
/// equal grow allocation, but not transient scratch files or later OS writes.
/// Reuses the graph produced by HC validation instead of constructing another.
pub(super) fn requirements<'a>(
    storage: &Storage,
    image: &OsImage,
    graph: &StorageGraph,
    disks: impl Iterator<Item = &'a Disk>,
) -> Result<HashMap<BlockDeviceId, u64>, Error> {
    let sizes = graph.calculate_partition_sizes()?;

    let esp_mount = storage
        .esp_filesystem()
        .and_then(|(_, filesystem)| filesystem.mount_point_path());

    from_partition_sizes(disks, &sizes, |mount| {
        let payload = if esp_mount == Some(mount) {
            image
                .esp_filesystem()
                .context("Missing COSI ESP metadata")?
        } else {
            image
                .filesystems()
                .find(|image_fs| image_fs.mount_point == mount)
                .with_context(|| format!("COSI has no filesystem for '{}'", mount.display()))?
        };
        Ok(payload_sizes(&payload))
    })
}

/// Extracts uncompressed data and optional verity-hash lengths from image metadata.
fn payload_sizes(filesystem: &OsImageFileSystem) -> (u64, Option<u64>) {
    (
        filesystem.image_file.uncompressed_size,
        filesystem
            .verity
            .as_ref()
            .map(|verity| verity.hash_image_file.uncompressed_size),
    )
}

/// Computes disk minima from the HC, looking up only grow-backed images.
///
/// Fixed partitions never require image metadata. Grow partitions share space
/// equally, so each must accommodate the largest grow requirement on its disk.
fn from_partition_sizes<'a>(
    disks: impl Iterator<Item = &'a Disk>,
    sizes: &HashMap<BlockDeviceId, CalculatedSize>,
    image_filesystem: impl Fn(&Path) -> Result<(u64, Option<u64>), Error>,
) -> Result<HashMap<BlockDeviceId, u64>, Error> {
    disks
        .map(|disk| {
            let mut fixed = GPT_RESERVE;
            let mut grow_count = 0_u64;
            let mut grow_min = GROW_MINIMUM;

            for part in &disk.partitions {
                let size = sizes.get(&part.id).with_context(|| {
                    format!("Missing calculated size for partition '{}'", part.id)
                })?;
                match size {
                    CalculatedSize::Fixed(bytes) => {
                        fixed = fixed
                            .checked_add(*bytes)
                            .context("Disk layout size overflow")?;
                    }
                    CalculatedSize::FromFilesystem { fs, extra }
                    | CalculatedSize::FromVerityHash { fs, extra } => {
                        let (data, hash) = image_filesystem(fs)?;
                        let bytes = if matches!(size, CalculatedSize::FromVerityHash { .. }) {
                            hash.with_context(|| {
                                format!(
                                    "Missing COSI verity hash uncompressed size for '{}'",
                                    fs.display()
                                )
                            })?
                        } else {
                            data
                        };

                        ensure!(
                            bytes > 0,
                            "COSI filesystem '{}' has no usable uncompressed size",
                            fs.display()
                        );

                        let required = bytes
                            .checked_add(*extra)
                            .context("Grow partition size overflow")?;

                        grow_count += 1;
                        grow_min = grow_min.max(required);
                    }
                    CalculatedSize::GrowPartitionWithNewFilesystem { .. } => {
                        // TODO: Consider the strategy kind to react accordingly in this
                        // edge case.
                        //
                        // Minimization strategies should continue to fail as we cannot
                        // possibly infer what the intended minimum size should be.
                        //
                        // Size-lookup and largest disk strategies could potentially use
                        // a default minimum size.
                        bail!(
                            "Cannot infer the minimum size of newly formatted grow partition '{}'; \
                             specify a fixed partition size",
                            part.id
                        );
                    }
                    CalculatedSize::GrowPartitionUnformatted { extra } => {
                        let required = GROW_MINIMUM
                            .checked_add(*extra)
                            .context("Grow partition size overflow")?;
                        grow_count += 1;
                        grow_min = grow_min.max(required);
                    }
                }
            }

            // Unbounded partitions have equal repart weights, so each needs
            // enough room for the largest grow payload on this disk.
            let grow_min = grow_min
                .checked_next_multiple_of(GRAIN)
                .context("Grow partition size overflow")?;
            let size = grow_min
                .checked_mul(grow_count)
                .and_then(|grow| fixed.checked_add(grow))
                .context("Disk layout size overflow")?;

            Ok((disk.id.clone(), size))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use sysdefs::{osuuid::OsUuid, partition_types::DiscoverablePartitionType};
    use trident_api::config::{
        AbUpdate, AbVolumePair, Disk, EncryptedVolume, Encryption, FileSystem, FileSystemSource,
        MountPoint, NewFileSystemType, Partition, PartitionSize, PartitionType, Swap, VerityDevice,
    };

    use crate::osimage::{
        mock::MockOsImage, OsImageFile, OsImageFileSystemType, OsImageVerityHash,
    };

    /// Supplies synthetic image metadata only when the sizing code requests it.
    fn from_filesystems(
        storage: &Storage,
        filesystems: &[OsImageFileSystem],
    ) -> Result<HashMap<BlockDeviceId, u64>, Error> {
        let sizes = storage.build_graph()?.calculate_partition_sizes()?;
        super::from_partition_sizes(storage.disks.iter(), &sizes, |mount| {
            filesystems
                .iter()
                .find(|image_fs| image_fs.mount_point == mount)
                .map(payload_sizes)
                .context("Missing test image filesystem")
        })
    }

    /// Provides authored partition sizes and synthetic COSI metadata for capacity tests.
    fn fixture() -> (Storage, Vec<OsImageFileSystem>) {
        (
            Storage {
                disks: vec![Disk {
                    id: "os".into(),
                    device: "/dev/sda".into(),
                    partitions: vec![
                        Partition::new("fixed", 100 * MIB),
                        Partition::new("grow", PartitionSize::Grow),
                    ],
                    ..Default::default()
                }],
                filesystems: vec![FileSystem {
                    device_id: Some("grow".into()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: "/".into(),
                        options: Default::default(),
                    }),
                    is_esp: false,
                }],
                ..Default::default()
            },
            vec![OsImageFileSystem {
                mount_point: PathBuf::from("/"),
                fs_type: OsImageFileSystemType::Ext4,
                fs_uuid: OsUuid::Uuid(uuid::Uuid::nil()),
                part_type: DiscoverablePartitionType::LinuxGeneric,
                image_file: OsImageFile {
                    compressed_size: 1,
                    uncompressed_size: 200 * MIB,
                    path: "/root.raw.zstd".into(),
                    sha384: "ignored".into(),
                },
                verity: None,
            }],
        )
    }

    #[test]
    fn capacity_uses_authored_layout_and_uncompressed_payload() {
        let (storage, image) = fixture();
        assert_eq!(from_filesystems(&storage, &image).unwrap()["os"], 302 * MIB);
        let mut larger_payload = image;
        larger_payload[0].image_file.uncompressed_size = 300 * MIB;
        assert_eq!(
            from_filesystems(&storage, &larger_payload).unwrap()["os"],
            402 * MIB
        );
    }

    #[test]
    fn fixed_partitions_use_hc_sizes_without_image_requirements() {
        let (mut storage, mut image) = fixture();
        storage.disks[0].partitions[1].size = (100 * MIB).into();
        assert_eq!(from_filesystems(&storage, &[]).unwrap()["os"], 202 * MIB);
        for size in [0, u64::MAX] {
            image[0].image_file.uncompressed_size = size;
            assert_eq!(from_filesystems(&storage, &image).unwrap()["os"], 202 * MIB);
        }
    }

    #[test]
    fn ab_fixed_partitions_both_contribute_to_capacity() {
        let (mut storage, mut image) = fixture();
        storage.disks[0].partitions[1].size = (200 * MIB).into();
        storage.disks[0]
            .partitions
            .push(Partition::new("root-b", 200 * MIB));
        storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".into(),
                volume_a_id: "grow".into(),
                volume_b_id: "root-b".into(),
            }],
        });
        storage.filesystems[0].device_id = Some("root".into());
        assert_eq!(from_filesystems(&storage, &image).unwrap()["os"], 502 * MIB);
        image[0].image_file.uncompressed_size += MIB;
        assert_eq!(from_filesystems(&storage, &image).unwrap()["os"], 502 * MIB);
    }

    #[test]
    fn unequal_grow_payloads_account_for_equal_repart_weights() {
        let (mut storage, mut image) = fixture();
        storage.disks[0]
            .partitions
            .push(Partition::new("var", PartitionSize::Grow));
        let mut filesystem = storage.filesystems[0].clone();
        filesystem.device_id = Some("var".into());
        filesystem.mount_point.as_mut().unwrap().path = "/var".into();
        storage.filesystems.push(filesystem);
        let mut var = fixture().1.remove(0);
        var.mount_point = "/var".into();
        var.image_file.uncompressed_size = 100 * MIB;
        image.push(var);
        assert_eq!(from_filesystems(&storage, &image).unwrap()["os"], 502 * MIB);
    }

    #[test]
    fn only_grow_backed_filesystems_require_image_metadata() {
        let (mut storage, image) = fixture();
        let mut fixed_fs = storage.filesystems[0].clone();
        fixed_fs.device_id = Some("fixed".into());
        fixed_fs.mount_point.as_mut().unwrap().path = "/var".into();
        storage.filesystems.push(fixed_fs);
        assert_eq!(from_filesystems(&storage, &image).unwrap()["os"], 302 * MIB);
    }

    #[test]
    fn filesystem_free_grow_partitions_keep_the_repart_floor() {
        let (mut storage, _) = fixture();
        storage.filesystems.clear();
        assert_eq!(from_filesystems(&storage, &[]).unwrap()["os"], 112 * MIB);
    }

    #[test]
    fn encrypted_unformatted_grow_minima_include_reservations() {
        for use_swap in [false, true] {
            for (additional_grow, expected) in [(false, 128 * MIB), (true, 154 * MIB)] {
                let (mut storage, _) = fixture();
                storage.filesystems.clear();
                storage.encryption = Some(Encryption {
                    volumes: vec![EncryptedVolume {
                        id: "encrypted".into(),
                        device_id: "grow".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                });
                if use_swap {
                    storage.disks[0].partitions[1].partition_type = PartitionType::Swap;
                    storage.swap.push(Swap {
                        device_id: "encrypted".into(),
                    });
                }
                if additional_grow {
                    storage.disks[0]
                        .partitions
                        .push(Partition::new("unused", PartitionSize::Grow));
                }
                assert_eq!(
                    from_filesystems(&storage, &[]).unwrap()["os"],
                    expected,
                    "swap: {use_swap}, additional grow: {additional_grow}"
                );
            }
        }
    }

    #[test]
    fn fixed_esp_uses_the_authored_size_for_file_based_deployment() {
        let (mut storage, mut image) = fixture();
        storage.disks[0].partitions[1].size = (1024 * MIB).into();
        storage.disks[0].partitions[0].partition_type = trident_api::config::PartitionType::Esp;
        let mut filesystem = storage.filesystems[0].clone();
        filesystem.device_id = Some("fixed".into());
        filesystem.is_esp = true;
        filesystem.mount_point.as_mut().unwrap().path = "/boot/efi".into();
        storage.filesystems.push(filesystem);
        let mut esp = fixture().1.remove(0);
        esp.mount_point = "/boot/efi".into();
        esp.fs_type = OsImageFileSystemType::Vfat;
        esp.part_type = DiscoverablePartitionType::Esp;
        esp.image_file.path = "/esp.raw.zstd".into();
        esp.image_file.uncompressed_size = 512 * MIB;
        image.push(esp);
        assert_eq!(
            from_filesystems(&storage, &image).unwrap()["os"],
            1126 * MIB
        );
        assert_eq!(
            requirements(
                &storage,
                &OsImage::mock(MockOsImage::new()),
                &storage.build_graph().unwrap(),
                storage.disks.iter(),
            )
            .unwrap()["os"],
            1126 * MIB
        );
    }

    #[test]
    fn fixed_verity_data_does_not_override_grow_hash_requirements() {
        let (mut storage, mut image) = fixture();
        storage.verity.push(VerityDevice {
            id: "root-verity".into(),
            name: "root".into(),
            data_device_id: "fixed".into(),
            hash_device_id: "grow".into(),
            ..Default::default()
        });
        storage.filesystems[0].device_id = Some("root-verity".into());
        let mut hash_image = fixture().1.remove(0).image_file;
        hash_image.uncompressed_size = 25 * MIB;
        image[0].verity = Some(OsImageVerityHash {
            roothash: "unused".into(),
            hash_image_file: hash_image,
        });
        image[0].image_file.uncompressed_size = 0;
        assert_eq!(from_filesystems(&storage, &image).unwrap()["os"], 127 * MIB);
    }

    #[test]
    fn encrypted_grow_minima_include_headers_but_still_require_payload_sizes() {
        let (mut storage, mut image) = fixture();
        storage.encryption = Some(Encryption {
            volumes: vec![EncryptedVolume {
                id: "root-crypt".into(),
                device_id: "grow".into(),
                ..Default::default()
            }],
            ..Default::default()
        });
        storage.filesystems[0].device_id = Some("root-crypt".into());
        assert_eq!(from_filesystems(&storage, &image).unwrap()["os"], 318 * MIB);
        image[0].image_file.uncompressed_size = 0;
        assert!(from_filesystems(&storage, &image)
            .unwrap_err()
            .to_string()
            .contains("uncompressed size"));
        image[0].image_file.uncompressed_size = u64::MAX;
        assert!(from_filesystems(&storage, &image)
            .unwrap_err()
            .to_string()
            .contains("overflow"));
    }

    #[test]
    fn missing_payload_sizes_are_rejected() {
        let (storage, mut image) = fixture();
        image[0].image_file.uncompressed_size = 0;
        assert!(from_filesystems(&storage, &image)
            .unwrap_err()
            .to_string()
            .contains("uncompressed size"));
    }

    #[test]
    fn unknown_new_filesystem_grow_minima_are_rejected() {
        let (mut storage, image) = fixture();
        storage.filesystems[0].source = FileSystemSource::New(NewFileSystemType::Ext4);
        assert!(from_filesystems(&storage, &image)
            .unwrap_err()
            .to_string()
            .contains("fixed partition size"));
    }

    #[test]
    fn only_supplied_disks_require_capacity_estimates() {
        let (mut storage, _) = fixture();
        storage.disks[0].partitions[1].size = (100 * MIB).into();
        storage.disks.push(Disk {
            id: "explicit".into(),
            device: "/dev/sdb".into(),
            partitions: vec![Partition::new("new", PartitionSize::Grow)],
            ..Default::default()
        });
        storage.filesystems.push(FileSystem {
            device_id: Some("new".into()),
            source: FileSystemSource::New(NewFileSystemType::Ext4),
            mount_point: Some(MountPoint {
                path: "/var".into(),
                options: Default::default(),
            }),
            is_esp: false,
        });
        assert_eq!(
            requirements(
                &storage,
                &OsImage::mock(MockOsImage::new()),
                &storage.build_graph().unwrap(),
                storage.disks.iter().take(1),
            )
            .unwrap(),
            HashMap::from([("os".into(), 202 * MIB)])
        );
    }
}
