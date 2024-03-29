use std::path::Path;

use anyhow::{bail, Context, Error};
use log::{debug, info};
use reqwest::Url;
use uuid::Uuid;

use osutils::{container, e2fsck, resize2fs, tune2fs};
use trident_api::{
    config::{HostConfiguration, Image, ImageFormat, ImageSha256, PartitionType},
    constants::{BOOT_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
    error::TridentResultExt,
    status::{AbVolumeSelection, BlockDeviceContents, BlockDeviceInfo, HostStatus, ReconcileState},
    BlockDeviceId,
};

use crate::modules::{self, storage::tabfile};

pub(crate) mod stream_image;
#[cfg(feature = "sysupdate")]
mod systemd_sysupdate;

/// Function that streams images to block devices:
/// 1. If image is a local file or an HTTP file published in RawZstd format, Trident will evoke a
/// sub-module called Stream-Image, which will use HashingReader to write the bytes to the
/// target block device.
/// 2. If image is a local file or an HTTP file published in RawLzma format, Trident will run
/// systemd-sysupdate.rs to download the image, if needed, and write it to the block device. The
/// block device has to be a part of an A/B volume pair backed by partition block device. This is
/// b/c sysupdate can only operate if there are 2+ copies of the partition type as the partition
/// to be updated.
/// 3. TODO: If image is an HTTP file published as an OCI Artifact, ImageFormat OciArtifact,
/// Trident will download the image from Azure container registry and pass it to
/// systemd-sysupdate.rs. ADO task: https://dev.azure.com/mariner-org/ECF/_workitems/edit/5503/.
///
/// This function is called by the provision() function in the image submodule and
/// returns an error if the image cannot be downloaded or installed correctly.
fn update_images(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    for image in get_undeployed_images(host_status, host_config, false) {
        // Validate that block device exists
        let block_device = modules::get_block_device(host_status, &image.target_id, false)
            .context(format!(
                "No block device with id '{}' found",
                image.target_id
            ))?;

        // Parse the URL to determine the download strategy
        let image_url = Url::parse(image.url.as_str())
            .context(format!("Failed to parse image URL '{}'", image.url))?;

        if image_url.scheme() == "file" {
            match image.format {
                // If image is of format RawLzma, the target-id must be an A/B volume pair.
                #[cfg(feature = "sysupdate")]
                ImageFormat::RawLzma => {
                    // Fetch directory and filename from image URL
                    let (directory, filename, computed_sha256) =
                        systemd_sysupdate::get_local_image(&image_url, image)?;
                    // Run helper func systemd_sysupdate::deploy() to execute A/B update; since image is
                    // local, pass directory and file name as arg-s
                    systemd_sysupdate::deploy(
                        image,
                        host_status,
                        Some(directory.as_path()),
                        Some(filename.as_str()),
                        Some(&computed_sha256),
                    )
                    .context(format!(
                        "Failed to deploy image {} via sysupdate",
                        image.url
                    ))?;
                }

                // Otherwise, use direct streaming of image bytes onto the block device
                ImageFormat::RawZst => {
                    // If image does NOT correspond to ESP partition, use direct streaming of image
                    if !is_esp(host_config, &image.target_id) {
                        update_image(&image_url, image, host_status, &block_device, true).context(
                            format!(
                            "Failed to deploy image '{}' to block device '{}' via direct streaming",
                            image.url, image.target_id
                        ),
                        )?;
                    }
                    // If image corresponds to ESP partition, no need to deploy image directly; instead,
                    // perform file-based update of ESP later
                }
            }
        } else if image_url.scheme() == "http" || image_url.scheme() == "https" {
            match image.format {
                // If image is of format RawLzma AND target-id corresponds to an A/B volume pair,
                // use systemd-sysupdate.rs to write to the partition.
                //
                // TODO: Instead of delegating the download of the payload and hash verification to
                // systemd-sysupdate, do it from Trident, to support more format types and avoid
                // the SHA256SUMS overhead for the user. Related ADO task:
                // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6175.
                #[cfg(feature = "sysupdate")]
                ImageFormat::RawLzma => {
                    // Determine if target-id corresponds to an A/B volume pair; if helper
                    // func returns None, then set bool to false
                    let targets_ab_volume_pair = host_status
                        .get_ab_volume_partition(&image.target_id)
                        .is_some();

                    // If image is of format RawLzma but target-id does NOT
                    // correspond to an A/B volume pair, report an error
                    if !targets_ab_volume_pair {
                        bail!("Block device with id {} is not part of an A/B volume pair, so image in raw lzma format cannot be written to it.\nRaw lzma is not supported for block devices that do not correspond to A/B volume pairs",
                            &image.target_id)
                    }
                    // Run helper func systemd_sysupdate::deploy() to execute A/B update; directory and file
                    // name arg-s are None to communicate that update image is published via URL,
                    // not locally
                    systemd_sysupdate::deploy(image, host_status, None, None, None).context(
                        format!("Failed to deploy image {} via sysupdate", image.url),
                    )?;
                }

                // Otherwise, use direct streaming of image bytes onto the block device
                ImageFormat::RawZst => {
                    // If image does NOT correspond to ESP partition, use direct streaming of image
                    if !is_esp(host_config, &image.target_id) {
                        update_image(
                            &image_url,
                            image,
                            host_status,
                            &block_device,
                            // Set is_local to false since image is not a local file
                            false,
                        )
                        .context(format!(
                            "Failed to deploy image '{}' to block device '{}' via direct streaming",
                            image.url, image.target_id
                        ))?;
                    }
                    // If image corresponds to ESP partition, no need to deploy image directly; instead,
                    // perform file-based update of ESP later
                }
            }
        } else if image_url.scheme() == "oci" {
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
        } else {
            bail!("Unsupported URL scheme")
        };
    }
    Ok(())
}

/// Invokes stream_image::deploy() to deploy an image onto a non-ESP volume. If the volume is the
/// mount point for /boot, assigns a new randomized FS UUID to the updated volume. Accepts 5 arg-s:
/// 1. image_url: Url, which is the URL of the image to be deployed,
/// 2. image: &Image, which is the Image object from HostConfig,
/// 3. host_status,
/// 4. block_device: BlockDeviceInfo of the volume on which the image will be deployed,
/// 5. is_local: bool indicating whether the image is a local file or not.
fn update_image(
    image_url: &Url,
    image: &Image,
    host_status: &mut HostStatus,
    block_device: &BlockDeviceInfo,
    is_local: bool,
) -> Result<(), Error> {
    info!(
        "Deploying image from URL '{}' to block device '{}'",
        image.url, image.target_id
    );

    stream_image::deploy(image_url, image, host_status, block_device, is_local).context(
        format!(
            "Failed to deploy image '{}' to block device '{}' via direct streaming",
            image.url, image.target_id
        ),
    )?;

    // If target_id corresponds to a block device that serves as the mount point for /boot,
    // assign a new randomized FS UUID to that updated volume. This is necessary so that the grub
    // boot loader can select the correct volume to load the kernel and initrd from, when the
    // firmware reboots after the A/B update (and in generally, so that grub
    // picks the right /boot volume to boot from).
    if is_mount_point_for_boot(host_status, &image.target_id) {
        info!(
            "Identified block device with id '{}' as the mount point for /boot",
            image.target_id
        );

        let new_fs_uuid = update_fs_uuid(&block_device.path)
            .context(format!(
                "Failed to assign a new randomized filesystem UUID to updated volume on block device '{}'",
                &image.target_id
            ))?;

        info!(
            "Assigned a new randomized filesystem UUID '{}' to updated volume at path '{}'",
            new_fs_uuid,
            block_device.path.display()
        );
    }

    // If the image has ext* filesystem and is not to be mounted read-only,
    // resize the filesystem. For now, we determine the filesystem by looking at
    // the corresponding mountpoint.
    let mount_point = host_status
        .spec
        .storage
        .mount_points
        .iter()
        .find(|mp| mp.target_id == image.target_id);
    if let Some(mount_point) = mount_point {
        if (mount_point.filesystem == "ext4"
            || mount_point.filesystem == "ext3"
            || mount_point.filesystem == "ext2")
            && !mount_point.options.contains(&"ro".into())
        {
            // TODO investigate if we stop doing the check, tracked by https://dev.azure.com/mariner-org/ECF/_workitems/edit/7218
            info!("Checking filesystem on block device '{}'", &image.target_id);
            e2fsck::run(&block_device.path).context(format!(
                "Failed to check filesystem on block device '{}'",
                &image.target_id
            ))?;
            info!("Resizing filesystem on block device '{}'", &image.target_id);
            resize_ext_fs(&block_device.path).context(format!(
                "Failed to resize filesystem on block device '{}'",
                &image.target_id
            ))?;
        }
    }

    Ok(())
}

/// Validates whether the block device corresponding to target_id is the mount point for the
/// /boot directory.
fn is_mount_point_for_boot(host_status: &HostStatus, target_id: &BlockDeviceId) -> bool {
    // Fetch block device id corresponding to /boot from mount points and compare
    // boot_block_device_id with target_id
    if let Some(boot_block_device_id) = host_status
        .spec
        .storage
        .path_to_mount_point(Path::new(BOOT_MOUNT_POINT_PATH))
        .map(|mp| &mp.target_id)
    {
        boot_block_device_id == target_id
    } else {
        false
    }
}

/// Assigns a new randomized FS UUID to the updated volume. Accepts one arg: block_device_path,
/// which is the block device path of the updated volume. Returns the new FS UUID.
fn update_fs_uuid(block_device_path: &Path) -> Result<Uuid, Error> {
    // Generate a random UUID for the updated volume
    let fs_uuid = Uuid::new_v4();
    // Run tune2fs to assign a new randomized FS UUID to the updated volume
    tune2fs::run(&fs_uuid, block_device_path).context(format!(
        "Failed to assign a new randomized filesystem UUID '{}' to updated volume at path '{}'",
        fs_uuid,
        block_device_path.display()
    ))?;

    Ok(fs_uuid)
}

/// Resize ext2/ext3/ext4 filesystem on the given block device to the maximum
/// size of the underlying block device
fn resize_ext_fs(block_device_path: &Path) -> Result<(), Error> {
    resize2fs::run(block_device_path).context(format!(
        "Failed to resize partition on block device at path '{}'",
        block_device_path.display()
    ))
}

/// Checks if block device corresponding to target_id is ESP partition. This func assumes that disk
/// always contains a stand-alone ESP partition that is not part of an A/B volume pair. This func
/// takes two arg-s:
/// 1. host_status, which is a reference to HostStatus object.
/// 2. target_id, which is a reference to a String representing the id of the block device.
//
/// Returns `true` if the partition is of type ESP, `false` otherwise or if not found.
pub(super) fn is_esp(host_config: &HostConfiguration, target_id: &BlockDeviceId) -> bool {
    // Iterate through all disks and partitions
    host_config
        .storage
        .disks
        .iter()
        .flat_map(|disk| &disk.partitions) // Flatten partitions from all disks
        .find(|&partition| &partition.id == target_id) // Find the target partition
        .map_or(false, |partition| {
            partition.partition_type == PartitionType::Esp
        }) // Check if it's an ESP partition
}

/// Returns a list of images that are undeployed.
///
/// An undeployed image is an Image in the HostConfiguration that meets
/// one of three conditions:
///
/// 1. It is not in HostStatus.
/// 2. Its target device does not contain an image.
/// 3. Its target device contains a different image than the one specified
///    in the HostConfiguration.
///
/// An image is different if either the url or sha256sum values are
/// different. If the sha256sum is set to "ignored" in the
/// HostConfiguration, then only the url must be different no matter the
/// contents of the target device.
///
/// If the target device is an A/B volume pair, then the active boolean is
/// used to determine whether that resolves to the active volume or the
/// inactive one.
pub(crate) fn get_undeployed_images<'a>(
    host_status: &HostStatus,
    host_config: &'a HostConfiguration,
    active: bool,
) -> Vec<&'a Image> {
    host_config
        .storage
        .images
        .iter()
        .filter(|image| {
            if let Some(bdi) = modules::get_block_device(host_status, &image.target_id, active) {
                if let BlockDeviceContents::Image { sha256, url, .. } = bdi.contents {
                    if url == image.url
                        && match image.sha256 {
                            ImageSha256::Checksum(ref sha256sum) => *sha256sum == sha256,
                            ImageSha256::Ignored => true,
                        }
                    {
                        return false;
                    }
                }
            }
            true
        })
        .collect()
}

