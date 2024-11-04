use std::{
    collections::HashMap,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Error};
use log::{debug, info, warn};
use reqwest::Url;
use stream_image::{exponential_backoff_get, GET_MAX_RETRIES, GET_TIMEOUT_SECS};

use osutils::{
    container, e2fsck, hashing_reader::HashingReader, image_streamer, resize2fs, veritysetup,
};
use trident_api::{
    config::{
        AbUpdate, HostConfiguration, HostConfigurationDynamicValidationError, Image, ImageFormat,
        ImageSha256,
    },
    constants::{MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    error::{InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{AbVolumeSelection, ServicingType},
    BlockDeviceId,
};

use crate::engine::{self, storage::tabfile, EngineContext};

pub(crate) mod stream_image;
#[cfg(feature = "sysupdate")]
mod systemd_sysupdate;

/// Deploys images onto block devices that are not ESP partitions, as ESP image deployments are
/// handled separately by the boot subsystem.
///
/// Depending on the image format, Trident will use different strategies to deploy the image:
/// 1. If image is a local file or an HTTP file published in RawZstd format, Trident will use a
///    HashingReader to write the bytes to the target block device.
/// 2. If image is a local file or an HTTP file published in RawLzma format, Trident will run
///    systemd-sysupdate.rs to download the image, if needed, and write it to the block device. The
///    block device has to be a part of an A/B volume pair backed by partition block device. This is
///    b/c sysupdate can only operate if there are 2+ copies of the partition type as the partition
///    to be updated.
/// 3. TODO: If image is an HTTP file published as an OCI Artifact, ImageFormat OciArtifact,
///    Trident will download the image from Azure container registry and pass it to
///    systemd-sysupdate.rs. ADO task: https://dev.azure.com/mariner-org/ECF/_workitems/edit/5503/.
///
/// This function is called by the provision() function in the image subsystem and returns an error
/// if the image cannot be downloaded or deployed correctly.
fn deploy_images(ctx: &EngineContext, host_config: &HostConfiguration) -> Result<(), Error> {
    // 1. During clean install, Trident will deploy images onto all non-ESP block devices here.
    // This includes A/B volume pairs and other standalone block devices that are not ESP.
    // 2. During A/B update, Trident will assume that all A/B volume pair and ESP images have been
    // updated in the host configuration. Here, Trident will deploy images onto the A/B volume
    // pairs.
    let images_to_deploy = match ctx.servicing_type {
        ServicingType::CleanInstall => host_config.storage.get_images(),
        ServicingType::AbUpdate => host_config.storage.get_ab_volume_pair_images(),
        _ => bail!(
            "Servicing type cannot be '{:?}' as images must deployed during clean install or A/B update",
            ctx.servicing_type
        ),
    };

    for (device_id, image) in images_to_deploy {
        // Validate that block device exists
        let block_device_path = engine::get_block_device_path(ctx, &device_id)
            .context(format!("No block device with id '{}' found", device_id))?;

        // Parse the URL to determine the download strategy
        let image_url =
            Url::parse(&image.url).context(format!("Failed to parse image URL '{}'", image.url))?;

        let stream: Box<dyn Read> = match image_url.scheme() {
            "file" => Box::new(
                File::open(image_url.path())
                    .with_context(|| format!("Failed to open '{}'", image_url))?,
            ),
            "http" | "https" => {
                // For remote files, perform a blocking GET request
                exponential_backoff_get(
                    &image_url,
                    GET_MAX_RETRIES,
                    Duration::from_secs(GET_TIMEOUT_SECS),
                )
                .context(format!(
                    "Failed to fetch image for device id '{}'",
                    device_id
                ))?
            }
            "oci" => {
                // TODO: Need to implement downloading images as OCI artifacts from Azure container
                // registry and passing them to sysupdate. This functionality will be implemented in
                // download_oci.rs. After the artifact is downloaded locally, Trident will evoke
                // systemd-sysupdate.rs to install the image, providing 2 extra arg-s:
                // 1. local_update_dir, which is a TempDir object pointing to a local directory
                // containing the update image,
                // 2. local_update_file, which is a String representing the name of the image file
                // downloaded by Trident so that sysupdate can operate on it.
                //
                // Related ADO task:
                // https://dev.azure.com/mariner-org/ECF/_workitems/edit/5503/.
                bail!("Downloading images as OCI artifacts from Azure container registry is not implemented")
            }
            _ => bail!("Unsupported URL scheme"),
        };

        match image.format {
            ImageFormat::RawZst => {
                info!(
                    "Initializing '{device_id}': writing image from '{}'",
                    image.url
                );

                // Initialize HashingReader instance on stream
                let stream = HashingReader::new(stream);

                let computed_sha256 = image_streamer::stream_zstd(stream, &block_device_path)?;

                // If SHA256 is ignored, log message and skip hash validation; otherwise, ensure computed
                // SHA256 matches SHA256 in HostConfig
                match image.sha256 {
                    ImageSha256::Ignored => {
                        warn!("Ignoring SHA256 for image from '{}'", image_url);
                    }
                    ImageSha256::Checksum(ref expected_sha256) => {
                        if computed_sha256 != expected_sha256.as_str() {
                            bail!(
                                "SHA256 mismatch for disk image {}: expected {}, got {}",
                                image_url,
                                expected_sha256,
                                computed_sha256
                            );
                        }
                    }
                }

                // If the image has ext* filesystem and is not to be mounted read-only,
                // resize the filesystem. For now, we determine the filesystem by looking at
                // the corresponding mountpoint.
                let mount_point = ctx
                    .spec
                    .storage
                    .internal_mount_points
                    .iter()
                    .find(|mp| mp.target_id == device_id);
                if let Some(mount_point) = mount_point {
                    if mount_point.filesystem.is_ext()
                        && !mount_point.options.contains(&MOUNT_OPTION_READ_ONLY.into())
                    {
                        // TODO investigate if we stop doing the check, tracked by https://dev.azure.com/mariner-org/ECF/_workitems/edit/7218
                        debug!("Checking filesystem on block device '{}'", &device_id);
                        e2fsck::fix(&block_device_path).context(format!(
                            "Failed to check filesystem on block device '{}'",
                            &device_id
                        ))?;
                        debug!("Resizing filesystem on block device '{}'", &device_id);
                        resize_ext_fs(&block_device_path).context(format!(
                            "Failed to resize filesystem on block device '{}'",
                            &device_id
                        ))?;
                    }
                }
            }
            #[cfg(feature = "sysupdate")]
            ImageFormat::RawLzma => {
                if image_url.scheme() == "file" {
                    // Fetch directory and filename from image URL
                    let (directory, filename, _computed_sha256) =
                        systemd_sysupdate::get_local_image(&image_url, &image)?;
                    // Run helper func systemd_sysupdate::deploy() to execute A/B update; since image is
                    // local, pass directory and file name as arg-s
                    systemd_sysupdate::deploy(
                        &image,
                        &device_id,
                        ctx,
                        Some(&directory),
                        Some(&filename),
                    )
                    .context(format!(
                        "Failed to deploy image {} via sysupdate",
                        image.url
                    ))?;
                } else if image_url.scheme() == "http" || image_url.scheme() == "https" {
                    // If image is of format RawLzma AND target-id corresponds to an A/B volume pair,
                    // use systemd-sysupdate.rs to write to the partition.
                    //
                    // TODO: Instead of delegating the download of the payload and hash verification to
                    // systemd-sysupdate, do it from Trident, to support more format types and avoid
                    // the SHA256SUMS overhead for the user. Related ADO task:
                    // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6175.

                    // Determine if target-id corresponds to an A/B volume pair; if helper
                    // func returns None, then set bool to false
                    let targets_ab_volume_pair =
                        ctx.get_ab_update_volume_partition(&device_id).is_some();

                    // If image is of format RawLzma but target-id does NOT
                    // correspond to an A/B volume pair, report an error
                    if !targets_ab_volume_pair {
                        bail!("Block device with id {} is not part of an A/B volume pair, so image in raw lzma format cannot be written to it.\nRaw lzma is not supported for block devices that do not correspond to A/B volume pairs",
                                    &device_id)
                    }
                    // Run helper func systemd_sysupdate::deploy() to execute A/B update; directory and file
                    // name arg-s are None to communicate that update image is published via URL,
                    // not locally
                    systemd_sysupdate::deploy(&image, &device_id, ctx, None, None).context(
                        format!("Failed to deploy image {} via sysupdate", image.url),
                    )?;
                } else {
                    bail!("Unsupported URL scheme")
                };
            }
        }
    }

    Ok(())
}

/// Resize ext2/ext3/ext4 filesystem on the given block device to the maximum
/// size of the underlying block device
fn resize_ext_fs(block_device_path: &Path) -> Result<(), Error> {
    resize2fs::run(block_device_path).context(format!(
        "Failed to resize partition on block device at path '{}'",
        block_device_path.display()
    ))
}

pub(crate) fn get_root_device_path() -> Result<PathBuf, Error> {
    let root_mount_path = if container::is_running_in_container()
        .unstructured("Failed to determine wheter running in a container")?
    {
        container::get_host_root_path().unstructured("Failed to get host root mount path")?
    } else {
        Path::new(ROOT_MOUNT_POINT_PATH).to_path_buf()
    };
    let path = tabfile::get_device_path(Path::new("/proc/mounts"), &root_mount_path)
        .context("Failed to find root mount point")?;
    debug!("Using root device path: {}", path.display());
    Ok(path)
}

pub(crate) fn update_active_volume(
    ctx: &mut EngineContext,
    root_device_path: PathBuf,
) -> Result<(), Error> {
    let ab_update = &ctx
        .spec
        .storage
        .ab_update
        .as_ref()
        .context("No A/B update found")?;

    let root_device_id = ctx
        .spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
        .map(|m| &m.target_id)
        .context("No mount point for root volume found")?;
    debug!("Root device id: {:?}", root_device_id);

    let root_is_verity = ctx
        .spec
        .storage
        .verity_filesystems
        .iter()
        .any(|fs| fs.mount_point.path == Path::new(ROOT_MOUNT_POINT_PATH));

    let ((volume_a_path, volume_b_path), root_device_path) = if root_is_verity {
        debug!("Root is a verity device");
        get_verity_data_volume_pair_paths(ctx, ab_update, root_device_id)
            .context("Failed to find root verity data volume pair")?
    } else {
        debug!("Root is not on verity");
        get_plain_volume_pair_paths(ctx, ab_update, root_device_id, root_device_path)
            .context("Failed to find root volume pair")?
    };

    // TODO: better error handling if canonicalize fails, tracked by https://dev.azure.com/mariner-org/ECF/_workitems/edit/7320/
    ctx.ab_active_volume = if volume_a_path
        .canonicalize()
        .context(format!("Failed to find path '{}'", volume_a_path.display()))?
        == root_device_path
    {
        debug!("Active volume is A");
        Some(AbVolumeSelection::VolumeA)
    } else if volume_b_path
        .canonicalize()
        .context(format!("Failed to find path '{}'", volume_a_path.display()))?
        == root_device_path
    {
        debug!("Active volume is B");
        Some(AbVolumeSelection::VolumeB)
    } else {
        debug!("Unrecognized active volume");
        // To prevent data loss, abort if we cannot find the
        // matching root volume outside of clean install
        if ctx.servicing_type != ServicingType::CleanInstall {
            bail!("No matching root volume found");
        }
        None
    };

    Ok(())
}

fn get_plain_volume_pair_paths(
    ctx: &EngineContext,
    ab_update: &AbUpdate,
    root_device_id: &BlockDeviceId,
    root_device_path: PathBuf,
) -> Result<((PathBuf, PathBuf), PathBuf), Error> {
    let root_device_pair = ab_update
        .volume_pairs
        .iter()
        .find(|p| &p.id == root_device_id)
        .context("No volume pair for root volume found")?;
    debug!("Root device pair: {:?}", root_device_pair);

    let volume_a_path =
        engine::get_block_device_path(ctx, &root_device_pair.volume_a_id).context(format!(
            "Failed to get block device path for volume A with ID {}",
            root_device_pair.volume_a_id
        ))?;
    let volume_b_path =
        engine::get_block_device_path(ctx, &root_device_pair.volume_b_id).context(format!(
            "Failed to get block device path for volume B with ID {}",
            root_device_pair.volume_b_id
        ))?;

    Ok((
        (volume_a_path.clone(), volume_b_path.clone()),
        root_device_path,
    ))
}

fn get_verity_data_volume_pair_paths(
    ctx: &EngineContext,
    ab_update: &AbUpdate,
    root_device_id: &BlockDeviceId,
) -> Result<((PathBuf, PathBuf), PathBuf), Error> {
    let root_verity_device_config = ctx
        .spec
        .storage
        .internal_verity
        .iter()
        .find(|vd| &vd.id == root_device_id)
        .context("Failed to find root verity device config")?;
    let root_data_device_pair = ab_update
        .volume_pairs
        .iter()
        .find(|vp| vp.id == root_verity_device_config.data_target_id)
        .context("No volume pair for root data device found")?;
    let volume_a_path = engine::get_block_device_path(ctx, &root_data_device_pair.volume_a_id)
        .context("Failed to get block device for data volume A")?;
    let volume_b_path = engine::get_block_device_path(ctx, &root_data_device_pair.volume_b_id)
        .context("Failed to get block device for data volume B")?;
    let root_verity_status = veritysetup::status(&root_verity_device_config.device_name)
        .context("Failed to get verity status")?;

    Ok((
        (volume_a_path, volume_b_path),
        root_verity_status.data_device_path,
    ))
}

/// Checks if the host needs an A/B update by comparing the images targeting ESP partitions and A/B
/// volume pairs in the host configuration with those in the engine context. Returns true if there is
/// at least one image that needs to be updated; otherwise, returns false.
pub(super) fn needs_ab_update(ctx: &EngineContext) -> bool {
    let old_images = ctx
        .spec_old
        .storage
        .get_ab_volume_pair_images()
        .into_iter()
        .chain(ctx.spec_old.storage.get_esp_images())
        .collect();
    let new_images = ctx
        .spec
        .storage
        .get_ab_volume_pair_images()
        .into_iter()
        .chain(ctx.spec.storage.get_esp_images())
        .collect();
    !get_updated_images(old_images, new_images).is_empty()
}

/// Returns the images that need to be updated.
///
/// The images are compared between the old images and the new images. If an image is found in the
/// new images that is not in the old images, or if the image is found in both but the URL or SHA256
/// checksum has changed, the image is added to the list of images to be updated.
pub(crate) fn get_updated_images(
    old_images: Vec<(BlockDeviceId, Image)>,
    mut new_images: Vec<(BlockDeviceId, Image)>,
) -> Vec<(BlockDeviceId, Image)> {
    let old_images: HashMap<String, Image> = old_images.into_iter().collect();
    new_images.retain(|(device_id, image)| {
        if let Some(old_image) = old_images.get(device_id) {
            old_image.url != image.url || old_image.sha256 != image.sha256
        } else {
            true
        }
    });
    new_images
}

/// Validates that the host configuration requests deployment only of those images that can be
/// deployed, for the specific servicing type.
///
/// Currently, this function is called only during A/B update, to ensure that the host
/// configuration does not request Trident to re-deploy images on standalone volumes that are
/// shared between A and B, such as /var/lib/trident.
pub(super) fn validate_host_config(
    ctx: &EngineContext,
    host_config: &HostConfiguration,
    planned_servicing_type: ServicingType,
) -> Result<(), TridentError> {
    if planned_servicing_type == ServicingType::AbUpdate {
        // Get lists of all old and new images in the host configuration
        let old_images = ctx
            .spec
            .storage
            .get_images()
            .into_iter()
            .chain(ctx.spec.storage.get_esp_images())
            .collect();
        let new_images = host_config
            .storage
            .get_images()
            .into_iter()
            .chain(host_config.storage.get_esp_images())
            .collect();

        // Filter only those images that have been updated, compared to the engine context
        let updated_images = get_updated_images(old_images, new_images);

        // Iterate over the updated images and ensure that they only target A/B volume pairs or ESP
        // partitions. If an image targets a standalone block device, return an error.
        for (device_id, _) in updated_images {
            // Get lists of ESP images and A/B volume pair images
            let esp_images = host_config.storage.get_esp_images();
            let ab_volume_pair_images = host_config.storage.get_ab_volume_pair_images();

            // If the device ID is not found in the list of ESP images or A/B volume pair images, return
            // an error
            if !esp_images.iter().any(|(id, _)| id == &device_id)
                && !ab_volume_pair_images.iter().any(|(id, _)| id == &device_id)
            {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::ImageUpdateOnStandaloneBlockDevice {
                        device_id: device_id.clone(),
                    },
                )));
            }
        }
    }

    Ok(())
}

