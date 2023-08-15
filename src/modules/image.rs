use std::{
    ffi::CString,
    fs,
    io::{self, BufWriter, Cursor, Read},
    os::{fd::IntoRawFd, unix},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::{error, info, warn};
use nix::NixPath;
use sha2::Digest;
use sys_mount::{Mount, MountFlags, Unmount, UnmountDrop, UnmountFlags};

use crate::{
    config::{HostConfig, Image, PartImageType, PartitionType},
    modules::Module,
    status::{self, HostStatus, PartitionContents, UpdateKind},
};

pub fn download_image(image: &Image) -> Result<Vec<u8>, Error> {
    // Download and decompress the image.
    let body = reqwest::blocking::get(&image.url)
        .and_then(|g| g.bytes())
        .context(format!("Failed to download {}", image.url))?;
    info!("Downloaded {} bytes", body.len());

    // Verify the image.
    let computed_sha256 = {
        let mut hasher = sha2::Sha256::new();
        hasher.update(&body);
        format!("{:x}", hasher.finalize())
    };
    if computed_sha256 != image.sha256 {
        bail!(
            "SHA256 mismatch for disk image: expected {}, got {}",
            image.sha256,
            computed_sha256
        );
    } else {
        info!("Validated image hash");
    }

    Ok(body.into())
}

pub fn write_image(
    host_status: &mut HostStatus,
    image: &Image,
    contents: &[u8],
) -> Result<(), Error> {
    // Decompress the first 4MB of the image to get the GPT header.
    let mut image_prefix = Vec::new();
    zstd::stream::read::Decoder::new(contents)
        .context("Failed to start decompression")?
        .take(4 << 20)
        .read_to_end(&mut image_prefix)
        .context("Failed to decompress image")?;

    // TODO: Use a better way to determine the block size.
    let block_size = gpt::disk::DEFAULT_SECTOR_SIZE;

    // Read the GPT header and partitions.
    let header =
        gpt::header::read_header_from_arbitrary_device(&mut Cursor::new(&image_prefix), block_size)
            .context("Failed to read image header")?;
    let partitions =
        gpt::partition::file_read_partitions(&mut Cursor::new(&image_prefix), &header, block_size)
            .context("Failed to read image partitions")?;
    let mut partitions: Vec<_> = partitions.into_values().collect();
    partitions.sort_by_key(|p| p.first_lba);

    // Write each partition image to disk.
    let mut position = 0;
    let mut decoder = zstd::stream::read::Decoder::new(contents)?;
    for (index, partition) in partitions.iter().enumerate() {
        info!(
            "Writing partition {} (size = {}MiB)",
            partition.name,
            partition.bytes_len(block_size)? >> 20
        );
        info!("partition UUID = {:?}", partition.part_type_guid);

        let start = partition.bytes_start(block_size)?;
        if position > start {
            error!("Image file contains overlapping partitions");
            continue;
        }

        io::copy(&mut (&mut decoder).take(start - position), &mut io::sink())?;
        position = start;

        let ty = match &partition.part_type_guid {
            &gpt::partition_types::EFI => PartImageType::Esp,
            &gpt::partition_types::LINUX_FS | &gpt::partition_types::LINUX_ROOT_X64 => {
                PartImageType::Root
            }
            t => {
                info!("Ignoring partition within image with unknown type: {t:?}");
                continue;
            }
        };
        let ty = ty.to_part_type(true);

        let Ok((disk_path, part_index, part_path)) = find_partition(&host_status.storage, ty) else {
            warn!("Skipping write of partition with type {:?}", ty);
            continue;
        };

        let partition_len = partition
            .bytes_len(block_size)
            .context("Disk image is invalid")?;
        position += partition_len;

        let mut file = BufWriter::with_capacity(
            4 << 20,
            fs::File::options()
                .write(true)
                .open(&part_path)
                .context(format!("Failed to open '{}'", part_path.display()))?,
        );
        io::copy(&mut (&mut decoder).take(partition_len), &mut file)
            .context("Failed to copy image")?;
        file.into_inner()
            .context("Failed to flush")?
            .sync_all()
            .context("Failed to sync")?;

        host_status
            .storage
            .disks
            .get_mut(&disk_path)
            .unwrap()
            .partitions[part_index]
            .contents = PartitionContents::SubImage {
            image_sha256: image.sha256.clone(),
            subimage_index: index,
        }
    }

    Ok(())
}

/// Returns the disk path, partition index, and path of the partition with the given type.
fn find_partition(
    storage: &status::Storage,
    ty: PartitionType,
) -> Result<(PathBuf, usize, PathBuf), Error> {
    for (disk_path, disk) in &storage.disks {
        for (part_index, part) in disk.partitions.iter().enumerate() {
            if part.ty == ty {
                return Ok((disk_path.clone(), part_index, part.path.clone()));
            }
        }
    }
    bail!("No partition of type {:?} found on disks", ty);
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
    let _mount = mount_partition(partition)?;

    // Mount special dirs.
    info!("Mounting special directories");
    let _mount = Mount::builder()
        .fstype("devtmpfs")
        .flags(MountFlags::RDONLY)
        .mount("devtmpfs", "/partitionMount/dev")?
        .into_unmount_drop(UnmountFlags::empty());
    let _mount = Mount::builder()
        .fstype("proc")
        .flags(MountFlags::RDONLY)
        .mount("proc", "/partitionMount/proc")?;
    let _mount = Mount::builder()
        .fstype("sysfs")
        .flags(MountFlags::RDONLY)
        .mount("sysfs", "/partitionMount/sys")?
        .into_unmount_drop(UnmountFlags::empty());

    // Enter the chroot.
    info!("Entering chroot");
    let rootfd = fs::File::open("/")?.into_raw_fd();
    unix::fs::chroot("/partitionMount")?;
    std::env::set_current_dir("/")?;

    // Run the closure.
    let t = f()?;

    // Exit the chroot.
    nix::unistd::fchdir(rootfd)?;
    unix::fs::chroot(".")?;
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
        _host_config: &HostConfig,
    ) -> Result<(), Error> {
        Ok(())
    }

    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfig,
    ) -> Option<UpdateKind> {
        Some(UpdateKind::HotPatch)
    }

    fn reconcile(
        &mut self,
        _host_status: &mut HostStatus,
        _host_config: &HostConfig,
    ) -> Result<(), Error> {
        Ok(())
    }
}
