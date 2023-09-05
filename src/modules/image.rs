use std::{
    ffi::CString,
    fs::{self, File},
    io::{self, BufWriter, Read},
    os::{fd::IntoRawFd, unix},
    path::Path,
};

use anyhow::{bail, Context, Error};
use log::info;
use nix::NixPath;
use reqwest::Url;
use sha2::Digest;
use sys_mount::{Mount, MountFlags, Unmount, UnmountDrop, UnmountFlags};

use trident_api::{
    config::{HostConfiguration, ImageFormat},
    status::{HostStatus, PartitionContents, UpdateKind},
};

use crate::modules::Module;

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
    for (image_type, image) in &host_config.imaging.images {
        let partition_type = image_type.to_part_type(true); // TODO: Properly pick A/B partition

        // Iterate over all partitions on all disks to find the first one with a matching type.
        let partition = host_status
            .storage
            .disks
            .values_mut()
            .flat_map(|d| &mut d.partitions)
            .find(|p| p.ty == partition_type)
            .ok_or_else(|| anyhow::anyhow!("No partition of type {:?} found", partition_type))?;

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
            .open(&partition.path)
            .context(format!("Failed to open '{}'", partition.path.display()))?;

        // Buffer small writes to the disk, ensuring we write blocks of at least 4MB.
        let mut file = BufWriter::with_capacity(4 << 20, file);

        // Mark the partition as having unknown contents in case the write operation is interrupted.
        partition.contents = PartitionContents::Unknown;

        // Decompress the image and write it to the partition, making sure not to write past the end.
        let bytes_copied = io::copy(
            &mut (&mut decoder).take(partition.end - partition.start),
            &mut file,
        )
        .context("Failed to copy image")?;

        info!(
            "Copied {} bytes to {}",
            bytes_copied,
            partition.path.display()
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
        partition.contents = PartitionContents::Image {
            sha256: computed_sha256.clone(),
            length: bytes_copied,
        };

        // Confirm that the hash matches what we expected.
        if computed_sha256 != image.sha256 {
            bail!(
                "SHA256 mismatch for disk image: expected {}, got {}",
                image.sha256,
                computed_sha256
            );
        }
    }

    Ok(())
}

pub fn mount_partition(partition: &Path) -> Result<UnmountDrop<Mount>, Error> {
    fs::create_dir_all("/partitionMount")?;
    info!("Mounting disk");
    Ok(Mount::builder()
        .fstype("ext4")
        .mount(partition, "/partitionMount")?
        .into_unmount_drop(UnmountFlags::DETACH))
}

pub fn chroot_run<T, F: FnOnce() -> Result<T, Error>>(partition: &Path, f: F) -> Result<T, Error> {
    let _mount = mount_partition(partition).context(format!(
        "Failed to mount partition '{}'",
        partition.display()
    ))?;

    // Mount special dirs.
    info!("Mounting special directories");
    let _mount = Mount::builder()
        .fstype("devtmpfs")
        .flags(MountFlags::RDONLY)
        .mount("devtmpfs", "/partitionMount/dev")
        .context("Failed to mount '/dev' for chroot")?
        .into_unmount_drop(UnmountFlags::empty());
    let _mount = Mount::builder()
        .fstype("proc")
        .flags(MountFlags::RDONLY)
        .mount("proc", "/partitionMount/proc")
        .context("Failed to mount '/proc' for chroot")?
        .into_unmount_drop(UnmountFlags::empty());
    let _mount = Mount::builder()
        .fstype("sysfs")
        .flags(MountFlags::RDONLY)
        .mount("sysfs", "/partitionMount/sys")
        .context("Failed to mount '/sys' for chroot")?
        .into_unmount_drop(UnmountFlags::empty());

    // Enter the chroot.
    info!("Entering chroot");
    let rootfd = fs::File::open("/")
        .context("Failed to open '/'")?
        .into_raw_fd();
    unix::fs::chroot("/partitionMount").context("Failed to enter chroot")?;
    std::env::set_current_dir("/")
        .context("Failed to set current directory to be inside chroot")?;

    // Run the closure.
    let t = f()?;

    // Exit the chroot.
    nix::unistd::fchdir(rootfd).context("Failed to exit chroot")?;
    unix::fs::chroot(".").context("Failed to set current directory out of chroot")?;
    info!("Exited chroot");

    Ok(t)
}

pub fn chroot_exec(partition: &Path, script: &str) -> Result<(), Error> {
    chroot_run(partition, || {
        info!("Writing cexecScript");
        fs::write("/cexecScript", script.as_bytes())?;

        info!("Running cexecScript");
        let status = std::process::Command::new("/bin/bash")
            .arg("/cexecScript")
            .status()?;
        info!("Script exited with status: {}", status);

        fs::remove_file("/cexecScript")?;
        Ok(())
    })
}

pub fn kexec(partition: &Path, args: &str) -> Result<(), Error> {
    let _mount = mount_partition(partition)?;

    info!("Searching for kernel and initrd");
    let kernel_path = glob::glob("/partitionMount/boot/vmlinuz-*")?
        .next()
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No kernel found",
        ))??;
    let initrd_path = glob::glob("/partitionMount/boot/initrd.img-*")?
        .next()
        .ok_or(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "No initrd found",
        ))??;

    info!("Opening kernel and initrd");
    let kernel = fs::File::open(kernel_path)?.into_raw_fd();
    let initrd = fs::File::open(initrd_path)?.into_raw_fd();
    let args = CString::new(args)?;

    // Run kexec file load.
    info!("Loading kernel");
    let r = unsafe {
        libc::syscall(
            libc::SYS_kexec_file_load,
            kernel,
            initrd,
            args.len() + 1,
            args.as_ptr(),
            0,
        )
    };
    if r < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    // Kexec into image.
    info!("Rebooting system");
    let r = unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_KEXEC) };
    if r < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    unreachable!()
}

#[derive(Default, Debug)]
pub struct ImageModule;
impl Module for ImageModule {
    fn name(&self) -> &'static str {
        "image"
    }

    fn refresh_host_status(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
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
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        Some(UpdateKind::HotPatch)
    }

    fn reconcile(
        &mut self,
        _host_status: &mut HostStatus,
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn hashing_reader() {
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
}