#[tracing::instrument(name = "image_provision", skip_all)]
pub(super) fn provision(
    ctx: &EngineContext,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    deploy_images(ctx, host_config).structured(ServicingError::DeployImages)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use maplit::btreemap;

    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, FileSystemType, Image,
            ImageSha256, MountOptions, MountPoint, Partition, PartitionType,
            Storage as StorageConfig,
        },
        status::ServicingType,
    };

    /// Validates that the logic in validate_host_config() is correct.
    #[test]
    fn test_validate_host_config_image() {
        // Initialize a engine context
        let ctx = EngineContext {
            servicing_type: ServicingType::NoActiveServicing,
            spec: HostConfiguration {
                storage: StorageConfig {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "esp".to_string(),
                                partition_type: PartitionType::Esp,
                                size: 100.into(),
                            },
                            Partition {
                                id: "root-a".to_string(),
                                partition_type: PartitionType::Root,
                                size: 100.into(),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                partition_type: PartitionType::Root,
                                size: 100.into(),
                            },
                            Partition {
                                id: "trident".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 100.into(),
                            },
                        ],
                        ..Default::default()
                    }],
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("esp".into()),
                            fs_type: FileSystemType::Vfat,
                            source: FileSystemSource::EspImage(Image {
                                url: "http://example.com/esp_2.img".to_string(),
                                sha256: ImageSha256::Checksum("esp_sha256_2".into()),
                                format: ImageFormat::RawZst,
                            }),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/esp"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("root".into()),
                            fs_type: FileSystemType::Vfat,
                            source: FileSystemSource::Image(Image {
                                url: "http://example.com/root_2.img".to_string(),
                                sha256: ImageSha256::Checksum("root_sha256_2".into()),
                                format: ImageFormat::RawZst,
                            }),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/"),
                                options: MountOptions::empty(),
                            }),
                        },
                        FileSystem {
                            device_id: Some("trident".into()),
                            fs_type: FileSystemType::Vfat,
                            source: FileSystemSource::Image(Image {
                                url: "http://example.com/trident_1.img".to_string(),
                                sha256: ImageSha256::Checksum("trident_sha256_1".into()),
                                format: ImageFormat::RawZst,
                            }),
                            mount_point: None,
                        },
                    ],
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "root".to_string(),
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "esp".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root-a".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "root-b".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "trident".into() => PathBuf::from("/dev/disk/by-partlabel/osp4"),
            },
            ..Default::default()
        };

        // Initialize a corresponding host configuration
        let mut host_config = ctx.spec.clone();
        if let FileSystemSource::Image(Image { ref mut sha256, .. }) =
            host_config.storage.filesystems[0].source
        {
            *sha256 = ImageSha256::Checksum("new_sha256".into());
        }
        if let FileSystemSource::Image(Image { ref mut sha256, .. }) =
            host_config.storage.filesystems[1].source
        {
            *sha256 = ImageSha256::Checksum("new_sha256".into());
        }

        // Test case 0. Running validate_host_config() when the planned servicing type is
        // CleanInstall should always return ((Ok)) since there is no validation logic.
        validate_host_config(&ctx, &host_config, ServicingType::CleanInstall).unwrap();

        // Test case 1. Running validate_host_config() when only update of the ESP partition and
        // A/B volume pair images is requested during A/B update should return ((Ok)).
        // Update servicing state to Provisioned for consistency.
        validate_host_config(&ctx, &host_config, ServicingType::AbUpdate).unwrap();

        // Test case 2. Running validate_host_config() when update of a standalone volume 'trident'
        // is requested during A/B update should return an error.
        // Update URL and sha256sum of 'trident' image in host configuration.
        host_config.storage.filesystems[2].source = FileSystemSource::Image(Image {
            url: "http://example.com/trident_2.img".to_string(),
            sha256: ImageSha256::Checksum("trident_sha256_2".into()),
            format: ImageFormat::RawZst,
        });
        validate_host_config(&ctx, &host_config, ServicingType::AbUpdate).unwrap_err();
    }

    /// Validates that the logic in needs_ab_update() and get_updated_images() is correct.
    #[test]
    fn test_needs_ab_update_and_get_updated_images() {
        // Initialize a host configuration
        let host_config = HostConfiguration {
            storage: StorageConfig {
                disks: vec![Disk {
                    id: "os".to_owned(),
                    device: PathBuf::from("/dev/disk/by-bus/foobar"),
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 100.into(),
                        },
                        Partition {
                            id: "root-a".to_string(),
                            partition_type: PartitionType::Root,
                            size: 100.into(),
                        },
                        Partition {
                            id: "root-b".to_string(),
                            partition_type: PartitionType::Root,
                            size: 100.into(),
                        },
                        Partition {
                            id: "trident".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 100.into(),
                        },
                    ],
                    ..Default::default()
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        fs_type: FileSystemType::Vfat,
                        source: FileSystemSource::EspImage(Image {
                            url: "http://example.com/esp_1.img".to_string(),
                            sha256: ImageSha256::Checksum("esp_sha256_1".into()),
                            format: ImageFormat::RawZst,
                        }),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/esp"),
                            options: MountOptions::empty(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        fs_type: FileSystemType::Vfat,
                        source: FileSystemSource::Image(Image {
                            url: "http://example.com/root_1.img".to_string(),
                            sha256: ImageSha256::Checksum("root_sha256_1".into()),
                            format: ImageFormat::RawZst,
                        }),
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/"),
                            options: MountOptions::empty(),
                        }),
                    },
                    FileSystem {
                        device_id: Some("trident".into()),
                        fs_type: FileSystemType::Vfat,
                        source: FileSystemSource::Image(Image {
                            url: "http://example.com/trident_1.img".to_string(),
                            sha256: ImageSha256::Checksum("trident_sha256_1".into()),
                            format: ImageFormat::RawZst,
                        }),
                        mount_point: None,
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![AbVolumePair {
                        id: "root".to_string(),
                        volume_a_id: "root-a".to_string(),
                        volume_b_id: "root-b".to_string(),
                    }],
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        // Initialize a engine context with spec matching the host configuration
        let mut ctx = EngineContext {
            servicing_type: ServicingType::NoActiveServicing,
            spec: host_config.clone(),
            spec_old: host_config,
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "esp".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root-a".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "root-b".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "trident".into() => PathBuf::from("/dev/disk/by-partlabel/osp4"),
            },
            // Set active volume to A
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            ..Default::default()
        };

        // Test case 1. Running needs_ab_update() when images are the same in engine context and host
        // configuration should return false.
        assert!(!needs_ab_update(&ctx));
        // Running get_updated_images() should return an empty list.
        assert!(get_updated_images(
            ctx.spec_old.storage.get_images(),
            ctx.spec.storage.get_images()
        )
        .is_empty());

        // Test case 2. Running needs_ab_update() when the URL of the ESP image in the host
        // configuration is different from that in the engine context should return true.
        ctx.spec.storage.filesystems[0].source = FileSystemSource::EspImage(Image {
            url: "http://example.com/esp_2.img".to_string(),
            sha256: ImageSha256::Checksum("esp_sha256_1".into()),
            format: ImageFormat::RawZst,
        });
        assert!(needs_ab_update(&ctx));
        // Running get_updated_images() should return the 'esp' image.
        assert_eq!(
            get_updated_images(
                ctx.spec_old.storage.get_esp_images(),
                ctx.spec.storage.get_esp_images()
            ),
            vec![(
                "esp".to_string(),
                Image {
                    url: "http://example.com/esp_2.img".to_string(),
                    sha256: ImageSha256::Checksum("esp_sha256_1".into()),
                    format: ImageFormat::RawZst,
                }
            )]
        );

        // Test case 3. Running needs_ab_update() when the sha256sum of the ESP image in the host
        // configuration is different from that in the engine context should return true.
        ctx.spec.storage.filesystems[0].source = FileSystemSource::EspImage(Image {
            url: "http://example.com/esp_1.img".to_string(),
            sha256: ImageSha256::Checksum("esp_sha256_2".into()),
            format: ImageFormat::RawZst,
        });
        assert!(needs_ab_update(&ctx));
        // Running get_updated_images() for ESP only should return the 'esp' image.
        assert_eq!(
            get_updated_images(
                ctx.spec_old.storage.get_esp_images(),
                ctx.spec.storage.get_esp_images()
            ),
            vec![(
                "esp".to_string(),
                Image {
                    url: "http://example.com/esp_1.img".to_string(),
                    sha256: ImageSha256::Checksum("esp_sha256_2".into()),
                    format: ImageFormat::RawZst,
                }
            )]
        );
        // But running get_updated_images() for all non-ESP images should return an empty list.
        assert!(get_updated_images(
            ctx.spec_old.storage.get_images(),
            ctx.spec.storage.get_images()
        )
        .is_empty());

        // Test case 4. Running needs_ab_update() when the URL of the root image in the host
        // configuration is different from that in the engine context should return true.
        ctx.spec.storage.filesystems[1].source = FileSystemSource::Image(Image {
            url: "http://example.com/root_2.img".to_string(),
            sha256: ImageSha256::Checksum("root_sha256_2".into()),
            format: ImageFormat::RawZst,
        });
        assert!(needs_ab_update(&ctx));
        // Running get_updated_images() for all non-ESP images should return the 'root' image.
        assert_eq!(
            get_updated_images(
                ctx.spec_old.storage.get_images(),
                ctx.spec.storage.get_images()
            ),
            vec![(
                "root".to_string(),
                Image {
                    url: "http://example.com/root_2.img".to_string(),
                    sha256: ImageSha256::Checksum("root_sha256_2".into()),
                    format: ImageFormat::RawZst,
                }
            )]
        );
        // But running get_updated_images() for all images should return both the 'esp' and 'root'
        // images.
        let mut all_images = ctx.spec.storage.get_images();
        all_images.extend(ctx.spec.storage.get_esp_images());
        // Assert length of the returned list
        assert_eq!(
            get_updated_images(ctx.spec_old.storage.get_images(), all_images.clone()).len(),
            2
        );
        // Assert it contains both the 'esp' and 'root' images
        assert!(
            get_updated_images(ctx.spec_old.storage.get_images(), all_images.clone()).contains(&(
                "esp".to_string(),
                Image {
                    url: "http://example.com/esp_1.img".to_string(),
                    sha256: ImageSha256::Checksum("esp_sha256_2".into()),
                    format: ImageFormat::RawZst,
                }
            ))
        );
        assert!(
            get_updated_images(ctx.spec_old.storage.get_images(), all_images).contains(&(
                "root".to_string(),
                Image {
                    url: "http://example.com/root_2.img".to_string(),
                    sha256: ImageSha256::Checksum("root_sha256_2".into()),
                    format: ImageFormat::RawZst,
                }
            ))
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::path::PathBuf;

    use const_format::formatcp;
    use maplit::btreemap;

    use osutils::testutils::{
        repart::{OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH},
        verity::{self, VerityGuard},
    };
    use pytest_gen::functional_test;
    use trident_api::config::{
        self, AbVolumePair, Disk, FileSystemType, InternalMountPoint, MountOptions, MountPoint,
        Partition, PartitionType, VerityFileSystem,
    };

    #[functional_test]
    fn test_get_root_device_path() {
        assert_eq!(
            get_root_device_path().unwrap().to_str().unwrap(),
            "/dev/sda2"
        );
    }

    #[functional_test]
    fn test_get_plain_volume_pair_paths() {
        let mut ctx = EngineContext {
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            spec: HostConfiguration {
                storage: config::Storage {
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "root".to_string(),
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
                PathBuf::from("/dev/sda")
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Failed to get block device path for volume A with ID root-a"
        );

        ctx.spec.storage.disks = vec![Disk {
            id: "os".to_owned(),
            device: PathBuf::from("/dev/sda"),
            partition_table_type: config::PartitionTableType::Gpt,
            adopted_partitions: vec![],
            partitions: vec![Partition {
                id: "root-a".to_owned(),
                partition_type: PartitionType::Root,
                size: 100.into(),
            }],
        }];
        ctx.block_device_paths
            .insert("root-a".to_string(), PathBuf::from("/dev/sda1"));

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
                PathBuf::from("/dev/sda")
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Failed to get block device path for volume B with ID root-b"
        );

        ctx.spec
            .storage
            .disks
            .iter_mut()
            .find(|d| d.id == "os")
            .unwrap()
            .partitions
            .push(Partition {
                id: "root-b".to_owned(),
                partition_type: PartitionType::Root,
                size: 100.into(),
            });
        ctx.block_device_paths
            .insert("root-b".to_string(), PathBuf::from("/dev/sda2"));

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
                PathBuf::from("/dev/sda")
            )
            .unwrap(),
            (
                (PathBuf::from("/dev/sda1"), PathBuf::from("/dev/sda2")),
                PathBuf::from("/dev/sda")
            )
        );

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
                PathBuf::from("/dev/sda1")
            )
            .unwrap(),
            (
                (PathBuf::from("/dev/sda1"), PathBuf::from("/dev/sda2")),
                PathBuf::from("/dev/sda1")
            )
        );

        assert_eq!(
            get_plain_volume_pair_paths(
                &ctx,
                ctx.spec.storage.ab_update.as_ref().unwrap(),
                &"root".to_string(),
                PathBuf::from("/dev/sda2")
            )
            .unwrap(),
            (
                (PathBuf::from("/dev/sda1"), PathBuf::from("/dev/sda2")),
                PathBuf::from("/dev/sda2")
            )
        );
    }

    #[functional_test]
    fn test_get_verity_data_volume_pair_paths() {
        let mut ab_update = AbUpdate {
            volume_pairs: vec![],
        };
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    ab_update: Some(ab_update.clone()),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root-id".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find root verity device config"
        );

        ctx.spec = HostConfiguration {
            storage: config::Storage {
                internal_verity: vec![config::InternalVerityDevice {
                    id: "root-id".to_string(),
                    device_name: "root".to_string(),
                    data_target_id: "root-data".to_string(),
                    hash_target_id: "root-hash".to_string(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root-id".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No volume pair for root data device found"
        );

        ab_update.volume_pairs = vec![
            AbVolumePair {
                id: "root-data".to_string(),
                volume_a_id: "root-data-a".to_string(),
                volume_b_id: "root-data-b".to_string(),
            },
            AbVolumePair {
                id: "root-hash".to_string(),
                volume_a_id: "root-hash-a".to_string(),
                volume_b_id: "root-hash-b".to_string(),
            },
        ];

        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root-id".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device for data volume A"
        );

        ctx.spec.storage.disks = vec![Disk {
            id: "os".into(),
            device: PathBuf::from("/dev/sda"),
            partition_table_type: config::PartitionTableType::Gpt,
            adopted_partitions: vec![],
            partitions: vec![Partition {
                id: "root-data-a".to_owned(),
                partition_type: PartitionType::Root,
                size: 100.into(),
            }],
        }];
        ctx.block_device_paths
            .insert("root-data-a".to_string(), PathBuf::from("/dev/sda1"));

        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root-id".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device for data volume B"
        );

        ctx.spec.storage.disks[0].partitions.push(Partition {
            id: "root-data-b".to_owned(),
            partition_type: PartitionType::Root,
            size: 100.into(),
        });
        ctx.block_device_paths
            .insert("root-data-b".to_string(), PathBuf::from("/dev/sda2"));

        let _ = veritysetup::close("root");
        assert!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root-id".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string()
                .contains("stdout:\n/dev/mapper/root is inactive.\n\n")
        );

        // now try the same, against actual verity volumes
        let expected_root_hash = verity::setup_verity_volumes();

        let verity_device_path = Path::new("/dev/mapper/root");
        if verity_device_path.exists() {
            veritysetup::close("root").unwrap();
        }

        let ctx = EngineContext {
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root-data-a".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                "root-hash-a".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                "boot2".into() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1")),
                "root-data-b".into() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
                "root-hash-b".into() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}3")),
                "root".into() => PathBuf::from("/dev/mapper/root"),
            },
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            spec: HostConfiguration {
                storage: config::Storage {
                    internal_verity: vec![config::InternalVerityDevice {
                        id: "root-id".to_string(),
                        device_name: "root".to_string(),
                        data_target_id: "root-data".to_string(),
                        hash_target_id: "root-hash".to_string(),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root-id".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string()
                .contains("stdout:\n/dev/mapper/root is inactive.\n\n")
        );

        // now open the verity and we should get further
        veritysetup::open(
            formatcp!("{TEST_DISK_DEVICE_PATH}3"),
            "root",
            formatcp!("{TEST_DISK_DEVICE_PATH}2"),
            &expected_root_hash,
        )
        .unwrap();
        let _verityguard = VerityGuard {
            device_name: "root",
        };

        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root-id".to_owned()).unwrap(),
            (
                (
                    PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                    PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
                ),
                PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
            )
        );

        // confirm that B is returned as well
        ab_update.volume_pairs[0].volume_a_id = "root-data-b".to_string();
        ab_update.volume_pairs[0].volume_b_id = "root-data-a".to_string();

        assert_eq!(
            get_verity_data_volume_pair_paths(&ctx, &ab_update, &"root-id".to_owned()).unwrap(),
            (
                (
                    PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
                    PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                ),
                PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3"))
            )
        );
    }

    #[functional_test]
    fn test_update_active_volume() {
        // Missing ab_update
        let mut ctx = EngineContext {
            ..Default::default()
        };
        assert_eq!(
            update_active_volume(&mut ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No A/B update found"
        );

        // Missing root mount point
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "rootq".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        assert_eq!(
            update_active_volume(&mut ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No mount point for root volume found"
        );

        // Missing volume pair for root mount point
        ctx.spec.storage.internal_mount_points = vec![InternalMountPoint {
            target_id: "root".to_string(),
            filesystem: FileSystemType::Ext4,
            options: vec![],
            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
        }];

        assert_eq!(
            update_active_volume(&mut ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No volume pair for root volume found"
        );

        // Missing block device for volume A
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        assert_eq!(
            update_active_volume(&mut ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume A with ID root-a"
        );

        // Missing block device for volume B
        ctx.block_device_paths = btreemap! {
            "root-a".to_owned() => PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}15")),
        };

        assert_eq!(
            update_active_volume(&mut ctx, PathBuf::from("/dev/sda"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume B with ID root-b"
        );

        ctx.block_device_paths.insert(
            "root-b".to_owned(),
            PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2")),
        );

        // Volume A path cannot be resolved
        assert_eq!(
            update_active_volume(&mut ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}3")))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No such file or directory (os error 2)"
        );

        // A or B paths do not match the root volume path
        *ctx.block_device_paths.get_mut("root-a").unwrap() =
            PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1"));

        assert_eq!(
            update_active_volume(&mut ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}3")))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No matching root volume found"
        );

        // None when clean install
        ctx.servicing_type = ServicingType::CleanInstall;

        update_active_volume(&mut ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}3"))).unwrap();
        assert_eq!(ctx.ab_active_volume, None);

        // Volume A is the root device path
        update_active_volume(&mut ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}1"))).unwrap();
        assert_eq!(ctx.ab_active_volume, Some(AbVolumeSelection::VolumeA));

        // Volume B is the root device path
        update_active_volume(&mut ctx, PathBuf::from(formatcp!("{OS_DISK_DEVICE_PATH}2"))).unwrap();
        assert_eq!(ctx.ab_active_volume, Some(AbVolumeSelection::VolumeB));

        // verity tests
        let expected_root_hash = verity::setup_verity_volumes();

        let verity_device_path = Path::new("/dev/mapper/root");
        if verity_device_path.exists() {
            veritysetup::close("root").unwrap();
        }
        veritysetup::open(
            formatcp!("{TEST_DISK_DEVICE_PATH}3"),
            "root",
            formatcp!("{TEST_DISK_DEVICE_PATH}2"),
            &expected_root_hash,
        )
        .unwrap();
        let _verityguard = VerityGuard {
            device_name: "root",
        };

        ctx.block_device_paths = btreemap! {
            "root-a".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
            "root-b".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
        };

        update_active_volume(
            &mut ctx,
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
        )
        .unwrap();
        assert_eq!(ctx.ab_active_volume, Some(AbVolumeSelection::VolumeA));

        update_active_volume(
            &mut ctx,
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
        )
        .unwrap();
        assert_eq!(ctx.ab_active_volume, Some(AbVolumeSelection::VolumeB));

        ctx.block_device_paths
            .insert("root".to_string(), PathBuf::from("/dev/mapper/root"));
        ctx.spec.storage.verity_filesystems = vec![VerityFileSystem {
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            data_image: Image {
                url: "http://example.com/root-data.img".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            hash_image: Image {
                url: "http://example.com/root-hash.img".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
            },
            fs_type: FileSystemType::Ext4,
            mount_point: MountPoint {
                path: ROOT_MOUNT_POINT_PATH.into(),
                options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
            },
        }];
        ctx.spec.storage.internal_verity = vec![config::InternalVerityDevice {
            id: "root".to_string(),
            device_name: "root".to_string(),
            data_target_id: "root-data".to_string(),
            hash_target_id: "root-hash".to_string(),
        }];
        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![
                AbVolumePair {
                    id: "root-data".to_string(),
                    volume_a_id: "root-a".to_string(),
                    volume_b_id: "root-b".to_string(),
                },
                AbVolumePair {
                    id: "root-hash".to_string(),
                    volume_a_id: "root-hash-a".to_string(),
                    volume_b_id: "root-hash-b".to_string(),
                },
            ],
        });

        update_active_volume(
            &mut ctx,
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
        )
        .unwrap();
        assert_eq!(ctx.ab_active_volume, Some(AbVolumeSelection::VolumeA));
    }
}
