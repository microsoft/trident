use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, info, trace, warn};

use osutils::{e2fsck, hashing_reader::HashingReader384, image_streamer, lsblk, resize2fs};
use trident_api::{
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
    primitives::bytes::ByteCount,
    status::ServicingType,
    BlockDeviceId,
};

use crate::{
    engine::{context::filesystem::FileSystemDataImage, EngineContext},
    osimage::OsImageFile,
};

/// Deploys all the filesystem images sourced from the OS Image to the
/// corresponding block devices.
#[tracing::instrument(name = "image_provision", skip_all)]
pub(super) fn deploy_images(ctx: &EngineContext) -> Result<(), TridentError> {
    // Get the filesystems that are sourced from the OS image
    let fs_from_img =
        filesystems_from_image(ctx).message("Failed to get filesystems sourced from the image")?;

    // If there are no filesystems sourced from the image, we have nothing to deploy?
    if fs_from_img.is_empty() {
        return Err(TridentError::internal(
            "Deployment in progress but no filesystems are sourced from the image",
        ));
    }

    // If we have filesystems sourced from the OS image, ensure that the OS image is available. This
    // should be caught during validation.
    let os_img = ctx.image.as_ref().structured(InternalError::Internal(
        "No OS image available for deployment",
    ))?;

    let images = os_img
        .filesystems()
        .map(|fs| (fs.mount_point.to_owned(), fs))
        .collect::<HashMap<_, _>>();

    // Now, deploy the filesystems sourced from the OS image
    for (id, mpp, fs) in fs_from_img {
        let image = images
            .get(mpp.as_path())
            .structured(InternalError::Internal("No image found for mount point"))
            .message(format!("Mount point '{}' should have image", mpp.display()))?;

        // Check if this ID is a verity device, if so, we must explore the graph
        // to obtain the underlying devices.
        if let Some(verity_device) = ctx.spec.storage.verity_device(&id) {
            let Some(image_file_verity) = image.verity.as_ref() else {
                // This case also should have been caught during validation.
                return Err(TridentError::internal(
                    "Attempt to deploy a filesystem image sourced from the OS image to a verity \
                    device, but no verity hash is available in the OS image.",
                ));
            };

            // Deploy the data image to the underlying data device.
            info!(
                "Initializing '{}': writing {} image for filesystem at '{}' from '{}'",
                verity_device.data_device_id,
                ByteCount::from(image.image_file.uncompressed_size).to_human_readable_approx(),
                image.mount_point.display(),
                os_img.source()
            );
            deploy_os_image_file(
                ctx,
                &verity_device.data_device_id,
                &image.image_file,
                FileSystemResize::NoResize,
            )
            .structured(ServicingError::DeployImages)?;

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
            )
            .structured(ServicingError::DeployImages)?;
        } else {
            // For non-verity devices, we can deploy the image directly.
            info!(
                "Initializing '{id}': writing {} image for filesystem at '{}' from '{}'",
                image.mount_point.display(),
                ByteCount::from(image.image_file.uncompressed_size).to_human_readable_approx(),
                os_img.source()
            );

            let fs_type = fs.fs_type.structured(InternalError::Internal(
                "Filesystem from image doesn't have fs type set",
            ))?;

            // Determine if/how the filesystem should be resized.
            let resize = if fs.is_read_only() || !fs_type.is_ext() {
                FileSystemResize::NoResize
            } else {
                FileSystemResize::Ext
            };

            deploy_os_image_file(ctx, &id, &image.image_file, resize)
                .structured(ServicingError::DeployImages)?;
        }
    }

    Ok(())
}

/// Scans the host configuration and the image in the engine context to find
/// filesystems sourced from the image. Returns a list of tuples containing
/// the block device ID, mount point path, and filesystem type for each filesystem
/// sourced from the OS image.
///
/// On A/B update, only filesystems that are part of an A/B volume pair are
/// returned.
fn filesystems_from_image(
    ctx: &EngineContext,
) -> Result<Vec<(BlockDeviceId, PathBuf, &FileSystemDataImage)>, TridentError> {
    let mut fs_list = Vec::new();

    for filesystem in &ctx.filesystems {
        let Some(img_fs) = filesystem.as_image() else {
            // Skip everything that is not sourced from the OS image.
            continue;
        };

        if img_fs.is_esp() {
            debug!(
                "Skipping deployment of filesystem [{}] sourced from OS Image, as it is the ESP.",
                filesystem.description()
            );
            continue;
        }

        let device_id = &img_fs.device_id;

        let mount_point_path = img_fs.mount_point_path();

        // If we're executing A/B update, Trident will only re-deploy images onto the A/B
        // volume pairs
        if ctx.servicing_type == ServicingType::AbUpdate
            && !ctx
                .storage_graph
                .has_ab_capabilities(device_id)
                .structured(InternalError::Internal(
                    "Failed to find device in storage graph",
                ))?
        {
            debug!(
                    "Skipping deployment of filesystem [{}] sourced from OS Image, as it is not part of an A/B volume pair.",
                    filesystem.description()
                );
            continue;
        }

        fs_list.push((device_id.clone(), mount_point_path.to_path_buf(), img_fs));
    }

    Ok(fs_list)
}

/// Resizes ext2/ext3/ext4 filesystem on the given block device to the maximum
/// size of the underlying block device.
fn resize_ext_fs(block_device_path: &Path) -> Result<(), Error> {
    resize2fs::run(block_device_path).context(format!(
        "Failed to resize partition on block device at path '{}'",
        block_device_path.display()
    ))
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
        .context(format!("No block device with id '{id}' found"))?;

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
