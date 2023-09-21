use std::{
    fs,
    io::{Seek, SeekFrom, Write},
    os::{
        fd::{IntoRawFd, RawFd},
        unix,
    },
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Error};
use log::info;
use sys_mount::{Mount, MountFlags, Unmount, UnmountFlags};
use trident_api::{config::HostConfiguration, status::HostStatus};

use crate::{
    modules::{get_root_block_device, storage},
    run_command,
};

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

pub(crate) struct UpdateTargetEnvironment {
    pub chroot: Option<Chroot>,
    pub mount_path: PathBuf,
    pub root_block_device: trident_api::status::BlockDeviceInfo,
}

pub(crate) fn setup_root_chroot(
    host_config: &HostConfiguration,
    host_status: &HostStatus,
    do_enter_chroot: bool,
) -> Result<Option<UpdateTargetEnvironment>, Error> {
    if let Some(root_block_device) = get_root_block_device(host_config, host_status) {
        let root_mount_path = Path::new("/partitionMount");
        let update_fs_target = Path::new("update-fs.target");
        let update_fstab_root =
            tempfile::tempdir().context("Failed to create temporary directory")?;
        let update_fstab_path = update_fstab_root.path().join(Path::new("fstab"));
        let systemd_unit_root_path = Path::new("/etc/systemd/system");

        storage::fstab::Fstab::from_mount_points(
            host_status,
            &host_config.storage.mount_points,
            root_mount_path,
            update_fs_target,
        )
        .context("Failed to generate bootstrap fstab")?
        .write(update_fstab_path.as_path())
        .context("Failed to write bootstrap fstab")?;

        // Create custom target for the filesystems mounted for the update reconciliation.
        fs::write(
            systemd_unit_root_path.join(update_fs_target),
            indoc::indoc! {r#"
                [Unit]
                Description=Update File Systems
                DefaultDependencies=no
                Conflicts=shutdown.target
            "#}
            .as_bytes(),
        )
        .context(format!(
            "Failed to write {}",
            update_fs_target.to_string_lossy()
        ))?;

        run_command(
            Command::new("/usr/lib/systemd/system-generators/systemd-fstab-generator")
                .arg(systemd_unit_root_path)
                .arg(systemd_unit_root_path)
                .arg(systemd_unit_root_path)
                .env("SYSTEMD_FSTAB", update_fstab_path)
                .env("SYSTEMD_LOG_TARGET", "console")
                .env("SYSTEMD_LOG_LEVEL", "debug"),
        )
        .context("Failed to reload systemd daemon")?;

        run_command(Command::new("systemctl").arg("daemon-reload"))
            .context("Failed to reload systemd daemon")?;

        let mount_result =
            run_command(Command::new("systemctl").arg("start").arg(update_fs_target))
                .context("Failed to mount target filesystems");

        if let Err(mount_result) = mount_result {
            unmount_target_volumes(root_mount_path)?;
            return Err(mount_result);
        }

        let chroot = if do_enter_chroot {
            Some(enter_chroot(root_mount_path)?)
        } else {
            None
        };
        Ok(Some(UpdateTargetEnvironment {
            chroot,
            mount_path: root_mount_path.to_owned(),
            root_block_device,
        }))
    } else {
        info!("No root block device found, will skip reconciling root filesystem.");
        Ok(None)
    }
}

pub(crate) fn enter_chroot(root_mount_path: &Path) -> Result<Chroot, Error> {
    Chroot::enter(root_mount_path).context("Failed to enter updated filesystem chroot")
}

pub(crate) fn unmount_target_volumes(mount_path: &Path) -> Result<(), Error> {
    let mount_unit = String::from_utf8(
        run_command(
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
    run_command(Command::new("systemctl").arg("stop").arg(mount_unit))
        .context("Failed to safely unmount target root partition.")?;
    Ok(())
}
