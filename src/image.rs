use std::{
    ffi::CString,
    fs,
    io::Write,
    os::{fd::IntoRawFd, unix},
    path::Path,
};

use log::{error, info};
use nix::NixPath;
use sha2::Digest;
use sys_mount::{Mount, MountFlags, Unmount, UnmountDrop, UnmountFlags};

pub async fn write_image(
    disk: &Path,
    url: &str,
    sha256: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Download and decompress the image.
    let body = reqwest::get(url).await?.bytes().await?;
    info!("Downloaded {} bytes", body.len());

    // Verify the image.
    let computed_sha256 = {
        let mut hasher = sha2::Sha256::new();
        hasher.update(&body);
        format!("{:x}", hasher.finalize())
    };
    if computed_sha256 != sha256 {
        error!(
            "SHA256 mismatch for disk image: expected {}, got {}",
            sha256, computed_sha256
        );
        return Err(format!(
            "SHA256 mismatch: expected {}, got {}",
            sha256, computed_sha256
        )
        .into());
    } else {
        info!("Validated image hash");
    }

    // Stream the image to the target disk.
    let mut device_file = fs::File::options().write(true).open(disk)?;
    {
        let mut writer = std::io::BufWriter::with_capacity(4 << 20, &mut device_file);
        zstd::stream::copy_decode(&mut &*body, &mut writer)?;
        writer.flush()?;
    }
    device_file.sync_all()?;
    Ok(())
}

pub fn mount_partition(partition: &Path) -> Result<UnmountDrop<Mount>, Box<dyn std::error::Error>> {
    fs::create_dir_all("/partitionMount")?;
    info!("Mounting disk");
    Ok(Mount::builder()
        .fstype("ext4")
        .mount(partition, "/partitionMount")?
        .into_unmount_drop(UnmountFlags::DETACH))
}

pub async fn chroot_exec(partition: &Path, script: &str) -> Result<(), Box<dyn std::error::Error>> {
    let _mount = mount_partition(partition)?;

    // Write cexec script.
    info!("Writing cexecScript");
    fs::write("/partitionMount/cexecScript", script.as_bytes())?;

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

    // Run script.
    info!("Running cexecScript");
    let status = std::process::Command::new("/bin/bash")
        .arg("/cexecScript")
        .status()?;
    info!("Script exited with status: {}", status);

    // Exit the chroot.
    nix::unistd::fchdir(rootfd)?;
    unix::fs::chroot(".")?;
    info!("Exited chroot");

    Ok(())
}

pub async fn kexec(partition: &Path, args: &str) -> Result<(), Box<dyn std::error::Error>> {
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