pub(super) fn refresh_host_status(host_status: &mut HostStatus) -> Result<(), Error> {
    // If running in a container, look for the host root mount point
    let root_mount_path = if container::is_running_in_container()
        .unstructured("Failed to determine wheter running in a container")?
    {
        container::get_host_root_path().unstructured("Failed to get host root mount path")?
    } else {
        Path::new(ROOT_MOUNT_POINT_PATH).to_path_buf()
    };
    // update root_device_path of the active root volume
    host_status.storage.root_device_path = Some(
        tabfile::get_device_path(Path::new("/proc/mounts"), root_mount_path.as_path())
            .context("Failed to find root mount point")?,
    );
    debug!(
        "Using root device path: {:?}",
        host_status.storage.root_device_path
    );

    if let Some((volume_a_path, volume_b_path)) = get_root_ab_volumes(host_status) {
        // update the active volume in the a/b scheme based on what
        // is the current root volume
        let root_device_path = host_status
            .storage
            .root_device_path
            .as_ref()
            .context("No root device")?;

        // TODO: better error handling if canonicalize fails
        host_status.storage.ab_active_volume = if &volume_a_path.canonicalize()? == root_device_path
        {
            Some(AbVolumeSelection::VolumeA)
        } else if &volume_b_path.canonicalize()? == root_device_path {
            Some(AbVolumeSelection::VolumeB)
        } else {
            // To prevent data loss, abort if we cannot find the
            // matching root volume outside of clean install
            if host_status.reconcile_state != ReconcileState::CleanInstall {
                bail!("No matching root volume found");
            }
            None
        };
        debug!("Active volume: {:?}", host_status.storage.ab_active_volume);
    } else {
        host_status.storage.ab_active_volume = None;
    }

    Ok(())
}

