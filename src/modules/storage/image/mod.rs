use anyhow::{bail, Context, Error};
use log::{debug, info};
use nix::NixPath;
use reqwest::Url;
use sha2::Digest;
use std::{
    ffi::CString,
    fs::{self},
    io::{self, Read},
    os::{fd::AsRawFd, unix::prelude::PermissionsExt},
    path::Path,
    process::Command,
};

use trident_api::{
    config::{HostConfiguration, Image, ImageFormat, ImageSha256},
    status::{
        AbUpdate, AbVolumePair, AbVolumeSelection, BlockDeviceContents, Disk, HostStatus,
        Partition, RaidArray, ReconcileState,
    },
    BlockDeviceId,
};

use crate::modules::{self, storage::tabfile::TabFile};
use osutils::udevadm;
pub mod mount;
mod stream_image;
mod systemd_sysupdate;
mod update_grub;

/// This struct wraps a reader and computes the SHA256 hash of the data as it is read.
struct HashingReader<R: Read>(R, sha2::Sha256);
impl<R: Read> HashingReader<R> {
    fn new(reader: R) -> Self {
        Self(reader, sha2::Sha256::new())
    }

    fn hash(&self) -> String {
        format!("{:x}", self.1.clone().finalize())
    }
}
impl<R: Read> Read for HashingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        // Read the requested amount of data from the inner reader
        let n = self.0.read(buf)?;
        // Update the hash with the data we read
        self.1.update(&buf[..n]);
        // Return the number of bytes read
        Ok(n)
    }
}

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

        // TODO: Add more options for download sources
        info!(
            "Updating image of block device with id '{}'",
            &image.target_id
        );
        info!(
            "Downloading image from URL '{}' in '{:?}' format",
            &image.url, &image.format
        );

        // Parse the URL to determine the download strategy
        let image_url = Url::parse(image.url.as_str())
            .context(format!("Failed to parse image URL '{}'", image.url))?;

        if image_url.scheme() == "file" {
            match image.format {
                // If image is of format RawLzma AND target-id corresponds to an A/B volume pair,
                // use systemd-sysupdate.rs to write to the partition
                ImageFormat::RawLzma => {
                    if !cfg!(feature = "sysupdate") {
                        bail!("Image format RawLzma is not supported")
                    }

                    // Determine if target-id corresponds to an A/B volume pair; if helper
                    // func returns None, then set bool to false
                    let targets_ab_volume_pair =
                        systemd_sysupdate::get_ab_volume_partition(host_status, &image.target_id)
                            .is_some();

                    // If image format is raw-lzma but block device is not part of an A/B volume
                    // pair, return error b/c sysupdate requires 2 copies of partitition for an
                    // update
                    if !targets_ab_volume_pair {
                        bail!("Block device with id {} is not part of an A/B volume pair, so image in raw lzma format cannot be written to it.",
                            &image.target_id)
                    }
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

                // If image format is raw-zstd, run helper func direct_deploy() to write image from
                // local file via stream_image.rs
                ImageFormat::RawZstd => {
                    // 5th arg is False to communicate that image is a local file, i.e.,  is_local
                    // will be set to True
                    stream_image::deploy(image_url, image, host_status, &block_device, true)
                        .context(format!(
                            "Failed to deploy image {} via direct streaming",
                            image.url
                        ))?;
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
                ImageFormat::RawLzma => {
                    if !cfg!(feature = "sysupdate") {
                        bail!("Image format RawLzma is not supported")
                    }

                    // Determine if target-id corresponds to an A/B volume pair; if helper
                    // func returns None, then set bool to false
                    let targets_ab_volume_pair =
                        systemd_sysupdate::get_ab_volume_partition(host_status, &image.target_id)
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

                // Otherwise, use direct streaming of image bytes onto the block device, by using
                // stream_image.rs
                ImageFormat::RawZstd => {
                    // 5th arg is False to communicate that image is not a local file, i.e.,
                    // is_local will be set to False
                    stream_image::deploy(image_url, image, host_status, &block_device, false)
                        .context(format!(
                            "Failed to deploy image {} via direct streaming",
                            image.url
                        ))?;
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
            todo!("Need to implement downloading images as OCI artifacts from Azure container registry")
        } else {
            bail!("Unsupported URL scheme")
        };
    }
    Ok(())
}

fn get_disk_mut<'a>(
    host_status: &'a mut HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<&'a mut Disk> {
    host_status.storage.disks.get_mut(block_device_id)
}

fn get_partition_mut<'a>(
    host_status: &'a mut HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<&'a mut Partition> {
    host_status
        .storage
        .disks
        .iter_mut()
        .flat_map(|(_block_device_id, disk)| &mut disk.partitions)
        .find(|p| p.id == *block_device_id)
}

fn get_raid_mut<'a>(
    host_status: &'a mut HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<&'a mut RaidArray> {
    host_status.storage.raid_arrays.get_mut(block_device_id)
}

fn set_host_status_block_device_contents(
    host_status: &mut HostStatus,
    block_device_id: &BlockDeviceId,
    contents: BlockDeviceContents,
) -> Result<(), Error> {
    if let Some(disk) = get_disk_mut(host_status, block_device_id) {
        disk.contents = contents;
        return Ok(());
    }

    if let Some(partition) = get_partition_mut(host_status, block_device_id) {
        partition.contents = contents;
        return Ok(());
    }

    if let Some(ab_update) = &host_status.storage.ab_update {
        if let Some(ab_volume_pair) = ab_update.volume_pairs.get(block_device_id) {
            let target_id = match modules::get_ab_update_volume(host_status, false) {
                Some(AbVolumeSelection::VolumeA) => Some(&ab_volume_pair.volume_a_id),
                Some(AbVolumeSelection::VolumeB) => Some(&ab_volume_pair.volume_b_id),
                None => None,
            };
            if let Some(target_id) = target_id {
                return set_host_status_block_device_contents(
                    host_status,
                    &target_id.clone(),
                    contents,
                );
            }
        }
    }

    if let Some(raid) = get_raid_mut(host_status, block_device_id) {
        raid.contents = contents;
        return Ok(());
    }

    anyhow::bail!("No block device with id '{}' found", block_device_id);
}

pub fn kexec(mount_path: &Path, args: &str) -> Result<(), Error> {
    let root = mount_path
        .to_str()
        .context(format!("Non-utf8 mount point: {}", mount_path.display()))?;

    info!("Searching for kernel and initrd");
    let kernel_path = glob::glob(&format!("{root}/boot/vmlinuz-*"))?
        .next()
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No kernel found",
        ))??;

    let initrd_path = glob::glob(&format!("{root}/boot/initrd.img-*"))?
        .next()
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No initrd found",
        ))??;

    info!("Opening kernel and initrd");
    let kernel = fs::File::open(kernel_path)?;
    let initrd = fs::File::open(initrd_path)?;
    let args = CString::new(args)?;

    // Run kexec file load.
    info!("Loading kernel");
    let r = unsafe {
        libc::syscall(
            libc::SYS_kexec_file_load,
            kernel.as_raw_fd(),
            initrd.as_raw_fd(),
            args.len() + 1,
            args.as_ptr(),
            0,
        )
    };
    if r < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    // Close remaining files and sync all writes to the filesystem.
    drop(kernel);
    drop(initrd);
    nix::unistd::sync();

    mount::unmount_updated_volumes(mount_path)?;

    // Kexec into image.
    info!("Rebooting system");
    let r = unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_KEXEC) };
    if r < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    unreachable!()
}

