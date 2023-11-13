use std::{
    fs,
    os::{
        fd::{IntoRawFd, RawFd},
        unix,
    },
    path::Path,
    process::Command,
};

use anyhow::{Context, Error};
use log::info;
use sys_mount::{Mount, MountFlags, Unmount, UnmountFlags};

/// Create a chroot environment.
///
/// Note: Dropping this object does *not* exit the chroot. You must call `exit()` manually.
pub(super) struct Chroot {
    rootfd: RawFd,
    mounts: Vec<Mount>,
}
impl Chroot {
    /// Mount special directories ('/dev', '/proc', and '/sys') and enter chroot.
    fn enter(path: &Path) -> Result<Self, Error> {
        // Mount special dirs.
        info!("Mounting special directories");
        let mounts = vec![
            Mount::builder()
                .fstype("devtmpfs")
                .flags(MountFlags::RDONLY)
                .mount("devtmpfs", path.join("dev"))
                .context("Failed to mount '/dev' for chroot")?,
            Mount::builder()
                .fstype("proc")
                .flags(MountFlags::RDONLY)
                .mount("proc", path.join("proc"))
                .context("Failed to mount '/proc' for chroot")?,
            Mount::builder()
                .fstype("sysfs")
                .flags(MountFlags::RDONLY)
                .mount("sysfs", path.join("sys"))
                .context("Failed to mount '/sys' for chroot")?,
        ];

        // Enter the chroot.
        info!("Entering chroot");
        let rootfd = fs::File::open("/")
            .context("Failed to open '/'")?
            .into_raw_fd();
        unix::fs::chroot("/partitionMount").context("Failed to enter chroot")?;
        std::env::set_current_dir("/")
            .context("Failed to set current directory to be inside chroot")?;

        Ok(Self { rootfd, mounts })
    }

    /// Exit the chroot environment and unmount special directories.
    #[allow(unused)]
    pub(super) fn exit(self) -> Result<(), Error> {
        // Exit the chroot.
        nix::unistd::fchdir(self.rootfd).context("Failed to exit chroot")?;
        unix::fs::chroot(".").context("Failed to set current directory out of chroot")?;
        info!("Exited chroot");

        info!("Unmounting special directories");
        for mount in self.mounts {
            mount.unmount(UnmountFlags::empty())?;
        }
        Ok(())
    }
}

pub(super) fn enter_chroot(root_mount_path: &Path) -> Result<Chroot, Error> {
    Chroot::enter(root_mount_path).context("Failed to enter updated filesystem chroot")
}

pub(super) fn unmount_target_volumes(mount_path: &Path) -> Result<(), Error> {
    let mount_unit = String::from_utf8(
        crate::run_command(
            Command::new("systemd-escape")
                .arg("-p")
                .arg("--suffix=mount")
                .arg(mount_path),
        )
        .context("Failed to escape root mount path")?
        .stdout,
    )
    .context("Failed to parse systemd-escape output as UTF-8")?
    .trim()
    .to_owned();
    crate::run_command(Command::new("systemctl").arg("stop").arg(mount_unit))
        .context("Failed to safely unmount target root partition.")?;
    Ok(())
}
