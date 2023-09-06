use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    os::{
        fd::{IntoRawFd, RawFd},
        unix,
    },
    path::Path,
    process::Command,
};

use anyhow::{bail, Context, Error};
use log::info;
use sys_mount::{Mount, MountFlags, Unmount, UnmountFlags};

/// Create a chroot environment.
///
/// Note: Dropping this object does *not* exit the chroot. You must call `exit()` manually.
pub(crate) struct Chroot {
    rootfd: RawFd,
    mounts: Vec<Mount>,
}
impl Chroot {
    /// Mount special directories ('/dev', '/proc', and '/sys') and enter chroot.
    pub(crate) fn enter(path: &Path) -> Result<Self, Error> {
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
    pub(crate) fn exit(self) -> Result<(), Error> {
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

pub(crate) fn run_script(script: &str) -> Result<(), Error> {
    let mut file = tempfile::tempfile().context("Failed to create temporary file")?;
    file.write_all(script.as_bytes())
        .context("Failed to write temporary file")?;
    file.seek(SeekFrom::Start(0))
        .context("Failed to seek temporary file")?;
    let status = Command::new("bash")
        .stdin(file)
        .status()
        .context("Failed to execute script")?;

    if !status.success() {
        match status.code() {
            Some(code) => bail!("Script exited with status: {code}"),
            None => bail!("Script was terminated by signal"),
        }
    }

    Ok(())
}