#[allow(unused)]
pub fn reboot() -> Result<(), Error> {
    // Sync all writes to the filesystem.
    nix::unistd::sync();

    info!("Rebooting system");
    crate::run_command(Command::new("systemctl").arg("reboot"))
        .context("Failed to reboot the host")?;

    unreachable!()
}

fn refresh_ab_volumes(host_status: &mut HostStatus, host_config: &HostConfiguration) {
    host_status.storage.ab_update = host_config.storage.ab_update.as_ref().map(|ab_update| {
        let ab_volume_pairs = ab_update
            .volume_pairs
            .iter()
            .map(|p| {
                (
                    p.id.clone(),
                    AbVolumePair {
                        volume_a_id: p.volume_a_id.clone(),
                        volume_b_id: p.volume_b_id.clone(),
                    },
                )
            })
            .collect();

        AbUpdate {
            volume_pairs: ab_volume_pairs,
            active_volume: None,
        }
    });
}

/// Returns a list of images that need to be updated/provisioned.
///
/// This goes through the list of images in the HostConfiguration and compares
/// them to the HostStatus. If an image is targeting an AB Volume Pair and
/// active is true, it compared the image against the active volume, and if
/// active is false it compares the image against the update volume (i.e. the
/// one that isn't active).
fn get_undeployed_images<'a>(
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
    // update root_device_path of the active root volume
    host_status.storage.root_device_path = Some(
        TabFile::get_device_path(Path::new("/proc/mounts"), Path::new("/"))
            .context("Failed find root mount point")?,
    );

    // if a/b update is enabled
    if let Some(ab_update) = &host_status.storage.ab_update {
        // and mount points have a reference to root volume
        if let Some(root_device_id) = host_status
            .storage
            .mount_points
            .iter()
            .find(|(_id, mp)| mp.path == Path::new("/"))
            .map(|(id, _mp)| id.clone())
        {
            // and one of the a/b update volumes points to the root volume
            if let Some(root_device_pair) = ab_update.volume_pairs.get(&root_device_id) {
                let volume_a_path =
                    modules::get_block_device(host_status, &root_device_pair.volume_a_id, false)
                        .context("Failed to get block device for volume A")?
                        .path;

                let volume_b_path =
                    modules::get_block_device(host_status, &root_device_pair.volume_b_id, false)
                        .context("Failed to get block device for volume B")?
                        .path;

                // update the active volume in the a/b scheme based on what
                // is the current root volume
                if let Some(root_device_path) = &host_status.storage.root_device_path {
                    host_status
                        .storage
                        .ab_update
                        .as_mut()
                        .unwrap()
                        .active_volume = if &volume_a_path.canonicalize()? == root_device_path {
                        Some(AbVolumeSelection::VolumeA)
                    } else if &volume_b_path.canonicalize()? == root_device_path {
                        Some(AbVolumeSelection::VolumeB)
                    } else {
                        None
                    };
                }
            }
        }
    }

    Ok(())
}

