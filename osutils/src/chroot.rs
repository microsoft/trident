use std::{
    fs,
    os::{
        fd::{IntoRawFd, RawFd},
        unix,
    },
    path::Path,
};

use anyhow::{Context, Error};
use log::info;
use sys_mount::{Mount, MountFlags, Unmount, UnmountFlags};

// TODO: Implement drop for Chroot that panics if the chroot has not been
// exited. Tracked by: https://dev.azure.com/mariner-org/ECF/_workitems/edit/6265

/// Create a chroot environment.
///
/// Note: Dropping this object does *not* exit the chroot. You must call `exit()` manually.
pub struct Chroot {
    rootfd: RawFd,
    mounts: Vec<Mount>,
}
impl Chroot {
    /// Mount special directories ('/dev', '/proc', and '/sys') and enter chroot.
    fn enter(path: &Path, mount_special_dirs: bool) -> Result<Self, Error> {
        // Mount special dirs.
        let mounts = if mount_special_dirs {
            info!("Mounting special directories");
            vec![
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
                Mount::builder()
                    .fstype("tmpfs")
                    .flags(MountFlags::empty())
                    .mount("tmpfs", path.join("tmp"))
                    .context("Failed to mount '/tmp' for chroot")?,
            ]
        } else {
            Vec::new()
        };

        // Enter the chroot.
        info!("Entering chroot");
        let rootfd = fs::File::open("/")
            .context("Failed to open '/'")?
            .into_raw_fd();
        unix::fs::chroot(path).context("Failed to enter chroot")?;
        std::env::set_current_dir("/")
            .context("Failed to set current directory to be inside chroot")?;

        Ok(Self { rootfd, mounts })
    }

    /// Exit the chroot environment and unmount special directories.
    #[allow(unused)]
    pub fn exit(self) -> Result<(), Error> {
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

pub fn enter_update_chroot(root_mount_path: &Path) -> Result<Chroot, Error> {
    Chroot::enter(root_mount_path, true).context("Failed to enter updated OS chroot")
}

pub fn enter_host_chroot(root_mount_path: &Path) -> Result<Chroot, Error> {
    Chroot::enter(root_mount_path, false).context("Failed to enter host chroot")
}
