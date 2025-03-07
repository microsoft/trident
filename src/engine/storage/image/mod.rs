use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, info, trace, warn};

use osutils::{e2fsck, hashing_reader::HashingReader384, image_streamer, lsblk, resize2fs};
use trident_api::{
    config::{FileSystemSource, HostConfiguration},
    constants::{ESP_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY},
    error::{ReportError, ServicingError, TridentError},
    status::ServicingType,
    BlockDeviceId,
};

use crate::{engine::EngineContext, osimage::OsImageFile};

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
