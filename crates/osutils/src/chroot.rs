use std::{
    fs, mem,
    os::{fd::OwnedFd, unix},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::Context;
use log::{debug, error, trace, warn};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;
use sys_mount::{Mount, MountFlags, Unmount, UnmountDrop, UnmountFlags};
use tempfile::{Builder, TempDir};

use trident_api::error::{
    InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt,
};

/// Create a chroot environment.
///
/// Note: Dropping this object does *not* exit the chroot. You must call `exit()` manually.
pub struct Chroot {
    rootfd: OwnedFd,
    mounts: Vec<UnmountDrop<Mount>>,
    cleanup_dirs: Vec<TempDir>,
}
impl Chroot {
    /// Mount special directories ('/dev', '/proc', and '/sys') and enter chroot.
    fn enter(path: &Path) -> Result<Self, TridentError> {
        if !path.exists() {
            return Err(TridentError::new(ServicingError::EnterChroot));
        }

        // Mount special dirs.
        debug!("Mounting special directories");
        let mut mounts = Vec::new();
        let mut cleanup_dirs = Vec::new();

        for dir in SpecialDir::iter() {
            let (mount, temp_dir) = mount_sepcial_dir(path, dir)?;

            mounts.push(mount);

            if let Some(temp_dir) = temp_dir {
                cleanup_dirs.push(temp_dir);
            }
        }

        // Enter the chroot.
        debug!("Entering chroot");
        let rootfd = fs::File::open("/")
            .structured(ServicingError::EnterChroot)?
            .into();
        unix::fs::chroot(path).structured(ServicingError::EnterChroot)?;
        std::env::set_current_dir("/").structured(ServicingError::EnterChroot)?;

        Ok(Self {
            rootfd,
            mounts,
            cleanup_dirs,
        })
    }

    pub fn execute_and_exit<F>(self, f: F) -> Result<(), TridentError>
    where
        F: FnOnce() -> Result<(), TridentError>,
    {
        // Execute the function.
        let result = f();

        // Exit the chroot.
        //
        // If function `f` produced an error it is returned from this function and any errors from
        // the exit are logged at the warn level. If `f` returned successfully, then directly return
        // any errors produced by the exit.
        if let Err(e) = self.exit() {
            if result.is_ok() {
                return Err(e);
            }
            warn!("Encountered secondary error while handling earlier error: {e:?}");
        }
        result
    }

    /// Exit the chroot environment and unmount special directories.
    fn exit(self) -> Result<(), TridentError> {
        // Exit the chroot.
        nix::unistd::fchdir(self.rootfd).structured(ServicingError::ExitChroot)?;
        unix::fs::chroot(".").structured(ServicingError::ExitChroot)?;
        debug!("Exited chroot. Unmounting special directories");

        for mount in self.mounts {
            for retry_count in 1..6 {
                if retry_count != 1 {
                    trace!(
                        "Unmounting '{}' attempt {}",
                        mount.target_path().display(),
                        retry_count
                    );
                }
                let ret = mount.unmount(UnmountFlags::empty());
                if ret.is_ok() {
                    mem::forget(mount);
                    break;
                } else if retry_count == 5 {
                    return ret.structured(ServicingError::ChrootUnmountSpecialDir);
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        }

        // Delete any temporary directories that were created for mounting
        // special directories. If these fail to be removed, log a warning but
        // continue with the exit process.
        for temp_dir in self.cleanup_dirs {
            let tmp_dir_path = temp_dir.path().to_path_buf();
            debug!(
                "Removing temporary directory '{}' used for mounting special directory",
                tmp_dir_path.display()
            );
            if let Err(e) = temp_dir.close() {
                error!(
                    "Failed to remove temporary directory '{}': {e:?}",
                    tmp_dir_path.display()
                );
            }
        }

        Ok(())
    }
}

pub fn enter_update_chroot(root_mount_path: &Path) -> Result<Chroot, TridentError> {
    Chroot::enter(root_mount_path).message("Failed to enter updated OS chroot")
}

/// Mount a specific special directory.
fn mount_sepcial_dir(
    root_path: &Path,
    dir: SpecialDir,
) -> Result<(UnmountDrop<Mount>, Option<TempDir>), TridentError> {
    let full_path = dir.dir_path(root_path);

    let temp_dir: Option<TempDir> = if !full_path.exists() {
        // Try to create the directory if it doesn't exist, as it is required
        // for mounting the special directory. We'll use a tempdir to ensure it
        // gets cleaned up after unmounting or on failure.
        debug!(
            "Special directory '{}' does not exist. Creating temporary.",
            full_path.display()
        );

        Some(
            Builder::new()
                .rand_bytes(0)
                .prefix(dir.dir_name())
                .tempdir_in(root_path)
                .with_context(|| {
                    format!(
                        "Failed to create temporary directory for '{}' in '{}'",
                        dir.dir_name(),
                        root_path.display()
                    )
                })
                .structured(ServicingError::ChrootMountSpecialDir {
                    dir: dir.dir_name().to_string(),
                })?,
        )
    } else if !full_path.is_dir() {
        // If the path exists but is not a directory, we cannot mount the
        // special directory and must return an error. This is an image
        // misconfiguration issue.
        return Err(TridentError::new(
            InvalidInputError::ChrootSpecialDirInvalid {
                dir: dir.dir_path("/").display().to_string(),
            },
        ));
    } else {
        // No temp dir needed.
        None
    };

    let mount = Mount::builder()
        .fstype(dir.fstype())
        .flags(MountFlags::empty())
        .mount(dir.source(), full_path)
        .structured(ServicingError::ChrootMountSpecialDir {
            dir: dir.dir_name().to_string(),
        })?
        .into_unmount_drop(UnmountFlags::empty());

    Ok((mount, temp_dir))
}

#[derive(Debug, Clone, Copy, EnumIter)]
enum SpecialDir {
    Dev,
    Proc,
    Sys,
}

impl SpecialDir {
    /// Returns the filesystem type to mount for this special directory.
    fn fstype(&self) -> &str {
        match self {
            Self::Dev => "devtmpfs",
            Self::Proc => "proc",
            Self::Sys => "sysfs",
        }
    }

    /// Returns the source to mount for this special directory.
    fn source(&self) -> &str {
        match self {
            Self::Dev => "devtmpfs",
            Self::Proc => "proc",
            Self::Sys => "sysfs",
        }
    }

    /// Returns the RELATIVE target path to mount for this special directory.
    fn dir_name(&self) -> &str {
        match self {
            Self::Dev => "dev",
            Self::Proc => "proc",
            Self::Sys => "sys",
        }
    }

    /// Returns the full target path to mount for this special directory,
    /// relative to the provided root path.
    fn dir_path(&self, root_path: impl AsRef<Path>) -> PathBuf {
        root_path.as_ref().join(self.dir_name())
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::fs::{self, File};

    use tempfile::tempdir;

    use pytest_gen::functional_test;
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
        let chroot = Chroot::enter(&chroot_path).unwrap();

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
    fn test_enter_and_exit_chroot_with_missing_special_dirs() {
        // Create a temporary directory to act as the chroot environment
        let temp_dir = tempdir().unwrap();
        let chroot_path = temp_dir.path().to_path_buf();

        // Intentionally do not create /dev, /proc, /sys under the chroot.
        assert!(!chroot_path.join("dev").exists());
        assert!(!chroot_path.join("proc").exists());
        assert!(!chroot_path.join("sys").exists());

        // Create a dummy file at /
        File::create(Path::new("/").join("dummy")).unwrap();
        assert!(Path::new("/dummy").exists());

        // Enter the chroot; special mount directories should be created temporarily.
        let chroot = Chroot::enter(&chroot_path).unwrap();

        // Verify we are inside the chroot and special directories are available.
        assert!(Path::new("/dev").exists());
        assert!(Path::new("/proc").exists());
        assert!(Path::new("/sys").exists());

        // Verify we cannot access the dummy file from inside of chroot.
        assert!(!Path::new("/dummy").exists());

        // Exit the chroot.
        chroot.exit().unwrap();

        // Temporary special directories should be cleaned up after unmount/exit.
        assert!(!chroot_path.join("dev").exists());
        assert!(!chroot_path.join("proc").exists());
        assert!(!chroot_path.join("sys").exists());
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
        let result_dev = Chroot::enter(&chroot_path);
        assert_eq!(
            result_dev.err().unwrap().kind(),
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
        let result_sys = Chroot::enter(&chroot_path);
        assert_eq!(
            result_sys.err().unwrap().kind(),
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
        let result = Chroot::enter(Path::new("/nonexistent-dir"));
        assert_eq!(
            result.err().unwrap().kind(),
            &ErrorKind::Servicing(ServicingError::EnterChroot)
        );
    }
}
