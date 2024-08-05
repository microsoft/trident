use std::{ffi::CString, fs::File, os::fd::AsRawFd};

use anyhow::{Context, Error};
use log::info;
use nix::NixPath;
use trident_api::error::TridentResultExt;

use crate::modules::mount_root::NewrootMount;

#[allow(unused)]
pub fn kexec(mut root_mount: NewrootMount, args: &str) -> Result<(), Error> {
    let root = root_mount.path().to_str().context(format!(
        "Non-utf8 mount point: {}",
        root_mount.path().display()
    ))?;

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
    let kernel = File::open(kernel_path)?;
    let initrd = File::open(initrd_path)?;
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

    root_mount
        .unmount_all()
        .unstructured("Failed to unmount new root")?;

    // Kexec into image.
    info!("Rebooting system");
    let r = unsafe { libc::reboot(libc::LINUX_REBOOT_CMD_KEXEC) };
    if r < 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    unreachable!()
}