pub(super) fn needs_ab_update(host_status: &HostStatus, host_config: &HostConfiguration) -> bool {
    let undeployed_images = get_undeployed_images(host_status, host_config, true);
    if !undeployed_images.is_empty() {
        debug!("Found following images to update: {:?}", undeployed_images);
    }
    !undeployed_images.is_empty()
}

pub(super) fn provision(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
    mount_point: &Path,
) -> Result<(), Error> {
    // Only call refresh_ab_volumes() and set active_volume to None if
    // the reconcile_state is CleanInstall
    if host_status.reconcile_state == ReconcileState::CleanInstall {
        refresh_ab_volumes(host_status, host_config);
    }

    update_images(host_status, host_config).context("Failed to update filesystem images")?;
    mount::mount_updated_volumes(host_config, host_status, mount_point, false)
        .context("Failed to mount the updated volumes")?;

    Ok(())
}

pub(super) fn configure(
    host_status: &mut HostStatus,
    _host_config: &HostConfiguration,
) -> Result<(), Error> {
    // Patch /var in case it was injected as a volume

    // TODO - this is a temporary fix for the issue where /var is mounted as
    // a volume, longer term, we should either require user to provide /var
    // partition image or allow to copy contents of /var from the root fs
    // image, similar to what MIC will do

    // if we let users mount over /var, some services will fail to start, so
    // we need to recreate missing directories first
    let var_log_path = Path::new("/var/log");
    if !var_log_path.exists() {
        fs::create_dir(var_log_path)?;
        fs::set_permissions(var_log_path, fs::Permissions::from_mode(0o755))?;
    }

    // auditd requires /var/log/audit to be present, and auditd is a
    // required component for Mariner images
    let var_log_audit_path = var_log_path.join("audit");
    if !var_log_audit_path.exists() {
        fs::create_dir(&var_log_audit_path)?;
        fs::set_permissions(var_log_audit_path, fs::Permissions::from_mode(0o700))?;
    }

    // sshd requires /var/lib/sshd to be present, and sshd is a
    // required component for Mariner images
    let var_lib_path = Path::new("/var/lib");
    if !var_lib_path.exists() {
        fs::create_dir(var_lib_path)?;
        fs::set_permissions(var_lib_path, fs::Permissions::from_mode(0o755))?;
    }
    let var_lib_sshd_path = var_lib_path.join("sshd");
    if !var_lib_sshd_path.exists() {
        fs::create_dir(&var_lib_sshd_path)?;
        fs::set_permissions(var_lib_sshd_path, fs::Permissions::from_mode(0o700))?;
    }

    // End of patch block

    // Fetch the root partition ID from HostStatus; it corresponds to the
    // "/", root fs, mountpoint
    let root_id = host_status
        .storage
        .mount_points
        .iter()
        .find(|(_, mp)| mp.path == Path::new("/"))
        .map(|(id, _)| id.clone())
        .context("Failed to find root partition")?;

    // Fetch the Partition object corresponding to root_id
    if let Some(root_part) = systemd_sysupdate::get_ab_volume_partition(host_status, &root_id) {
        udevadm::settle()?;
        let root_uuid = update_grub::get_uuid_from_path(&root_part.path)?;

        // Call update_grub() to update the UUID of root FS and if needed,
        // PARTUUID of root partition inside GRUB config files
        update_grub::update_grub_rootfs(
            update_grub::GRUB_BOOT_CONFIG_PATH,
            &root_uuid,
            Some(&root_part.uuid.to_string()),
        )
        .context(format!(
            "Failed to update GRUB config at path '{}'",
            &update_grub::GRUB_BOOT_CONFIG_PATH
        ))?;

        // For GRUB_EFI_CONFIG_PATH, no need to update the PARTUUID of root FS inside GRUB
        update_grub::update_grub_rootfs(update_grub::GRUB_EFI_CONFIG_PATH, &root_uuid, None)
            .context(format!(
                "Failed to update GRUB config at path '{}'",
                &update_grub::GRUB_EFI_CONFIG_PATH
            ))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, path::PathBuf};

    use maplit::btreemap;
    use uuid::Uuid;

    use trident_api::{
        config::{
            AbUpdate as AbUpdateConfig, AbVolumePair as AbVolumePairConfig, ImageSha256,
            MountPoint, PartitionType, Storage as StorageConfig,
        },
        status::{MountPoint as MountPointStatus, Storage, UpdateKind},
    };

    use super::*;

    #[test]
    fn test_hashing_reader() {
        let input = b"Hello, world!";
        let mut hasher = HashingReader::new(Cursor::new(&input));

        let mut output = Vec::new();
        hasher.read_to_end(&mut output).unwrap();
        assert_eq!(input, &*output);
        assert_eq!(
            hasher.hash(),
            "315f5bdb76d078c43b8ac0064e4a0164612b1fce77c869345bfc94c75894edd3"
        );
    }

    /// Validates that refresh_ab_volumes initializes HostStatus correctly.
    #[test]
    fn test_refresh_ab_volumes_yaml() {
        let host_config = HostConfiguration {
            storage: StorageConfig {
                ab_update: Some(AbUpdateConfig {
                    volume_pairs: vec![AbVolumePairConfig {
                        id: "ab".into(),
                        volume_a_id: "a".to_string(),
                        volume_b_id: "b".to_string(),
                    }],
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut host_status = HostStatus::default();

        refresh_ab_volumes(&mut host_status, &host_config);
        assert!(host_status.storage.ab_update.is_some());
        assert!(host_status
            .storage
            .ab_update
            .as_ref()
            .unwrap()
            .volume_pairs
            .contains_key("ab"));
    }

    #[test]
    fn test_get_undeployed_images() {
        let mut host_config = HostConfiguration {
            storage: StorageConfig {
                mount_points: vec![
                    MountPoint {
                        path: PathBuf::from("/boot"),
                        target_id: "boot".to_string(),
                        filesystem: "fat32".to_string(),
                        options: vec![],
                    },
                    MountPoint {
                        path: PathBuf::from("/"),
                        target_id: "root".to_string(),
                        filesystem: "ext4".to_string(),
                        options: vec![],
                    },
                ],
                images: vec![
                    Image {
                        url: "http://example.com/esp.img".to_string(),
                        target_id: "boot".to_string(),
                        format: ImageFormat::RawZstd,
                        sha256: ImageSha256::Checksum("foobar".to_string()),
                    },
                    Image {
                        url: "http://example.com/image1.img".to_string(),
                        target_id: "root".to_string(),
                        format: ImageFormat::RawZstd,
                        sha256: ImageSha256::Ignored,
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                disks: btreemap! {
                    "foo".to_string() => Disk {
                        uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap(),
                        path: PathBuf::from("/dev/sda"),
                        capacity: 10,
                        contents: BlockDeviceContents::Initialized,
                        partitions: vec![
                            Partition {
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000001")
                                    .unwrap(),
                                path: PathBuf::from("/dev/sda1"),
                                id: "boot".to_string(),
                                start: 1,
                                end: 3,
                                ty: PartitionType::Esp,
                                contents: BlockDeviceContents::Image {
                                    url: "http://example.com/esp.img".to_string(),
                                    sha256: "foobar".to_string(),
                                    length: 100,
                                },
                            },
                            Partition {
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000002")
                                    .unwrap(),
                                path: PathBuf::from("/dev/sda2"),
                                id: "root".to_string(),
                                start: 4,
                                end: 10,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Image {
                                    url: "http://example.com/image1.img".to_string(),
                                    sha256: "foobar".to_string(),
                                    length: 100,
                                },
                            },
                        ],
                    },
                },
                mount_points: btreemap! {
                    "boot".to_string() => MountPointStatus {
                        path: PathBuf::from("/boot"),
                        filesystem: "fat32".to_string(),
                        options: vec![],
                    },
                    "root".to_string() => MountPointStatus {
                        path: PathBuf::from("/"),
                        filesystem: "ext4".to_string(),
                        options: vec![],
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // should be zero, as images are matching and hash is ignored
        assert_eq!(
            get_undeployed_images(&host_status, &host_config, false).len(),
            0
        );

        // should be zero, as images and hashes are matching
        host_config.storage.images[0].sha256 = ImageSha256::Checksum("foobar".to_string());
        assert_eq!(
            get_undeployed_images(&host_status, &host_config, false).len(),
            0
        );

        // should be one, as image hash is different
        host_config.storage.images[0].sha256 = ImageSha256::Checksum("barfoo".to_string());
        assert_eq!(
            get_undeployed_images(&host_status, &host_config, false),
            vec![&Image {
                url: "http://example.com/esp.img".to_string(),
                target_id: "boot".to_string(),
                format: ImageFormat::RawZstd,
                sha256: ImageSha256::Checksum("barfoo".to_string()),
            }]
        );

        // should be one, as image url is different
        host_config.storage.images[0].sha256 = ImageSha256::Ignored;
        host_config.storage.images[0].url = "http://example.com/image2.img".to_string();
        assert_eq!(
            get_undeployed_images(&host_status, &host_config, false),
            vec![&Image {
                url: "http://example.com/image2.img".to_string(),
                target_id: "boot".to_string(),
                format: ImageFormat::RawZstd,
                sha256: ImageSha256::Ignored,
            }]
        );

        // could be zero, as despite the url being different, the hash is the
        // same; for now though we reimage to be safe, hence 1
        host_config.storage.images[0].sha256 = ImageSha256::Checksum("foobar".to_string());
        assert_eq!(
            get_undeployed_images(&host_status, &host_config, false),
            vec![&Image {
                url: "http://example.com/image2.img".to_string(),
                target_id: "boot".to_string(),
                format: ImageFormat::RawZstd,
                sha256: ImageSha256::Checksum("foobar".to_string()),
            }]
        );

        // should be 2, as the image is not initialized and the other is from
        // the previous case
        host_status.storage.disks.get_mut("foo").unwrap().partitions[1].contents =
            BlockDeviceContents::Unknown;
        assert_eq!(
            get_undeployed_images(&host_status, &host_config, false),
            vec![
                &Image {
                    url: "http://example.com/image2.img".to_string(),
                    target_id: "boot".to_string(),
                    format: ImageFormat::RawZstd,
                    sha256: ImageSha256::Checksum("foobar".to_string()),
                },
                &Image {
                    url: "http://example.com/image1.img".to_string(),
                    target_id: "root".to_string(),
                    format: ImageFormat::RawZstd,
                    sha256: ImageSha256::Ignored,
                }
            ]
        );

        // root config is not matching root status
        let host_config = HostConfiguration {
            storage: StorageConfig {
                mount_points: vec![
                    MountPoint {
                        path: PathBuf::from("/boot"),
                        target_id: "boot".to_string(),
                        filesystem: "fat32".to_string(),
                        options: vec![],
                    },
                    MountPoint {
                        path: PathBuf::from("/"),
                        target_id: "root".to_string(),
                        filesystem: "ext4".to_string(),
                        options: vec![],
                    },
                ],
                images: vec![
                    Image {
                        url: "http://example.com/esp.img".to_string(),
                        target_id: "boot".to_string(),
                        format: ImageFormat::RawZstd,
                        sha256: ImageSha256::Checksum("foobar".to_string()),
                    },
                    Image {
                        url: "http://example.com/image1.img".to_string(),
                        target_id: "root".to_string(),
                        format: ImageFormat::RawZstd,
                        sha256: ImageSha256::Ignored,
                    },
                ],
                ab_update: Some(AbUpdateConfig {
                    volume_pairs: vec![AbVolumePairConfig {
                        id: "root".into(),
                        volume_a_id: "root-a".to_string(),
                        volume_b_id: "root-b".to_string(),
                    }],
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                disks: btreemap! {
                    "foo".into() => Disk {
                        uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap(),
                        path: PathBuf::from("/dev/sda"),
                        capacity: 10,
                        contents: BlockDeviceContents::Initialized,
                        partitions: vec![
                            Partition {
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000001")
                                    .unwrap(),
                                path: PathBuf::from("/dev/sda1"),
                                id: "boot".to_string(),
                                start: 1,
                                end: 3,
                                ty: PartitionType::Esp,
                                contents: BlockDeviceContents::Image {
                                    url: "http://example.com/esp.img".to_string(),
                                    sha256: "foobar".to_string(),
                                    length: 100,
                                },
                            },
                            Partition {
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000002")
                                    .unwrap(),
                                path: PathBuf::from("/dev/sda2"),
                                id: "root-b".to_string(),
                                start: 4,
                                end: 10,
                                ty: PartitionType::Root,
                                contents: BlockDeviceContents::Image {
                                    url: "http://example.com/image1.img".to_string(),
                                    sha256: "foobar".to_string(),
                                    length: 100,
                                },
                            },
                        ],
                    },
                },
                mount_points: btreemap! {
                    "boot".to_string() => MountPointStatus {
                        path: PathBuf::from("/boot"),
                        filesystem: "fat32".to_string(),
                        options: vec![],
                    },
                    "root".to_string() => MountPointStatus {
                        path: PathBuf::from("/"),
                        filesystem: "ext4".to_string(),
                        options: vec![],
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            get_undeployed_images(&host_status, &host_config, false),
            vec![&Image {
                url: "http://example.com/image1.img".to_string(),
                target_id: "root".to_string(),
                format: ImageFormat::RawZstd,
                sha256: ImageSha256::Ignored,
            }]
        );

        assert_eq!(
            get_undeployed_images(&host_status, &host_config, true),
            // Vec::<&Image>::new()
            vec![&Image {
                url: "http://example.com/image1.img".to_string(),
                target_id: "root".to_string(),
                format: ImageFormat::RawZstd,
                sha256: ImageSha256::Ignored,
            }]
        );

        // with a/b update, we should get ...

        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        host_status.storage.ab_update = Some(AbUpdate {
            active_volume: Some(AbVolumeSelection::VolumeA),
            volume_pairs: [(
                "root".to_string(),
                AbVolumePair {
                    volume_a_id: "root-a".to_string(),
                    volume_b_id: "root-b".to_string(),
                },
            )]
            .iter()
            .map(|p| {
                (
                    p.0.clone(),
                    AbVolumePair {
                        volume_a_id: p.1.volume_a_id.clone(),
                        volume_b_id: p.1.volume_b_id.clone(),
                    },
                )
            })
            .collect(),
        });

        assert_eq!(
            get_undeployed_images(&host_status, &host_config, false),
            Vec::<&Image>::new()
        );

        assert_eq!(
            get_undeployed_images(&host_status, &host_config, true),
            vec![&Image {
                url: "http://example.com/image1.img".to_string(),
                target_id: "root".to_string(),
                format: ImageFormat::RawZstd,
                sha256: ImageSha256::Ignored,
            }]
        );
    }

    /// Validates logic for setting block device contents
    #[test]
    fn test_set_host_status_block_device_contents() {
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "efi".to_owned(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000")
                                    .unwrap(),
                            },
                            Partition {
                                id: "root".to_owned(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                                contents: BlockDeviceContents::Unknown,
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000")
                                    .unwrap(),
                            },
                            Partition {
                                id: "rootb".to_owned(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                                contents: BlockDeviceContents::Unknown,
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000")
                                    .unwrap(),
                            },
                        ],
                    },
                    "data".into() => Disk {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap(),
                        capacity: 1000,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![],
                    },
                },
                ab_update: Some(AbUpdate {
                    active_volume: None,
                    volume_pairs: btreemap! {
                        "osab".to_owned() => AbVolumePair {
                            volume_a_id: "root".to_owned(),
                            volume_b_id: "rootb".to_owned(),
                        },
                    },
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .contents,
            BlockDeviceContents::Unknown
        );
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(0)
                .unwrap()
                .contents,
            BlockDeviceContents::Unknown
        );
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(1)
                .unwrap()
                .contents,
            BlockDeviceContents::Unknown
        );

        // test for disks
        let contents = BlockDeviceContents::Zeroed;
        set_host_status_block_device_contents(&mut host_status, &"os".to_owned(), contents.clone())
            .unwrap();
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .contents,
            contents.clone()
        );

        // test for partitions
        set_host_status_block_device_contents(
            &mut host_status,
            &"efi".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(0)
                .unwrap()
                .contents,
            contents.clone()
        );

        // test for ab volumes
        set_host_status_block_device_contents(
            &mut host_status,
            &"osab".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(1)
                .unwrap()
                .contents,
            contents.clone()
        );

        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);

        set_host_status_block_device_contents(
            &mut host_status,
            &"osab".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(1)
                .unwrap()
                .contents,
            contents.clone()
        );

        host_status
            .storage
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);

        set_host_status_block_device_contents(
            &mut host_status,
            &"osab".to_owned(),
            contents.clone(),
        )
        .unwrap();
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(2)
                .unwrap()
                .contents,
            contents.clone()
        );

        // test failure when missing id is provided
        assert_eq!(
            set_host_status_block_device_contents(
                &mut host_status,
                &"foorbar".to_owned(),
                contents.clone()
            )
            .err()
            .unwrap()
            .to_string(),
            "No block device with id 'foorbar' found"
        );
    }

    /// Validates logic for querying disks and partitions.
    #[test]
    fn test_get_disk_partition_mut() {
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            storage: Storage {
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "efi".to_owned(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000")
                                    .unwrap(),
                            },
                            Partition {
                                id: "root".to_owned(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                                contents: BlockDeviceContents::Unknown,
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000")
                                    .unwrap(),
                            },
                            Partition {
                                id: "rootb".to_owned(),
                                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                                contents: BlockDeviceContents::Unknown,
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                uuid: Uuid::parse_str("00000000-0000-0000-0000-000000000000")
                                    .unwrap(),
                            },
                        ],
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let disk_mut = get_disk_mut(&mut host_status, &"os".to_owned());
        disk_mut.unwrap().contents = BlockDeviceContents::Zeroed;
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .contents,
            BlockDeviceContents::Zeroed
        );

        let partition_mut = get_partition_mut(&mut host_status, &"efi".to_owned());
        partition_mut.unwrap().contents = BlockDeviceContents::Initialized;
        assert_eq!(
            host_status
                .storage
                .disks
                .get(&"os".to_owned())
                .unwrap()
                .partitions
                .get(0)
                .unwrap()
                .contents,
            BlockDeviceContents::Initialized
        );
    }
}