fn get_root_ab_volumes(host_status: &HostStatus) -> Option<(&Path, &Path)> {
    // if a/b update is enabled
    let ab_update = &host_status.spec.storage.ab_update.as_ref()?;

    // and mount points have a reference to root volume
    let root_device_id = host_status
        .spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
        .map(|m| &m.target_id)?;

    // and one of the a/b update volumes points to the root volume
    let root_device_pair = ab_update
        .volume_pairs
        .iter()
        .find(|p| &p.id == root_device_id)?;

    // and both volumes are initialized
    let volume_a_path = &host_status
        .storage
        .block_devices
        .get(&root_device_pair.volume_a_id)?
        .path;
    let volume_b_path = &host_status
        .storage
        .block_devices
        .get(&root_device_pair.volume_b_id)?
        .path;

    Some((volume_a_path, volume_b_path))
}

pub(super) fn needs_ab_update(host_status: &HostStatus, host_config: &HostConfiguration) -> bool {
    let undeployed_images = get_undeployed_images(host_status, host_config, true);
    if !undeployed_images.is_empty() {
        debug!("Found following images to update: {:?}", undeployed_images);
    }
    !undeployed_images.is_empty()
}

/// Validates that every undeployed image targets either the ESP partition or an A/B volume pair.
/// If not, returns an error to reject HostConfiguration.
///
/// This func is called only during A/B updates, to ensure that the HostConfiguration does not
/// request Trident to overwrite images on the volumes that are shared between A and B, such as
/// /var/lib/trident.
pub(super) fn validate_undeployed_images(
    host_status: &HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    for image in get_undeployed_images(host_status, host_config, false) {
        let is_valid_target = if let Some(ab_update) = &host_status.spec.storage.ab_update {
            // If ab_update is not null, check if target_id corresponds to an A/B volume pair or
            // ESP partition
            ab_update
                .volume_pairs
                .iter()
                .any(|p| p.id == image.target_id)
                || is_esp(host_config, &image.target_id)
        } else {
            // If ab_update is null, only check if target_id corresponds to the ESP partition
            is_esp(host_config, &image.target_id)
        };

        if !is_valid_target {
            bail!(
                "Image '{}' cannot target block device '{}' as it is neither the ESP partition nor an A/B volume pair, so it cannot be overwritten during A/B update",
                image.url, image.target_id
            )
        }
    }

    Ok(())
}

