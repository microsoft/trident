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
use trident_api::error::{ReportError, ServicingError, TridentError, TridentResultExt};

// TODO: Implement drop for Chroot that panics if the chroot has not been
// exited. Tracked by: https://dev.azure.com/mariner-org/ECF/_workitems/edit/6265

/// Create a chroot environment.
///
/// Note: Dropping this object does *not* exit the chroot. You must call `exit()` manually.
#[derive(Debug)]
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
                    .structured(ServicingError::ChrootMountSpecialDir {
                        dir: "/dev".to_string(),
                    })?,
                Mount::builder()
                    .fstype("proc")
                    .flags(MountFlags::empty())
                    .mount("proc", path.join("proc"))
                    .structured(ServicingError::ChrootMountSpecialDir {
                        dir: "/proc".to_string(),
                    })?,
                Mount::builder()
                    .fstype("sysfs")
                    .flags(MountFlags::empty())
                    .mount("sysfs", path.join("sys"))
                    .structured(ServicingError::ChrootMountSpecialDir {
                        dir: "/sys".to_string(),
                    })?,
            ]
        } else {
            Vec::new()
        };

        // Enter the chroot.
        info!("Entering chroot");
        let rootfd = fs::File::open("/")
            .structured(ServicingError::EnterChroot)?
            .into_raw_fd();
        unix::fs::chroot(path).structured(ServicingError::EnterChroot)?;
        std::env::set_current_dir("/").structured(ServicingError::EnterChroot)?;

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
        nix::unistd::fchdir(self.rootfd).structured(ServicingError::ExitChroot)?;
        unix::fs::chroot(".").structured(ServicingError::ExitChroot)?;
        info!("Exited chroot");

        info!("Unmounting special directories");
        for mount in self.mounts {
            mount
                .unmount(UnmountFlags::empty())
                .structured(ServicingError::ChrootUnmountSpecialDir)?;
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

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;
    use std::fs::{self, File};
    use tempfile::tempdir;
    use trident_api::error::ErrorKind;

    #[functional_test(feature = "helpers")]
    fn test_enter_and_exit_chroot() {
        // Create a temporary directory to act as the chroot environment
        let temp_dir = tempdir().unwrap();
        let chroot_path = temp_dir.path().to_path_buf();

        // Create necessary directories for the chroot environment
        fs::create_dir_all(chroot_path.join("dev")).unwrap();
        fs::create_dir_all(chroot_path.join("proc")).unwrap();
        fs::create_dir_all(chroot_path.join("sys")).unwrap();

        // Create a dummy file at /
        File::create(Path::new("/").join("dummy")).unwrap();
        assert!(Path::new("/dummy").exists());

        // Enter the chroot
        let chroot = Chroot::enter(&chroot_path, true).unwrap();

        // Verify we are inside the chroot
        assert!(!chroot_path.join("dev").exists());
        assert!(!chroot_path.join("proc").exists());
        assert!(!chroot_path.join("sys").exists());
        assert!(Path::new("/dev").exists());
        assert!(Path::new("/proc").exists());
        assert!(Path::new("/sys").exists());

        // Verify we cannot access the dummy file from inside of chroot
        assert!(!Path::new("/dummy").exists());

        // Exit the chroot
        chroot.exit().unwrap();

        // Verify that files exist at original paths
        assert!(chroot_path.join("dev").exists());
        assert!(chroot_path.join("proc").exists());
        assert!(chroot_path.join("sys").exists());
        assert!(Path::new("/dummy").exists());
    }

    #[functional_test(feature = "helpers")]
    fn test_enter_update_chroot() {
        // Create a temporary directory to act as the chroot environment
        let temp_dir = tempdir().unwrap();
        let chroot_path = temp_dir.path().to_path_buf();

        // Create necessary directories for the chroot environment
        fs::create_dir_all(chroot_path.join("dev")).unwrap();
        fs::create_dir_all(chroot_path.join("proc")).unwrap();
        fs::create_dir_all(chroot_path.join("sys")).unwrap();

        // Create a dummy file at /
        File::create(Path::new("/").join("dummy")).unwrap();
        assert!(Path::new("/dummy").exists());

        // Enter the chroot
        let chroot = enter_update_chroot(&chroot_path).unwrap();

        // Verify we are inside the chroot
        assert!(!chroot_path.join("dev").exists());
        assert!(!chroot_path.join("proc").exists());
        assert!(!chroot_path.join("sys").exists());
        assert!(Path::new("/dev").exists());
        assert!(Path::new("/proc").exists());
        assert!(Path::new("/sys").exists());

        // Verify we cannot access the dummy file from inside of chroot
        assert!(!Path::new("/dummy").exists());

        // Exit the chroot
        chroot.exit().unwrap();

        // Verify that files exist at original paths
        assert!(chroot_path.join("dev").exists());
        assert!(chroot_path.join("proc").exists());
        assert!(chroot_path.join("sys").exists());
        assert!(Path::new("/dummy").exists());
    }

    #[functional_test(feature = "helpers")]
    fn test_enter_host_chroot() {
        // Create a temporary directory to act as the chroot environment
        let temp_dir = tempdir().unwrap();
        let chroot_path = temp_dir.path().to_path_buf();

        // Create a dummy file at /
        File::create(Path::new("/").join("dummy-root")).unwrap();
        assert!(Path::new("/dummy-root").exists());
        // Create a dummy file in chroot_path
        File::create(chroot_path.join("dummy-chroot")).unwrap();
        assert!(chroot_path.join("dummy-chroot").exists());

        // Enter the chroot
        let chroot = enter_host_chroot(&chroot_path).unwrap();

        // Verify we cannot access dummy-root from inside chroot, but can access dummy-chroot
        assert!(!Path::new("/dummy-root").exists());
        assert!(Path::new("/dummy-chroot").exists());

        // Exit the chroot
        chroot.exit().unwrap();

        // Verify that files exist at the original path
        assert!(chroot_path.join("dummy-chroot").exists());
        assert!(Path::new("/dummy-root").exists());
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_enter_chroot_fail_to_mount_special_dir() {
        // Create a temporary directory to act as the chroot environment
        let temp_dir = tempdir().unwrap();
        let chroot_path = temp_dir.path().to_path_buf();

        // Create necessary directories for the chroot environment
        fs::create_dir_all(chroot_path.join("dev")).unwrap();
        fs::create_dir_all(chroot_path.join("proc")).unwrap();
        fs::create_dir_all(chroot_path.join("sys")).unwrap();

        // Pre-mount /dev to simulate failure
        let dev_mount = Mount::builder()
            .fstype("devtmpfs")
            .flags(MountFlags::empty())
            .mount("devtmpfs", chroot_path.join("dev"))
            .unwrap();

        // Attempt to enter the chroot
        let result_dev = Chroot::enter(&chroot_path, true);
        assert_eq!(
            result_dev.unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::ChrootMountSpecialDir {
                dir: "/dev".to_string()
            })
        );

        // Un-mount /dev
        dev_mount.unmount(UnmountFlags::empty()).unwrap();

        // Pre-mount /sys to simulate failure
        let sys_mount = Mount::builder()
            .fstype("sysfs")
            .flags(MountFlags::empty())
            .mount("sysfs", chroot_path.join("sys"))
            .unwrap();

        // Attempt to enter the chroot
        let result_sys = Chroot::enter(&chroot_path, true);
        assert_eq!(
            result_sys.unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::ChrootMountSpecialDir {
                dir: "/sys".to_string()
            })
        );

        // Un-mount /sys
        sys_mount.unmount(UnmountFlags::empty()).unwrap();

        // Mounting a new /proc filesystem over the existing one does not fail
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_enter_chroot_fail_nonexistent_dir() {
        // Attempt to enter the chroot
        let result = Chroot::enter(Path::new("/nonexistent-dir"), false);
        assert_eq!(
            result.unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::EnterChroot)
        );
    }
}
