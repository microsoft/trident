use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::Read,
    path::Path,
    time::Duration,
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, info, trace, warn};
use reqwest::Url;
use stream_image::{exponential_backoff_get, GET_MAX_RETRIES, GET_TIMEOUT_SECS};

use osutils::{
    e2fsck,
    hashing_reader::{HashingReader256, HashingReader384},
    image_streamer, lsblk, resize2fs,
};
use trident_api::{
    config::{FileSystemSource, HostConfiguration, ImageFormat, ImageSha256},
    constants::{ESP_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY},
    error::{ReportError, ServicingError, TridentError},
    status::ServicingType,
    BlockDeviceId,
};

use crate::{engine::EngineContext, osimage::OsImageFile};

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
    // updated in the Host Configuration. Here, Trident will deploy images onto the A/B volume
    // pairs.
    let images_to_deploy = match ctx.servicing_type {
        ServicingType::CleanInstall => host_config.storage.get_images(),
        ServicingType::AbUpdate => host_config.storage.get_ab_volume_pair_images(),
        _ => bail!(
            "Servicing type cannot be '{:?}' as images must be deployed during clean install or A/B update",
            ctx.servicing_type
        ),
    };

    for (device_id, image) in images_to_deploy {
        // Validate that block device exists
        let block_device_path = ctx
            .get_block_device_path(&device_id)
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
                let stream = HashingReader256::new(stream);

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

/// Resizes ext2/ext3/ext4 filesystem on the given block device to the maximum
/// size of the underlying block device.
fn resize_ext_fs(block_device_path: &Path) -> Result<(), Error> {
    resize2fs::run(block_device_path).context(format!(
        "Failed to resize partition on block device at path '{}'",
        block_device_path.display()
    ))
}

#[tracing::instrument(name = "image_provision", skip_all)]
pub(super) fn provision(
    ctx: &EngineContext,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    deploy_images(ctx, host_config).structured(ServicingError::DeployImages)?;
    deploy_os_image(ctx, host_config).structured(ServicingError::DeployImages)?;

    Ok(())
}

/// Deploys all the filesystem images sourced from the OS Image to the
/// corresponding block devices.
fn deploy_os_image(ctx: &EngineContext, host_config: &HostConfiguration) -> Result<(), Error> {
    // Get the filesystems that are sourced from the OS image
    let filesystems_from_os_image = {
        let mut fs_list = Vec::new();

        for filesystem in host_config.storage.filesystems.iter() {
            if filesystem.source != FileSystemSource::OsImage {
                // Skip everything that is not sourced from the OS image.
                continue;
            }

            let device_id = filesystem.device_id.as_ref().with_context(|| {
                format!(
                    "Filesystem [{}] is sourced from an OS Image, but does not reference a block device.",
                    filesystem.description()
                )
            })?;

            let mount_point = filesystem.mount_point.as_ref().with_context(|| {
                format!(
                    "Filesystem [{}] is sourced from an OS Image, but does not have a mount point.",
                    filesystem.description(),
                )
            })?;

            if mount_point.path == Path::new(ESP_MOUNT_POINT_PATH) {
                debug!(
                    "Skipping deployment of filesystem [{}] sourced from OS Image, as it is the ESP.",
                    filesystem.description()
                );
                continue;
            }

            // If we're executing A/B update, Trident will only re-deploy images onto the A/B
            // volume pairs
            if ctx.servicing_type == ServicingType::AbUpdate
                && !ctx
                    .storage_graph
                    .has_ab_capabilities(device_id)
                    .with_context(|| {
                        format!("Failed to find device '{device_id}' in storage graph")
                    })?
            {
                debug!(
                    "Skipping deployment of filesystem [{}] sourced from OS Image, as it is not part of an A/B volume pair.",
                    filesystem.description()
                );
                continue;
            }

            fs_list.push((device_id, mount_point, filesystem.fs_type));
        }

        fs_list
    };

    // If there are no filesystems sourced from the OS image, return early
    if filesystems_from_os_image.is_empty() {
        if ctx.image.is_none() {
            // We don't have any filesystems sourced from the OS image nor an OS
            // image, this is fine. This most likely means that the host
            // configuration is using the old images API.
            return Ok(());
        } else {
            bail!("OS image is available, but no filesystems are sourced from it.");
        }
    }

    // If we have filesystems sourced from the OS image, ensure that the OS
    // image is available.
    let os_img = ctx.image.as_ref().context("OS image is not available")?;

    // TODO: MOVE THIS TO THE VALIDATE FUNCTION (#9826)
    // Get the available mount points
    let available_mount_points = os_img.available_mount_points().collect::<HashSet<_>>();

    // Iterate over the filesystems sourced from the OS image and ensure that the
    // mount points are available
    for (device_id, mp, _) in filesystems_from_os_image.iter() {
        if !available_mount_points.contains(mp.path.as_path()) {
            bail!(
                "Mount point '{}' for device '{}' is not available in the OS image",
                mp.path.display(),
                device_id
            );
        }
    }

    let images = os_img
        .filesystems()
        .map(|fs| (fs.mount_point.to_owned(), fs))
        .collect::<HashMap<_, _>>();

    // Now, deploy the filesystems sourced from the OS image
    for (id, mp, fs_type) in filesystems_from_os_image {
        let image = images.get(&mp.path).context(format!(
            "Internal error: No image found for mount point '{}' in the OS image",
            mp.path.display()
        ))?;

        // Check if this ID is a verity device, if so, we must explore the graph
        // to obtain the underlying devices.
        if let Some(verity_device) = ctx.spec.storage.verity_device(id) {
            let Some(image_file_verity) = image.verity.as_ref() else {
                bail!(
                    "Attempt to deploy a filesystem image sourced from the OS image to a verity \
                    device, but no verity hash is available in the OS image."
                )
            };

            // Deploy the data image to the underlying data device.
            info!(
                "Initializing '{}': writing image for filesystem at '{}' from '{}'",
                verity_device.data_device_id,
                image.mount_point.display(),
                os_img.source()
            );
            deploy_os_image_file(
                ctx,
                &verity_device.data_device_id,
                &image.image_file,
                FileSystemResize::NoResize,
            )?;

            info!(
                "Initializing '{}': writing verity hash image for filesystem at '{}' from '{}'",
                verity_device.hash_device_id,
                image.mount_point.display(),
                os_img.source()
            );
            deploy_os_image_file(
                ctx,
                &verity_device.hash_device_id,
                &image_file_verity.hash_image_file,
                FileSystemResize::NoResize,
            )?;
        } else {
            // For non-verity devices, we can deploy the image directly.
            info!(
                "Initializing '{id}': writing image for filesystem at '{}' from '{}'",
                image.mount_point.display(),
                os_img.source()
            );

            // Determine if/how the filesystem should be resized.
            let resize = if mp.options.contains(MOUNT_OPTION_READ_ONLY) {
                FileSystemResize::NoResize
            } else if fs_type.is_ext() {
                FileSystemResize::Ext
            } else {
                FileSystemResize::NoResize
            };

            deploy_os_image_file(ctx, id, &image.image_file, resize)?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileSystemResize {
    NoResize,
    Ext,
}

/// Deploys an individual OS image file from an OS image.
fn deploy_os_image_file(
    ctx: &EngineContext,
    id: &BlockDeviceId,
    image_file: &OsImageFile,
    fs_resize: FileSystemResize,
) -> Result<(), Error> {
    let block_device_path = ctx
        .get_block_device_path(id)
        .context(format!("No block device with id '{}' found", id))?;

    let dev_info = lsblk::get(&block_device_path).with_context(|| {
        format!(
            "Failed to get block device information for '{id}' at '{}'",
            block_device_path.display()
        )
    })?;

    ensure!(
        dev_info.size >= image_file.uncompressed_size,
        "Block device is too small, expected at least {} bytes, got {} bytes",
        image_file.uncompressed_size,
        dev_info.size
    );

    let stream = HashingReader384::new(
        image_file
            .reader()
            .context("Failed to create reader for filesystem image file")?,
    );

    let computed_sha384 =
        image_streamer::stream_zstd(stream, &block_device_path).context(format!(
            "Failed to stream image to block device '{id}' at '{}'",
            block_device_path.display()
        ))?;

    trace!("Deployed image with hash {computed_sha384}");

    // Ensure computed SHA384 matches SHA384 in OS image
    if image_file.sha384 != computed_sha384 {
        bail!(
            "SHA384 mismatch for OS image: expected {}, got {}",
            image_file.sha384,
            computed_sha384
        )
    }

    match fs_resize {
        // Resize an ext* filesystem
        FileSystemResize::Ext => {
            // TODO investigate if we stop doing the check, tracked by https://dev.azure.com/mariner-org/ECF/_workitems/edit/7218
            debug!("Checking filesystem on block device '{id}'");
            e2fsck::fix(&block_device_path)
                .context(format!("Failed to check filesystem on block device '{id}'"))?;
            debug!("Resizing filesystem on block device '{id}'");
            resize_ext_fs(&block_device_path).context(format!(
                "Failed to resize filesystem on block device '{id}'",
            ))?;
        }

        // No resizing needed
        FileSystemResize::NoResize => {}
    }

    Ok(())
}