pub(super) fn provision(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    // Only call refresh_ab_volumes() and set active_volume to None if
    // the reconcile_state is CleanInstall
    if host_status.reconcile_state == ReconcileState::CleanInstall {
        debug!("Initializing A/B volumes");
        host_status.storage.ab_active_volume = None;
    }

    update_images(host_status, host_config).context("Failed to update filesystem images")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use maplit::btreemap;

    use trident_api::{
        config::{
            AbUpdate, AbVolumePair, Disk, ImageSha256, MountPoint, Partition, PartitionSize,
            PartitionType, Storage as StorageConfig,
        },
        status::{Storage, UpdateKind},
    };

    use super::*;

    #[test]
    fn test_get_undeployed_images() {
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: StorageConfig {
                    mount_points: vec![
                        MountPoint {
                            path: PathBuf::from("/boot"),
                            target_id: "boot".to_string(),
                            filesystem: "fat32".to_string(),
                            options: vec![],
                        },
                        MountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            target_id: "root".to_string(),
                            filesystem: "ext4".to_string(),
                            options: vec![],
                        },
                    ],
                    images: vec![
                        Image {
                            url: "http://example.com/esp.img".to_string(),
                            target_id: "boot".to_string(),
                            format: ImageFormat::RawZst,
                            sha256: ImageSha256::Checksum("foobar".to_string()),
                        },
                        Image {
                            url: "http://example.com/image1.img".to_string(),
                            target_id: "root".to_string(),
                            format: ImageFormat::RawZst,
                            sha256: ImageSha256::Ignored,
                        },
                    ],
                    disks: vec![Disk {
                        id: "foo".to_string(),
                        device: PathBuf::from("/dev/sda"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(100),
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "foo".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda"),
                        size: 10,
                        contents: BlockDeviceContents::Initialized,
                    },
                    "boot".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda1"),
                        size: 100,
                        contents: BlockDeviceContents::Image {
                            url: "http://example.com/esp.img".to_string(),
                            sha256: "foobar".to_string(),
                            length: 100,
                        },
                    },
                    "root".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda2"),
                        size: 100,
                        contents: BlockDeviceContents::Image {
                            url: "http://example.com/image1.img".to_string(),
                            sha256: "foobar".to_string(),
                            length: 100,
                        },
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // should be zero, as images are matching and hash is ignored
        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false).len(),
            0
        );

        // should be zero, as images and hashes are matching
        host_status.spec.storage.images[0].sha256 = ImageSha256::Checksum("foobar".to_string());
        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false).len(),
            0
        );

        // should be one, as image hash is different
        host_status.spec.storage.images[0].sha256 = ImageSha256::Checksum("barfoo".to_string());
        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false),
            vec![&Image {
                url: "http://example.com/esp.img".to_string(),
                target_id: "boot".to_string(),
                format: ImageFormat::RawZst,
                sha256: ImageSha256::Checksum("barfoo".to_string()),
            }]
        );

        // should be one, as image url is different
        host_status.spec.storage.images[0].sha256 = ImageSha256::Ignored;
        host_status.spec.storage.images[0].url = "http://example.com/image2.img".to_string();
        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false),
            vec![&Image {
                url: "http://example.com/image2.img".to_string(),
                target_id: "boot".to_string(),
                format: ImageFormat::RawZst,
                sha256: ImageSha256::Ignored,
            }]
        );

        // could be zero, as despite the url being different, the hash is the
        // same; for now though we reimage to be safe, hence 1
        host_status.spec.storage.images[0].sha256 = ImageSha256::Checksum("foobar".to_string());
        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false),
            vec![&Image {
                url: "http://example.com/image2.img".to_string(),
                target_id: "boot".to_string(),
                format: ImageFormat::RawZst,
                sha256: ImageSha256::Checksum("foobar".to_string()),
            }]
        );

        // should be 2, as the image is not initialized and the other is from
        // the previous case
        host_status
            .storage
            .block_devices
            .get_mut("root")
            .unwrap()
            .contents = BlockDeviceContents::Unknown;
        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false),
            vec![
                &Image {
                    url: "http://example.com/image2.img".to_string(),
                    target_id: "boot".to_string(),
                    format: ImageFormat::RawZst,
                    sha256: ImageSha256::Checksum("foobar".to_string()),
                },
                &Image {
                    url: "http://example.com/image1.img".to_string(),
                    target_id: "root".to_string(),
                    format: ImageFormat::RawZst,
                    sha256: ImageSha256::Ignored,
                }
            ]
        );

        // root config is not matching root status
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: StorageConfig {
                    mount_points: vec![
                        MountPoint {
                            path: PathBuf::from("/boot"),
                            target_id: "boot".to_string(),
                            filesystem: "fat32".to_string(),
                            options: vec![],
                        },
                        MountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            target_id: "root".to_string(),
                            filesystem: "ext4".to_string(),
                            options: vec![],
                        },
                    ],
                    images: vec![
                        Image {
                            url: "http://example.com/esp.img".to_string(),
                            target_id: "boot".to_string(),
                            format: ImageFormat::RawZst,
                            sha256: ImageSha256::Checksum("foobar".to_string()),
                        },
                        Image {
                            url: "http://example.com/image1.img".to_string(),
                            target_id: "root".to_string(),
                            format: ImageFormat::RawZst,
                            sha256: ImageSha256::Ignored,
                        },
                    ],
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "root".into(),
                            volume_a_id: "root-a".to_string(),
                            volume_b_id: "root-b".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "foo".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda"),
                        size: 10,
                        contents: BlockDeviceContents::Initialized,
                    },
                    "boot".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda1"),
                        size: 100,
                        contents: BlockDeviceContents::Image {
                            url: "http://example.com/esp.img".to_string(),
                            sha256: "foobar".to_string(),
                            length: 100,
                        },
                    },
                    "root-b".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda2"),
                        size: 100,
                        contents: BlockDeviceContents::Image {
                            url: "http://example.com/image1.img".to_string(),
                            sha256: "foobar".to_string(),
                            length: 100,
                        },
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false),
            vec![&Image {
                url: "http://example.com/image1.img".to_string(),
                target_id: "root".to_string(),
                format: ImageFormat::RawZst,
                sha256: ImageSha256::Ignored,
            }]
        );

        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, true),
            // Vec::<&Image>::new()
            vec![&Image {
                url: "http://example.com/image1.img".to_string(),
                target_id: "root".to_string(),
                format: ImageFormat::RawZst,
                sha256: ImageSha256::Ignored,
            }]
        );

        // with a/b update, we should get ...

        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        host_status.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false),
            Vec::<&Image>::new()
        );

        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, true),
            vec![&Image {
                url: "http://example.com/image1.img".to_string(),
                target_id: "root".to_string(),
                format: ImageFormat::RawZst,
                sha256: ImageSha256::Ignored,
            }]
        );
    }

    /// Validates that is_esp() correctly determines whether block device corresponds to
    /// ESP partition.
    #[test]
    fn test_is_esp() {
        // Setup HostStatus with predefined disks and partitions
        let mut host_config = HostConfiguration {
            storage: StorageConfig {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: PathBuf::from("/dev/disk/by-bus/foobar"),
                    partitions: vec![
                        Partition {
                            id: "esp".to_string(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::Fixed(100),
                        },
                        Partition {
                            id: "root-a".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::Fixed(100),
                        },
                        Partition {
                            id: "root-b".to_string(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::Fixed(100),
                        },
                    ],
                    ..Default::default()
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case 1: Check for ESP partition
        assert!(
            is_esp(&host_config, &"esp".to_string()),
            "ESP partition was not correctly identified"
        );

        // Test case 2: Check for non-ESP partition
        assert!(
            !is_esp(&host_config, &"root-a".to_string()),
            "Non-ESP partition was incorrectly identified as ESP partition"
        );

        // Test case 3: Check for non-existent partition
        assert!(
            !is_esp(&host_config, &"non-existent".to_string()),
            "Non-existent partition was incorrectly identified as ESP partition"
        );

        // Test case 4: Change the id of ESP partition to non-ESP
        for disk in host_config.storage.disks.iter_mut() {
            for partition in &mut disk.partitions {
                if partition.id == "esp" {
                    partition.id = "non-esp".to_owned();
                }
            }
        }
        assert!(
            is_esp(&host_config, &"non-esp".to_string()),
            "ESP partition was not correctly identified"
        );
    }

    /// Validates that is_mount_point_for_boot() correctly determines whether the block device is
    /// a mount point for /boot.
    #[test]
    fn test_is_mount_point_for_boot() {
        // Setup HostStatus with predefined mount points
        let host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: StorageConfig {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        MountPoint {
                            path: PathBuf::from("/boot"),
                            target_id: "boot".to_string(),
                            filesystem: "fat32".to_string(),
                            options: vec![],
                        },
                        MountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            target_id: "root".to_string(),
                            filesystem: "ext4".to_string(),
                            options: vec![],
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case 1: Check for mount point for /boot
        assert!(
            is_mount_point_for_boot(&host_status, &"boot".to_string()),
            "Block device with target_id boot was not correctly identified as mount point for /boot"
        );

        // Test case 2: Check for non-mount point for /boot
        assert!(
            !is_mount_point_for_boot(&host_status, &"root".to_string()),
            "Block device with target_id root was incorrectly identified as mount point for /boot"
        );

        // Test case 3: Check for non-existent mount point
        assert!(
            !is_mount_point_for_boot(&host_status, &"non-existent".to_string()),
            "Non-existent target_id was incorrectly identified as mount point for /boot"
        );
    }

    /// Validates that the logic in validate_undeployed_images() is correct.
    #[test]
    fn test_validate_undeployed_images() {
        // Initialize a HostStatus object
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: StorageConfig {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "esp".to_string(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root-a".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root-b".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "trident".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(100),
                            },
                        ],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        MountPoint {
                            path: PathBuf::from("/esp"),
                            target_id: "esp".to_string(),
                            filesystem: "fat32".to_string(),
                            options: vec![],
                        },
                        MountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            target_id: "root".to_string(),
                            filesystem: "ext4".to_string(),
                            options: vec![],
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "esp".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-a".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-b".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "trident".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp4"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Initialize image objects
        let image_esp = Image {
            url: "http://example.com/esp_1.img".to_string(),
            target_id: "esp".to_string(),
            format: ImageFormat::RawZst,
            sha256: ImageSha256::Checksum("esp_sha256_1".to_string()),
        };
        let image_root = Image {
            url: "http://example.com/root_1.img".to_string(),
            target_id: "root".to_string(),
            format: ImageFormat::RawZst,
            sha256: ImageSha256::Checksum("root_sha256_1".to_string()),
        };
        let image_trident = Image {
            url: "http://example.com/trident_1.img".to_string(),
            target_id: "trident".to_string(),
            format: ImageFormat::RawZst,
            sha256: ImageSha256::Checksum("trident_sha256_1".to_string()),
        };

        // Test case 1: Running validate_undeployed_images() when update of ESP image only is
        // requested should return ((Ok)), even if ab_update is null.
        // Update images section of host_config
        host_status.spec.storage.images = vec![image_esp.clone()];
        assert!(
            validate_undeployed_images(&host_status,&host_status.spec).is_ok(),
            "Failed to determine that no images should be undeployed when update of ESP image is requested"
        );

        // Test case 2: Running validate_undeployed_images() when update of ESP and root images is
        // requested should return an error since ab_update is null.
        // Update images section of host_config
        host_status.spec.storage.images = vec![image_esp.clone(), image_root.clone()];
        // Compare the actual error kind with the expected one.
        assert_eq!(
            validate_undeployed_images(&host_status,&host_status.spec)
                .unwrap_err()
                .root_cause()
                .to_string(),
                "Image 'http://example.com/root_1.img' cannot target block device 'root' as it is neither the ESP partition nor an A/B volume pair, so it cannot be overwritten during A/B update",
            "Unexpected error kind"
        );

        // Test case 3: Running validate_undeployed_images() when all images are valid should
        // return ((Ok))
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        host_status.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        host_status.spec.storage.images = vec![image_esp.clone()];
        assert!(
            validate_undeployed_images(&host_status, &host_status.spec).is_ok(),
            "Failed to determine that no images should be undeployed when all images are valid"
        );

        // Test case 4: Running validate_undeployed_images() when there is an image requested for
        // block device 'trident' should return an error since it's neither the ESP partition nor
        // an A/B volume pair
        // Update images section of host_config
        host_status.spec.storage.images =
            vec![image_esp.clone(), image_root.clone(), image_trident.clone()];
        // Compare the actual error kind with the expected one.
        assert_eq!(
            validate_undeployed_images(&host_status,&host_status.spec)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Image 'http://example.com/trident_1.img' cannot target block device 'trident' as it is neither the ESP partition nor an A/B volume pair, so it cannot be overwritten during A/B update",
            "Unexpected error kind"
        );

        // Test case 5: Running validate_undeployed_images() when there is an image requested for
        // root should return an error since root is a single volume and not an A/B volume pair in
        // this scenario
        let mut host_status_2 = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: StorageConfig {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partitions: vec![
                            Partition {
                                id: "esp".to_string(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "boot-a".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "boot-b".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(100),
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "esp".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot-a".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot-b".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp4"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        host_status_2.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "boot".to_string(),
                volume_a_id: "boot-a".to_string(),
                volume_b_id: "boot-b".to_string(),
            }],
        });
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        let image_boot = Image {
            url: "http://example.com/boot_1.img".to_string(),
            target_id: "boot".to_string(),
            format: ImageFormat::RawZst,
            sha256: ImageSha256::Checksum("boot_sha256_1".to_string()),
        };

        // Update images section of host_config
        host_status_2.spec.storage.images =
            vec![image_esp.clone(), image_root.clone(), image_boot.clone()];
        // Compare the actual error kind with the expected one.
        assert_eq!(
            validate_undeployed_images(&host_status_2, &host_status_2.spec)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Image 'http://example.com/root_1.img' cannot target block device 'root' as it is neither the ESP partition nor an A/B volume pair, so it cannot be overwritten during A/B update",
            "Unexpected error kind"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use osutils::{blkid, mkfs};

    /// Validates that run() correctly assigns a new UUID to the filesystem.
    #[functional_test(feature = "helpers")]
    fn test_update_fs_uuid() {
        let block_device_path = Path::new("/dev/sdb");
        // Create a new ext4 filesystem on /dev/sdb
        mkfs::run(block_device_path, "ext4").unwrap();

        let new_uuid = update_fs_uuid(block_device_path).unwrap();

        // Validate that the UUID was assigned correctly by running blkid command to fetch block
        // devices
        let fs_uuid = blkid::get_filesystem_uuid(block_device_path).unwrap();

        // Assert that the UUIDs match
        assert_eq!(fs_uuid, new_uuid);
    }
}
