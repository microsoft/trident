use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, BufWriter, Read},
    os::fd::AsRawFd,
    path::Path,
    process::Command,
};

use anyhow::{bail, Context, Error};
use log::info;
use nix::NixPath;
use reqwest::Url;
use sha2::Digest;

use trident_api::{
    config::{HostConfiguration, Image, ImageFormat},
    status::{
        AbUpdate, AbVolumePair, AbVolumeSelection, BlockDeviceContents, HostStatus, UpdateKind,
    },
};

use crate::modules::{storage::tabfile::TabFile, unmount_target_volumes, Module};
use crate::{get_block_device, run_command, set_host_status_block_device_contents};

const HASH_IGNORED: &str = "ignored";

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

pub(crate) fn stream_images(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    for image in get_images_for_updating(host_status, host_config) {
        let block_device = get_block_device(host_status, &image.target_id).context(format!(
            "No block device with id '{}' found",
            image.target_id
        ))?;

        // TODO: Add more options for download sources
        let image_url = Url::parse(image.url.as_str())
            .context(format!("Failed to parse image URL '{}'", image.url))?;
        let stream: Box<dyn Read> = if image_url.scheme() == "file" {
            Box::new(File::open(image_url.path()).context(format!("Failed to open {}", image.url))?)
        } else if image_url.scheme() == "http" || image_url.scheme() == "https" {
            Box::new(
                reqwest::blocking::get(image_url)
                    .context(format!("Failed to download {}", image.url))?,
            )
        } else if image_url.scheme() == "oci" {
            todo!("OCI image support")
        } else {
            bail!("Unsupported URL scheme")
        };
        let mut stream = HashingReader::new(stream);

        let mut decoder = match image.format {
            ImageFormat::RawZstd => zstd::stream::read::Decoder::new(&mut stream)?,
        };

        // Open the partition for writing.
        let file = fs::File::options()
            .write(true)
            .open(&block_device.path)
            .context(format!("Failed to open '{}'", block_device.path.display()))?;

        // Buffer small writes to the disk, ensuring we write blocks of at least 4MB.
        let mut file = BufWriter::with_capacity(4 << 20, file);

        // Mark the block device as having unknown contents in case the write operation is interrupted.
        set_host_status_block_device_contents(
            host_status,
            &image.target_id,
            BlockDeviceContents::Unknown,
        )?;

        // Decompress the image and write it to the block device, making sure not to write past the end.
        let bytes_copied = io::copy(&mut (&mut decoder).take(block_device.size), &mut file)
            .context("Failed to copy image")?;

        info!(
            "Copied {} bytes to {}",
            bytes_copied,
            block_device.path.display()
        );

        file.into_inner()
            .context("Failed to flush")?
            .sync_all()
            .context("Failed to sync")?;

        // Attempt to read an additional byte from the stream to see whether the whole image was
        // consumed.
        if decoder.read(&mut [0])? != 0 {
            bail!("Image is larger than destination");
        }

        let computed_sha256 = stream.hash();
        set_host_status_block_device_contents(
            host_status,
            &image.target_id,
            BlockDeviceContents::Image {
                sha256: computed_sha256.clone(),
                length: bytes_copied,
                url: image.url.clone(),
            },
        )?;

        // Confirm that the hash matches what we expected.
        if image.sha256 == HASH_IGNORED {
            info!("Ignoring SHA256 for image from '{}'", image.url);
        } else if computed_sha256 != image.sha256 {
            bail!(
                "SHA256 mismatch for disk image {}: expected {}, got {}",
                image.url,
                image.sha256,
                computed_sha256
            );
        }
    }

    Ok(())
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

    unmount_target_volumes(mount_path)?;

    // Kexec into image.
    info!("Rebooting system");
    let r = unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_KEXEC) };
    if r < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    unreachable!()
}

pub fn reboot() -> Result<(), Error> {
    // Sync all writes to the filesystem.
    nix::unistd::sync();

    info!("Rebooting system");
    run_command(Command::new("systemctl").arg("reboot")).context("Failed to reboot the host")?;

    unreachable!()
}

pub fn refresh_ab_volumes(host_status: &mut HostStatus, host_config: &HostConfiguration) {
    host_status.imaging.ab_update = host_config.imaging.ab_update.as_ref().map(|ab_update| {
        let ab_volume_pairs = ab_update
            .volume_pairs
            .iter()
            .map(|p| {
                (
                    p.id.clone(),
                    AbVolumePair {
                        id: p.id.clone(),
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
fn get_images_for_updating<'a>(
    host_status: &HostStatus,
    host_config: &'a HostConfiguration,
) -> Vec<&'a Image> {
    host_config
        .imaging
        .images
        .iter()
        .filter(|image| {
            if let Some(bdi) = get_block_device(host_status, &image.target_id) {
                if let BlockDeviceContents::Image { sha256, url, .. } = bdi.contents {
                    if url == image.url && (sha256 == image.sha256 || image.sha256 == HASH_IGNORED)
                    {
                        return false;
                    }
                }
            }
            true
        })
        .collect()
}

#[derive(Default, Debug)]
pub struct ImageModule;
impl Module for ImageModule {
    fn name(&self) -> &'static str {
        "image"
    }

    fn refresh_host_status(&mut self, host_status: &mut HostStatus) -> Result<(), Error> {
        // update root_device_path of the active root volume
        host_status.imaging.root_device_path = Some(
            TabFile::get_device_path(Path::new("/proc/mounts"), Path::new("/"))
                .context("Failed find root mount point")?,
        );

        // if a/b update is enabled
        if let Some(ab_update) = &host_status.imaging.ab_update {
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
                        get_block_device(host_status, &root_device_pair.volume_a_id)
                            .context("Failed to get block device for volume A")?
                            .path;

                    let volume_b_path =
                        get_block_device(host_status, &root_device_pair.volume_b_id)
                            .context("Failed to get block device for volume B")?
                            .path;

                    // update the active volume in the a/b scheme based on what
                    // is the current root volume
                    if let Some(root_device_path) = &host_status.imaging.root_device_path {
                        host_status
                            .imaging
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

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn select_update_kind(
        &self,
        host_status: &HostStatus,
        host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        let update_images = get_images_for_updating(host_status, host_config);
        if update_images.is_empty() {
            None
        } else {
            Some(UpdateKind::AbUpdate)
        }
    }

    fn reconcile(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        refresh_ab_volumes(host_status, host_config);

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;
    use std::io::Cursor;

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
        let host_config_yaml = indoc! {r#"
            storage:
                disks:
            imaging:
                images:
                ab-update:
                    volume-pairs:
                      - id: ab
                        volume-a-id: a
                        volume-b-id: b
        "#};
        let host_config = serde_yaml::from_str::<HostConfiguration>(host_config_yaml).unwrap();
        let mut host_status = HostStatus::default();

        refresh_ab_volumes(&mut host_status, &host_config);
        assert!(host_status.imaging.ab_update.is_some());
        assert!(host_status
            .imaging
            .ab_update
            .as_ref()
            .unwrap()
            .volume_pairs
            .contains_key("ab"));
    }
}
