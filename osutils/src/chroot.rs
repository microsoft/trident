use std::{
    fs,
    os::{
        fd::{IntoRawFd, RawFd},
        unix,
    },
    path::Path,
};

use log::info;
use sys_mount::{Mount, MountFlags, Unmount, UnmountFlags};
use trident_api::error::{ManagementError, ReportError, TridentError, TridentResultExt};

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
    fn enter(path: &Path, mount_special_dirs: bool) -> Result<Self, TridentError> {
        // Mount special dirs.
        let mounts = if mount_special_dirs {
            info!("Mounting special directories");
            vec![
                Mount::builder()
                    .fstype("devtmpfs")
                    .flags(MountFlags::empty())
                    .mount("devtmpfs", path.join("dev"))
                    .structured(ManagementError::ChrootMountSpecial { dir: "/dev" })?,
                Mount::builder()
                    .fstype("proc")
                    .flags(MountFlags::empty())
                    .mount("proc", path.join("proc"))
                    .structured(ManagementError::ChrootMountSpecial { dir: "/proc" })?,
                Mount::builder()
                    .fstype("sysfs")
                    .flags(MountFlags::empty())
                    .mount("sysfs", path.join("sys"))
                    .structured(ManagementError::ChrootMountSpecial { dir: "/sys" })?,
            ]
        } else {
            Vec::new()
        };

        // Enter the chroot.
        info!("Entering chroot");
        let rootfd = fs::File::open("/")
            .structured(ManagementError::ChrootEnter)?
            .into_raw_fd();
        unix::fs::chroot(path).structured(ManagementError::ChrootEnter)?;
        std::env::set_current_dir("/").structured(ManagementError::ChrootEnter)?;

        Ok(Self { rootfd, mounts })
    }

    pub fn execute_and_exit<F, T>(self, f: F) -> Result<T, TridentError>
    where
        F: FnOnce() -> Result<T, TridentError>,
    {
        // Execute the function.
        let result = f();

        // Exit the chroot and return any errors from the function, the exit
        // call, or both.
        match self.exit() {
            Ok(_) => result,
            Err(e2) => match result {
                Ok(_) => Err(e2),
                Err(e) => Err(e.secondary_error_context(e2)),
            },
        }
    }

    /// Exit the chroot environment and unmount special directories.
    fn exit(self) -> Result<(), TridentError> {
        // Exit the chroot.
        nix::unistd::fchdir(self.rootfd).structured(ManagementError::ChrootExit)?;
        unix::fs::chroot(".").structured(ManagementError::ChrootExit)?;
        info!("Exited chroot");

        info!("Unmounting special directories");
        for mount in self.mounts {
            mount
                .unmount(UnmountFlags::empty())
                .structured(ManagementError::ChrootUnmountSpecial)?;
        }
        Ok(())
    }
}

pub fn enter_update_chroot(root_mount_path: &Path) -> Result<Chroot, TridentError> {
    Chroot::enter(root_mount_path, true).message("Failed to enter updated OS chroot")
}

pub fn enter_host_chroot(root_mount_path: &Path) -> Result<Chroot, TridentError> {
    Chroot::enter(root_mount_path, false).message("Failed to enter host chroot")
}
