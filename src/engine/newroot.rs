use std::{
    collections::BTreeMap,
    fmt::Write,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::{debug, error, info, warn};
use sys_mount::{MountBuilder, MountFlags};

use osutils::{
    container, files,
    filesystems::MountFileSystemType,
    findmnt::{FindMnt, MountpointMetadata},
    mount, path,
};
use trident_api::{
    config::{HostConfiguration, InternalMountPoint},
    constants::{
        internal_params::EXECROOT_DENYLIST_EXTENSION, EXEC_ROOT_PATH, MOUNT_OPTION_READ_ONLY,
        NONE_MOUNT_POINT, ROOT_MOUNT_POINT_PATH, UPDATE_ROOT_FALLBACK_PATH, UPDATE_ROOT_PATH,
    },
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::AbVolumeSelection,
    BlockDeviceId,
};

/// List of special directories that should not be bind mounted anywhere in the
/// execroot.
const PROHIBITED_EXECROOT_MOUNTS: [&str; 5] = [
    // All devfs
    "/dev",
    // All procfs
    "/proc",
    // All sysfs
    "/sys",
    // Docker containers
    "/var/lib/docker/vfs/dir",
    // Everything under /run, fix for #8879 and #8926
    "/run",
];

/// Filter function to prevent specific mount points from being bind mounted in
/// the execroot.
///
/// Used in a call to
/// `FindMnt::traverse_depth().into_iter().filter(execroot_filter)`
///
/// Should return `true` if the mount point should be bind mounted in the
/// execroot.
fn execroot_filter(mnt: &&MountpointMetadata) -> bool {
    // Skip anything with docker in the name
    !mnt.target.components().any(|c| c.as_os_str() == "docker")
}

/// NewrootMount represents all the necessary mounting points for newroot and
/// the nested execmount to exit the chroot jail. It is also responsible for
/// unmounting them in the correct order. NewrootMount provides information for:
/// - The newroot device path.
/// - The execroot path.
///
/// NewrootMount can:
/// - Update/add custom mount paths for the new root each time one is created.
/// - Unmount all registered new rootmounts.
/// - Handle Trident crashes with the `Drop` method to unmount the new root.
#[derive(Debug)]
pub struct NewrootMount {
    /// Absolute path in the host to newroot's mount point.
    ///
    /// E.g.: `/mnt/newroot`
    newroot_mount_path: PathBuf,

    /// Absolute path in the host to execroot's mount point.
    ///
    /// E.g.: `/mnt/newroot/tmp/execroot`
    execroot_mount_path: PathBuf,

    /// List of active mount points being managed by the NewrootMount object.
    mounts: Vec<PathBuf>,
}

impl NewrootMount {
    /// Construct a simple NewrootMount object with the given newroot path.
    fn new(newroot_mount_path: PathBuf) -> Self {
        Self {
            execroot_mount_path: path::join_relative(&newroot_mount_path, EXEC_ROOT_PATH),
            newroot_mount_path,
            mounts: Vec::new(),
        }
    }

    /// Given a host status, create all the required mount points for newroot
    /// and return a NewrootMount object.
    #[tracing::instrument(name = "initialize_new_root", skip_all)]
    pub fn create_and_mount(
        host_config: &HostConfiguration,
        disk_paths: &BTreeMap<BlockDeviceId, PathBuf>,
        update_volume: AbVolumeSelection,
    ) -> Result<Self, TridentError> {
        // Get the path where the newroot should be mounted
        let new_root_path = get_new_root_path();
        info!(
            "Attempting to mount newroot at '{}'",
            new_root_path.display()
        );

        // Create a base NewrootMount object. We create it early so we can
        // leverage the Drop trait to unmount everything in newroot in case of an error
        // while mounting.
        let mut newroot_mount = NewrootMount::new(new_root_path);

        newroot_mount
            .mount_newroot_partitions(host_config, disk_paths, update_volume)
            .message("Failed to mount all partitions in newroot")?;

        // Mount tmpfs for /tmp and /run
        newroot_mount.mount_tmpfs("/tmp")?;
        newroot_mount.mount_tmpfs("/run")?;

        // PREVIEW-ONLY OVERRIDE
        let execroot_deny_list_extension = if let Some(res) = host_config
            .internal_params
            .get_vec_string(EXECROOT_DENYLIST_EXTENSION)
        {
            let overrides = res.structured(InternalError::Internal(
                "Failed to get execroot deny-list extension",
            ))?;

            let mut overrides_string = String::new();
            for s in &overrides {
                let _ = writeln!(overrides_string, "  - {s}");
            }
            warn!("PREVIEW ONLY: Extending execroot deny-list with:\n{overrides_string}");

            overrides
        } else {
            Vec::new()
        };

        // Mount execroot on newroot
        newroot_mount
            .mount_execroot(execroot_deny_list_extension)
            .structured(ServicingError::MountExecroot)?;

        Ok(newroot_mount)
    }

    /// Returns the absolute path in the host to newroot's mount point.
    ///
    /// E.g.: `/mnt/newroot`
    pub fn path(&self) -> &Path {
        &self.newroot_mount_path
    }

    /// Returns the absolute path in the host to execroot's mount point.
    ///
    /// E.g.: `/mnt/newroot/tmp/execroot`
    pub fn execroot_path(&self) -> &Path {
        &self.execroot_mount_path
    }

    /// Returns the absolute path of the execroot relative to newroot.
    ///
    /// E.g.: `/tmp/execroot`
    pub fn execroot_relative_path(&self) -> &Path {
        Path::new(EXEC_ROOT_PATH)
    }

    fn unmount_all_impl(&mut self) -> Result<(), TridentError> {
        // Unmount all mounts in reverse order. If we fail to unmount one, we still clear
        // `self.mounts` but stop trying to unmount the rest of the mounts.
        for mount in self.mounts.drain(..).rev() {
            debug!("Unmounting '{}'", mount.display());
            mount::umount(&mount, false).structured(ServicingError::UnmountNewroot {
                dir: mount.to_string_lossy().into(),
            })?;
        }

        Ok(())
    }

    /// Unmount all registered mounts in the correct order.
    #[tracing::instrument(name = "newroot_unmount", skip_all)]
    pub fn unmount_all(mut self) -> Result<(), TridentError> {
        info!("Unmounting newroot at '{}'", self.path().display());
        self.unmount_all_impl()
    }

    /// Add a new mount point to newroot object.
    fn add_mount(&mut self, mount: PathBuf) {
        self.mounts.push(mount);
    }

    /// Mount all block devices in the newroot.
    fn mount_newroot_partitions(
        &mut self,
        host_config: &HostConfiguration,
        disk_paths: &BTreeMap<BlockDeviceId, PathBuf>,
        update_volume: AbVolumeSelection,
    ) -> Result<(), TridentError> {
        let mut block_device_paths = disk_paths.clone();

        for raid in &host_config.storage.raid.software {
            block_device_paths.insert(raid.id.clone(), raid.device_path());
        }

        if let Some(encryption) = &host_config.storage.encryption {
            for volume in &encryption.volumes {
                block_device_paths.insert(volume.id.clone(), volume.device_path());
            }
        }

        for verity in &host_config.storage.internal_verity {
            block_device_paths.insert(verity.id.clone(), verity.temporary_device_path());
        }

        if let Some(ab) = &host_config.storage.ab_update {
            for pair in &ab.volume_pairs {
                let path = match update_volume {
                    AbVolumeSelection::VolumeA => block_device_paths.get(&pair.volume_a_id),
                    AbVolumeSelection::VolumeB => block_device_paths.get(&pair.volume_b_id),
                }
                .structured(InternalError::Internal("Bad reference in A/B volume"))?;
                block_device_paths.insert(pair.id.clone(), path.clone());
            }
        }

        // Mount all block devices in the newroot
        mount_points_map(host_config)
            .iter()
            .try_for_each(|(path, mp)| {
                let target_path =
                    self.path()
                        .join(path.strip_prefix(ROOT_MOUNT_POINT_PATH).context(format!(
                            "Failed to strip prefix '{}' from '{}'",
                            ROOT_MOUNT_POINT_PATH,
                            path.display()
                        ))?);

                debug!(
                    "Mounting block device '{}' to '{}'",
                    mp.target_id,
                    target_path.display()
                );

                mount::ensure_mount_directory(&target_path).context(format!(
                    "Failed to prepare mount directory for block device '{}'",
                    mp.target_id
                ))?;

                let device_path = block_device_paths.get(&mp.target_id).context(format!(
                    "Failed to find block device path for id '{}'",
                    mp.target_id
                ))?;

                mount::mount(
                    device_path,
                    &target_path,
                    MountFileSystemType::from_api_type(mp.filesystem).context(format!(
                        "Filesystem type of block device '{}' is not valid for mounting: '{}'",
                        mp.target_id, mp.filesystem,
                    ))?,
                    &mp.options,
                )
                .context(format!(
                    "Failed to mount block device '{}' with device path '{}' to '{}'",
                    mp.target_id,
                    device_path.display(),
                    target_path.display()
                ))?;

                self.add_mount(target_path.clone());

                Ok::<(), Error>(())
            })
            .structured(ServicingError::MountNewroot)?;

        Ok(())
    }

    /// Mount a tmpfs filesystem on the specified path in newroot.
    fn mount_tmpfs(&mut self, target_path: impl AsRef<Path>) -> Result<(), TridentError> {
        // Get the full path to the target path in newroot.
        let target_path_full = path::join_relative(self.path(), target_path);

        debug!("Mounting tmpfs to '{}'", target_path_full.display());

        // Do the actual tmpfs mount
        MountBuilder::default()
            .fstype("tmpfs")
            .flags(MountFlags::empty())
            .mount("tmpfs", &target_path_full)
            .context(format!(
                "Failed to mount tmpfs for '{}'",
                target_path_full.display()
            ))
            .structured(ServicingError::MountNewrootSpecialDir {
                dir: target_path_full.to_string_lossy().to_string(),
            })?;

        // Add the mount to the list of mounts
        self.add_mount(target_path_full);

        Ok(())
    }

    /// Create bind mount points to the local root filesystem & nested mounts
    /// for the execroot inside newroot.
    fn mount_execroot(&mut self, execroot_deny_list_extension: Vec<String>) -> Result<(), Error> {
        debug!(
            "Attempting to bind mount execroot to '{}'",
            self.execroot_path().display()
        );

        std::fs::create_dir_all(self.execroot_path()).context(format!(
            "Failed to create directory '{}' for execroot",
            self.execroot_path().display()
        ))?;

        info!("Mounting execroot to '{}'", self.execroot_path().display());

        let mut mounts = FindMnt::run()
            .context("Failed to get current mount points")?
            .root()
            .context("Failed to get root mount point")?;

        // Remove anything mounted under newroot from the execroot mounts
        mounts.prune_prefix(self.path());

        // Generate a list of paths to deny-list from being bind mounted in the
        // execroot.
        let execroot_deny_list = PROHIBITED_EXECROOT_MOUNTS
            .iter()
            .map(|s| s.to_string())
            .chain(execroot_deny_list_extension)
            .collect::<Vec<_>>();

        // Prune special directories from the execroot mounts
        execroot_deny_list.iter().for_each(|deny_path| {
            debug!(
                "Blocking all mounts under '{}' from being bind mounted in execroot",
                deny_path
            );
            mounts.prune_prefix(deny_path)
        });

        // If we are running in a container, we expect a bind mount to the
        // container's host, generally `/host`. We must also prune the special
        // directories from the host root path.
        if container::is_running_in_container()
            .unstructured("Failed to check if running in container")?
        {
            // Get the host root path, generally `/host`
            let host =
                container::get_host_root_path().unstructured("Failed to get host root path")?;

            debug!(
                "Running in a container, pruning special mounts under host root path '{}'",
                host.display()
            );

            // Prune special directories from the host root path. (e.g. `/host/dev`)
            execroot_deny_list.iter().for_each(|deny_path| {
                let prune = path::join_relative(&host, deny_path);
                debug!(
                    "Blocking all mounts under '{}' from being bind mounted in execroot",
                    prune.display()
                );
                mounts.prune_prefix(prune);
            });
        }

        // Go over all remaining mounts to filter out anything else we don't want
        // and bind mount the rest.
        mounts
            .traverse_depth()
            .into_iter()
            .filter(execroot_filter)
            .try_for_each(|item| {
                // The target for the bind mount is the path in the newroot
                let target = path::join_relative(self.execroot_path(), &item.target);
                // The source for the bind mount is the path in the host
                let source = &item.target;

                if item.is_unbindable() {
                    warn!(
                        "Skipping unbindable mount '{}' to '{}'",
                        source.display(),
                        target.display()
                    );

                    return Ok(());
                }

                // Check if the mount is read-only, if so, we need to bind mount
                // it as read-only.
                let flags = if item.options.contains(MOUNT_OPTION_READ_ONLY) {
                    MountFlags::RDONLY
                } else {
                    MountFlags::empty()
                };

                do_bind_mount(source, &target, flags).with_context(|| {
                    format!(
                        "Failed to bind mount '{}' to '{}' in execroot. This is likely due to a \
                        special directory that should not be bind mounted. Please send all of this \
                        output to support :)\n{:#?}",
                        source.display(),
                        target.display(),
                        item,
                    )
                })?;

                self.add_mount(target);

                Ok(())
            })
    }
}

impl Drop for NewrootMount {
    fn drop(&mut self) {
        if !self.mounts.is_empty() {
            error!("NewrootMount was dropped without unmounting all mounts");
        }

        if let Err(e) = self.unmount_all_impl() {
            error!(
                "Failed to unmount new root while handling another error: {:?}",
                e
            );
        }
    }
}

/// Returns an ordered map of mount points to their corresponding InternalMountPoint objects.
fn mount_points_map(host_config: &HostConfiguration) -> BTreeMap<&Path, &InternalMountPoint> {
    host_config
        .storage
        .internal_mount_points
        .iter()
        .map(|mp| (&*mp.path, mp))
        .filter(|(path, _)| path.as_os_str() != NONE_MOUNT_POINT)
        .collect::<BTreeMap<_, _>>()
}

/// Returns the path where the newroot should be mounted.
fn get_new_root_path() -> PathBuf {
    let mut new_root_path = Path::new(UPDATE_ROOT_PATH);
    if mount::ensure_mount_directory(new_root_path).is_err() {
        new_root_path = Path::new(UPDATE_ROOT_FALLBACK_PATH);
    }
    new_root_path.to_owned()
}

/// Perform a bind mount from the source to the target path. If the target does
/// not exist, it will be created based on the type of the source. Supports
/// both files and directories.
fn do_bind_mount(source: &Path, target: &Path, flags: MountFlags) -> Result<(), Error> {
    debug!(
        "Bind mounting '{}' to '{}'",
        source.display(),
        target.display()
    );

    // Ensure the target exists and is valid
    if !target.exists() {
        // If the target does not exist, create it
        if source.is_dir() {
            // If the source is a directory, create the target directory
            std::fs::create_dir_all(target).context(format!(
                "Failed to create directory '{}' for bind mount",
                target.display()
            ))?;
        } else if source.is_file() {
            // If the source is a file, create the target file
            files::create_file(source).context(format!(
                "Failed to create file '{}' for bind mount",
                source.display()
            ))?;
        } else {
            // If the source is not a directory or file, fail
            bail!(
                "Bind mount source '{}' is not a directory or file",
                source.display()
            );
        }
    } else if source.is_dir() != target.is_dir() {
        // If the source and target have different types, fail
        bail!(
            "Bind mount source '{}' and target '{}' have different types",
            source.display(),
            target.display()
        );
    }

    // Do the actual bind mount
    MountBuilder::default()
        .flags(MountFlags::BIND | flags)
        .mount(source, target)
        .with_context(|| {
            format!(
                "Failed to bind mount '{}' to '{}'",
                source.display(),
                target.display()
            )
        })?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    use std::path::PathBuf;

    use trident_api::config::{FileSystemType, HostConfiguration, InternalMountPoint, Storage};

    #[test]
    fn test_mount_point_ordering() {
        let host_config = HostConfiguration {
            storage: Storage {
                internal_mount_points: vec![
                    InternalMountPoint {
                        path: PathBuf::from("/mnt/boot/efi"),
                        target_id: "sda3".to_string(),
                        filesystem: FileSystemType::Vfat,
                        options: vec![],
                    },
                    InternalMountPoint {
                        path: PathBuf::from("/mnt"),
                        target_id: "sda1".to_string(),
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    },
                    InternalMountPoint {
                        path: PathBuf::from("/a"),
                        target_id: "sda1".to_string(),
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    },
                    InternalMountPoint {
                        path: PathBuf::from("/"),
                        target_id: "sda1".to_string(),
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    },
                    InternalMountPoint {
                        path: PathBuf::from("/mnt/boot"),
                        target_id: "sda2".to_string(),
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let paths = mount_points_map(&host_config)
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                PathBuf::from("/"),
                PathBuf::from("/a"),
                PathBuf::from("/mnt"),
                PathBuf::from("/mnt/boot"),
                PathBuf::from("/mnt/boot/efi")
            ]
        );
    }

    #[test]
    fn test_newroot_mount_paths() {
        let newroot_path = Path::new("/mnt/newroot");
        let newroot_mount = NewrootMount::new(newroot_path.to_owned());

        assert_eq!(newroot_mount.path(), newroot_path, "Newroot path mismatch");
        assert_eq!(
            newroot_mount.execroot_path(),
            path::join_relative(newroot_path, EXEC_ROOT_PATH),
            "Execroot path mismatch"
        );
        assert_eq!(
            newroot_mount.execroot_relative_path(),
            Path::new(EXEC_ROOT_PATH),
            "Execroot relative path mismatch"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::{
        fs::{self, File},
        io::Read,
        path::{Path, PathBuf},
    };

    use const_format::formatcp;
    use maplit::btreemap;
    use tempfile::{NamedTempFile, TempDir};

    use osutils::{
        hashing_reader::HashingReader,
        image_streamer, mountpoint,
        repart::{RepartEmptyMode, SystemdRepartInvoker},
        testutils::repart::{self, CDROM_DEVICE_PATH, CDROM_MOUNT_PATH, TEST_DISK_DEVICE_PATH},
        udevadm,
    };
    use pytest_gen::functional_test;
    use trident_api::{
        config::{self, Disk, FileSystemType, HostConfiguration, Partition, PartitionType},
        constants::MOUNT_OPTION_READ_ONLY,
        error::ErrorKind,
        status::HostStatus,
    };

    #[functional_test(feature = "engine")]
    fn test_mount_and_umount() {
        // CDROM device to be mounted
        let device = Path::new(CDROM_DEVICE_PATH);
        // Mount point
        let mount_point = Path::new(CDROM_MOUNT_PATH);

        if mountpoint::check_is_mountpoint(mount_point).unwrap() {
            mount::umount(mount_point, false).unwrap();
        }

        // Create the mount point directory if it doesn't exist yet
        fs::create_dir_all(mount_point).unwrap();

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/sr"),
                        partitions: vec![Partition {
                            id: "sr0".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 0.into(),
                        }],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![config::InternalMountPoint {
                        path: PathBuf::from("/"),
                        target_id: "sr0".to_string(),
                        filesystem: FileSystemType::Iso9660,
                        options: vec![MOUNT_OPTION_READ_ONLY.into()],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/sr"),
                "sr0".into() => PathBuf::from(CDROM_DEVICE_PATH)
            },
            ..Default::default()
        };

        let mut newroot_mount = NewrootMount::new(mount_point.to_owned());
        newroot_mount
            .mount_newroot_partitions(
                &host_status.spec,
                &host_status.block_device_paths,
                AbVolumeSelection::VolumeA,
            )
            .unwrap();

        // If device is a file, fetch the name of loop device that was mounted at mount point;
        // otherwise, use the device path itself
        let loop_device = if device.is_file() {
            find_loop_device(device).unwrap()
        } else {
            device.to_string_lossy().to_string()
        };

        // Validate that the device has been successfully mounted
        assert!(
            is_device_mounted_at(&loop_device, mount_point),
            "Device not mounted at the expected mount point"
        );

        let root_mount_dir = tempfile::tempdir().unwrap();

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "esp".to_string(),
                                partition_type: PartitionType::Esp,
                                size: 0.into(),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: 0.into(),
                            },
                        ],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![
                        config::InternalMountPoint {
                            path: PathBuf::from("/"),
                            target_id: "root".to_string(),
                            filesystem: FileSystemType::Ext4,
                            options: vec!["defaults".into()],
                        },
                        config::InternalMountPoint {
                            path: PathBuf::from("/boot/efi"),
                            target_id: "esp".to_string(),
                            filesystem: FileSystemType::Vfat,
                            options: vec!["umask=0077".into()],
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "esp".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root".into() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2"))
            },
            ..Default::default()
        };

        // Partition test drive
        let partition_definition = repart::generate_partition_definition_esp_generic();
        let disk_bus_path = PathBuf::from(TEST_DISK_DEVICE_PATH);
        let repart = SystemdRepartInvoker::new(disk_bus_path, RepartEmptyMode::Force)
            .with_partition_entries(partition_definition.clone());
        let _ = repart.execute().unwrap();
        udevadm::settle().unwrap();

        // Image partitions on /dev/sdb
        let stream: Box<dyn Read> = Box::new(
            File::open("/data/esp.rawzst")
                .context("Failed to open esp image")
                .unwrap(),
        );
        image_streamer::stream_zstd(
            HashingReader::new(stream),
            Path::new(format!("{TEST_DISK_DEVICE_PATH}1").as_str()),
        )
        .unwrap();
        let stream: Box<dyn Read> = Box::new(
            File::open("/data/root.rawzst")
                .context("Failed to open root image")
                .unwrap(),
        );
        image_streamer::stream_zstd(
            HashingReader::new(stream),
            Path::new(format!("{TEST_DISK_DEVICE_PATH}2").as_str()),
        )
        .unwrap();

        // Test recursive mounting
        let mut newroot_mount2 = NewrootMount::new(root_mount_dir.path().to_owned());
        newroot_mount2
            .mount_newroot_partitions(
                &host_status.spec,
                &host_status.block_device_paths,
                AbVolumeSelection::VolumeA,
            )
            .unwrap();

        assert!(root_mount_dir
            .path()
            .join("boot/efi/EFI/BOOT/bootx64.efi")
            .exists());
        newroot_mount2.unmount_all().unwrap();
        assert!(!root_mount_dir.path().join("boot").exists());

        // Test unmount_dir function
        newroot_mount.unmount_all().unwrap();

        // Validate that the device has been successfully unmounted
        assert!(
            !is_device_mounted_at(&loop_device, mount_point),
            "Device '{loop_device}' still mounted at '{}'",
            mount_point.display()
        );
    }

    /// Checks if a device is mounted at a given mount point
    #[cfg(test)]
    fn is_device_mounted_at(device: impl AsRef<Path>, mount_point: impl AsRef<Path>) -> bool {
        let mounts = fs::read_to_string("/proc/mounts").expect("Failed to read /proc/mounts");
        for line in mounts.lines() {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2
                && parts[0] == device.as_ref().to_string_lossy()
                && parts[1] == mount_point.as_ref().to_string_lossy()
            {
                return true;
            }
        }
        false
    }

    /// Identifies the loop device associated with a given file
    #[cfg(test)]
    fn find_loop_device(file_path: &Path) -> Result<String, Error> {
        use std::process::Command;

        let output = Command::new("losetup")
            .arg("-j")
            .arg(file_path)
            .output()
            .context("Failed to execute losetup command")?;

        let output_str =
            String::from_utf8(output.stdout.clone()).context("Failed to parse losetup output")?;

        // Extract the loop device name from the losetup output
        output_str
            .lines()
            .next()
            .and_then(|line| line.split(':').next())
            .map(String::from)
            .ok_or_else(|| Error::msg("Failed to find loop device in losetup output"))
    }

    #[functional_test(feature = "engine", negative = true)]
    fn test_mount_failure() {
        let temp_mount_dir = TempDir::new().unwrap();

        // bad mount path
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/sr"),
                        partitions: vec![Partition {
                            id: "sr0".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 0.into(),
                        }],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![config::InternalMountPoint {
                        path: PathBuf::from("foobar"),
                        target_id: "sr0".to_string(),
                        filesystem: FileSystemType::Iso9660,
                        options: vec!["bad-options".into()],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/sr"),
                "sr0".into() => PathBuf::from(CDROM_DEVICE_PATH)
            },
            ..Default::default()
        };

        let mut newroot_mount = NewrootMount::new(temp_mount_dir.path().to_owned());

        assert_eq!(
            newroot_mount
                .mount_newroot_partitions(
                    &host_status.spec,
                    &host_status.block_device_paths,
                    AbVolumeSelection::VolumeA
                )
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::MountNewroot)
        );

        // bad root path
        let mut value = host_status.spec.storage.internal_mount_points.remove(0);
        value.path = PathBuf::from("/");
        host_status.spec.storage.internal_mount_points.push(value);
        let temp_file = NamedTempFile::new().unwrap();

        let mut newroot_mount = NewrootMount::new(temp_file.path().to_owned());

        assert_eq!(
            newroot_mount
                .mount_newroot_partitions(
                    &host_status.spec,
                    &host_status.block_device_paths,
                    AbVolumeSelection::VolumeA
                )
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::MountNewroot)
        );
    }

    #[functional_test(feature = "engine", negative = true)]
    fn test_umount_failure() {
        // Create a valid temporary directory
        let temp_mount_dir = TempDir::new().unwrap();
        let temp_mount_path = temp_mount_dir.path().to_owned();

        // Test case 1: Attempt to unmount an existing directory that isn't mounted and assert that
        // it fails
        let mut root_mount = NewrootMount::new(temp_mount_path.clone());
        root_mount.add_mount(temp_mount_path.clone());
        let umount_result_1 = root_mount.unmount_all();

        assert_eq!(
            umount_result_1.unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::UnmountNewroot {
                dir: temp_mount_path.to_string_lossy().into(),
            })
        );

        // Test case 2: Attempt to unmount a directory that does not exist
        let mut root_mount2 = NewrootMount::new(temp_mount_path.clone());
        root_mount2.add_mount(PathBuf::from("/path/to/non/existent/directory"));
        let umount_result_2 = root_mount2.unmount_all();

        assert_eq!(
            umount_result_2.unwrap_err().kind(),
            &ErrorKind::Servicing(ServicingError::UnmountNewroot {
                dir: "/path/to/non/existent/directory".to_string(),
            })
        );
    }

    #[functional_test(feature = "engine", negative = true)]
    fn test_mount_with_populated_dir_failure() {
        // Mount point
        let temp_mount_dir = TempDir::new().unwrap();

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from("/dev/sr"),
                        partitions: vec![Partition {
                            id: "sr0".to_string(),
                            partition_type: PartitionType::Esp,
                            size: 0.into(),
                        }],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![config::InternalMountPoint {
                        path: PathBuf::from("/"),
                        target_id: "sr0".to_string(),
                        filesystem: FileSystemType::Iso9660,
                        options: vec![MOUNT_OPTION_READ_ONLY.into()],
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/sr"),
                "sr0".into() => PathBuf::from("/dev/sr0")
            },
            ..Default::default()
        };

        // Create the mount point directory if it doesn't exist yet
        // Add a file to the mount point directory to simulate a populated directory
        let temp_mount_point_file = temp_mount_dir.path().join("temp_file");
        File::create(temp_mount_point_file).unwrap();

        // Attempt to mount the CDROM device to the mount point and assert that it fails
        let mut newroot_mount = NewrootMount::new(temp_mount_dir.path().to_owned());

        assert_eq!(
            newroot_mount
                .mount_newroot_partitions(
                    &host_status.spec,
                    &host_status.block_device_paths,
                    AbVolumeSelection::VolumeA
                )
                .expect_err(
                    "Expected mount_new_root to fail because of populated directory as path"
                )
                .kind(),
            &ErrorKind::Servicing(ServicingError::MountNewroot)
        );
    }

    #[functional_test(feature = "engine")]
    fn test_newroot_mount_drop() {
        let temp_mount_dir = TempDir::new().unwrap();

        // Full path to where we expect the tmpfs to be mounted
        let temp_mount_path = temp_mount_dir.path().join("tmp");

        {
            // Create a new NewrootMount object
            let mut newroot_mount = NewrootMount::new(temp_mount_dir.path().to_owned());

            // Create the /tmp directory
            fs::create_dir_all(temp_mount_dir.path().join("tmp")).unwrap();

            // Mount tmpfs for /tmp
            newroot_mount.mount_tmpfs("/tmp").unwrap();

            assert_eq!(newroot_mount.mounts.len(), 1);
            assert_eq!(newroot_mount.mounts[0], temp_mount_path);

            let root_mount = FindMnt::run().unwrap().root().unwrap();
            assert!(root_mount.contains_mountpoint(&temp_mount_path));
        }

        let root_mount = FindMnt::run().unwrap().root().unwrap();
        assert!(!root_mount.contains_mountpoint(&temp_mount_path));
    }

    #[functional_test(feature = "engine")]
    fn test_mount_newroot_tmpfs() {
        let temp_mount_dir = TempDir::new().unwrap();

        // Create a new NewrootMount object
        let mut newroot_mount = NewrootMount::new(temp_mount_dir.path().to_owned());

        // Full path to where we expect the tmpfs to be mounted
        let temp_mount_path = temp_mount_dir.path().join("tmp");

        // Create the /tmp directory
        fs::create_dir_all(temp_mount_dir.path().join("tmp")).unwrap();

        // Mount tmpfs for /tmp
        newroot_mount.mount_tmpfs("/tmp").unwrap();

        assert_eq!(newroot_mount.mounts.len(), 1);
        assert_eq!(newroot_mount.mounts[0], temp_mount_path);

        let root_mount = FindMnt::run().unwrap().root().unwrap();
        assert!(root_mount.contains_mountpoint(&temp_mount_path));

        // Unmount the tmpfs
        newroot_mount.unmount_all().unwrap();

        let root_mount = FindMnt::run().unwrap().root().unwrap();
        assert!(!root_mount.contains_mountpoint(&temp_mount_path));
    }
}
