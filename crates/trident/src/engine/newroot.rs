use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{anyhow, bail, ensure, Context, Error};
use log::{debug, error, trace, warn};
use sys_mount::{MountBuilder, MountFlags};

use osutils::{files, filesystems::MountFileSystemType, findmnt::FindMnt, lsblk, mount, path};
use sysdefs::filesystems::{KernelFilesystemType, RealFilesystemType};
use trident_api::{
    config::{FileSystem, HostConfiguration},
    constants::{
        NONE_MOUNT_POINT, ROOT_MOUNT_POINT_PATH, UPDATE_ROOT_FALLBACK_PATH, UPDATE_ROOT_PATH,
    },
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::AbVolumeSelection,
    BlockDeviceId,
};

use crate::{OS_MODIFIER_BINARY_PATH, OS_MODIFIER_NEWROOT_PATH};

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

    /// List of active mount points being managed by the NewrootMount object.
    mounts: Vec<PathBuf>,
}

impl NewrootMount {
    /// Construct a simple NewrootMount object with the given newroot path.
    fn new(newroot_mount_path: PathBuf) -> Self {
        Self {
            newroot_mount_path,
            mounts: Vec::new(),
        }
    }

    /// Given an engine context, creates all the required mount points for newroot and returns a
    /// NewrootMount object.
    #[tracing::instrument(name = "initialize_new_root", skip_all)]
    pub fn create_and_mount(
        host_config: &HostConfiguration,
        partition_paths: &BTreeMap<BlockDeviceId, PathBuf>,
        update_volume: AbVolumeSelection,
    ) -> Result<Self, TridentError> {
        // Get the path where the newroot should be mounted
        let new_root_path = get_new_root_path();
        debug!(
            "Attempting to mount newroot at '{}'",
            new_root_path.display()
        );

        // Create a base NewrootMount object. We create it early so we can
        // leverage the Drop trait to unmount everything in newroot in case of an error
        // while mounting.
        let mut newroot_mount = NewrootMount::new(new_root_path);

        newroot_mount
            .mount_newroot_partitions(host_config, partition_paths, update_volume)
            .message("Failed to mount all partitions in newroot")?;

        // Mount tmpfs for /tmp and /run
        newroot_mount.mount_tmpfs("/tmp")?;
        newroot_mount.mount_tmpfs("/run")?;

        if Path::new(OS_MODIFIER_BINARY_PATH).exists() {
            // Bind mount the execroot binary to the newroot
            debug!("Bind mounting osmodifier binary into newroot");
            let mount_path = path::join_relative(newroot_mount.path(), OS_MODIFIER_NEWROOT_PATH);

            fs::write(&mount_path, b"").structured(ServicingError::MountExecrootBinary)?;

            MountBuilder::default()
                .flags(MountFlags::BIND)
                .mount(OS_MODIFIER_BINARY_PATH, &mount_path)
                .structured(ServicingError::MountExecrootBinary)?;
            newroot_mount.mounts.push(mount_path);
        } else {
            debug!("Skipping bind mount of osmodifier binary into newroot");
        }

        Ok(newroot_mount)
    }

    /// Returns the absolute path in the host to newroot's mount point.
    ///
    /// E.g.: `/mnt/newroot`
    pub fn path(&self) -> &Path {
        &self.newroot_mount_path
    }

    fn unmount_all_impl(&mut self) -> Result<(), TridentError> {
        // Unmount all mounts in reverse order.
        for mount in self.mounts.drain(..).rev() {
            debug!("Unmounting '{}'", mount.display());
            // Try up to 5 times to unmount, allowing for the
            // async nature of umount.
            for retry_count in 1..6 {
                if retry_count != 1 {
                    trace!("Unmounting '{}' attempt {}", mount.display(), retry_count);
                }
                let ret = mount::umount(&mount, false);
                if ret.is_ok() {
                    break;
                } else if retry_count == 5 {
                    return ret.structured(ServicingError::UnmountNewroot {
                        dir: mount.to_string_lossy().into(),
                    });
                } else {
                    thread::sleep(Duration::from_millis(100));
                }
            }
            trace!("Unmounted '{}' successfully", mount.display());
        }
        Ok(())
    }

    /// Unmount all registered mounts in the correct order.
    #[tracing::instrument(name = "newroot_unmount", skip_all)]
    pub fn unmount_all(mut self) -> Result<(), TridentError> {
        debug!("Unmounting newroot at '{}'", self.path().display());
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
        partition_paths: &BTreeMap<BlockDeviceId, PathBuf>,
        update_volume: AbVolumeSelection,
    ) -> Result<(), TridentError> {
        let mut block_device_paths = partition_paths.clone();

        for raid in &host_config.storage.raid.software {
            block_device_paths.insert(raid.id.clone(), raid.device_path());
        }

        if let Some(encryption) = &host_config.storage.encryption {
            for volume in &encryption.volumes {
                block_device_paths.insert(volume.id.clone(), volume.device_path());
            }
        }

        for verity in &host_config.storage.verity {
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
            .try_for_each(|(path, fs)| {
                // target_id may be None if mounting Overlay or Tmpfs
                let target_id = fs.device_id.as_deref().unwrap_or_default();
                let Some(mp) = fs.mount_point.as_ref() else {
                    return Ok(());
                };
                let target_path =
                    self.path()
                        .join(path.strip_prefix(ROOT_MOUNT_POINT_PATH).context(format!(
                            "Failed to strip prefix '{}' from '{}'",
                            ROOT_MOUNT_POINT_PATH,
                            path.display()
                        ))?);

                debug!(
                    "Mounting block device '{}' to '{}'",
                    target_id,
                    target_path.display()
                );

                prepare_mount_directory(&target_path, false).with_context(||format!(
                    "Failed to prepare mount directory for block device '{target_id}'"
                ))?;

                let device_path = block_device_paths.get(target_id).context(format!(
                    "Failed to find block device path for id '{target_id}'"
                ))?;

                // Check if block device is already mounted
                let block_device = lsblk::get(device_path).with_context(|| {
                    format!("Failed to get info about block device '{target_id}'")
                })?;

                let fs_type = block_device.fstype.and_then(|fs_type| KernelFilesystemType::from(fs_type.as_str()).try_as_real());

                // If a filesystem is of type NTFS and the device is already mounted, need to use a
                // private bind mount instead, b/c NTFS doesn't support multiple mounts.
                match (should_be_bind_mounted(fs_type), block_device.mountpoint) {
                    (true, Some(mp_path)) => {
                        // Issue a warning to inform the user that we are creating a private bind
                        // mount, instead of the "regular" mount.
                        warn!(
                            "Block device '{}' with an NTFS filesystem is already mounted at '{}', but NTFS does not support multiple mounts.\nCreating a private bind mount at '{}' instead",
                            target_id,
                            mp_path.display(),
                            target_path.display()
                        );

                        // Fetch mount options from the existing mount
                        let flags = if block_device.readonly {
                            MountFlags::RDONLY
                        } else {
                            MountFlags::empty()
                        };

                        // Do a private non-recursive bind mount
                        do_bind_mount(&mp_path, &target_path, flags).with_context(|| {
                            format!(
                                "Failed to bind mount '{}' to '{}'",
                                mp_path.display(),
                                target_path.display(),
                            )
                        })?;
                    },
                    _ => {
                        mount::mount(
                            device_path,
                            &target_path,
                            MountFileSystemType::Auto,
                            &mp.options.to_string_vec(),
                        )
                        .context(format!(
                            "Failed to mount block device '{}' with device path '{}' to '{}'",
                            target_id,
                            device_path.display(),
                            target_path.display()
                        ))?;
                    }
                }

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

        if !target_path_full.exists() {
            fs::create_dir(&target_path_full).structured(ServicingError::MountNewrootSpecialDir {
                dir: target_path_full.clone().to_string_lossy().to_string(),
            })?;
        }

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

/// Only NTFS should be bind mounted. Also return true for Fuseblk for cover
/// case where lsblk reports that an NTFS filesystem has type Fuseblk.
fn should_be_bind_mounted(fs_type: Option<RealFilesystemType>) -> bool {
    let Some(real_fs_type) = fs_type else {
        return false;
    };
    match real_fs_type {
        RealFilesystemType::Ntfs | RealFilesystemType::Fuseblk => true,
        RealFilesystemType::Btrfs
        | RealFilesystemType::Cramfs
        | RealFilesystemType::Exfat
        | RealFilesystemType::Ext2
        | RealFilesystemType::Ext3
        | RealFilesystemType::Ext4
        | RealFilesystemType::Iso9660
        | RealFilesystemType::Msdos
        | RealFilesystemType::Squashfs
        | RealFilesystemType::Udf
        | RealFilesystemType::Vfat
        | RealFilesystemType::Xfs => false,
    }
}

/// Returns an ordered map of mount points to their corresponding FileSystem objects.
fn mount_points_map(host_config: &HostConfiguration) -> BTreeMap<&Path, &FileSystem> {
    host_config
        .storage
        .filesystems
        .iter()
        .filter_map(|fs| {
            if let Some(mpp) = fs.mount_point_path() {
                return Some((mpp, fs));
            };
            None
        })
        .filter(|(path, _)| path.as_os_str() != NONE_MOUNT_POINT)
        .collect::<BTreeMap<_, _>>()
}

/// Returns the path where the newroot should be mounted.
fn get_new_root_path() -> PathBuf {
    let mut new_root_path = Path::new(UPDATE_ROOT_PATH);
    if let Err(e) = prepare_mount_directory(new_root_path, true) {
        debug!(
            "Failed to prepare new root mount directory at '{}'. Error: {}.",
            new_root_path.display(),
            e
        );
        debug!("Falling back to '{}'", UPDATE_ROOT_FALLBACK_PATH);
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

/// Verify that target_path is not within a read-only directory or mounted filesystem.
fn verify_write_access(target_path: &Path) -> Result<(), Error> {
    // TODO (TASK 11038): Use InvalidMountDirectoryPath from InvalidInputError instead of current error handling

    // Check if the target path is within a read-only directory
    let mut existing_ancestor = target_path.parent().with_context(|| {
        format!(
            "The target path {} has no parent directory to be built into",
            target_path.display()
        )
    })?;
    // Find the first existing parent directory
    while !existing_ancestor.exists() {
        existing_ancestor = existing_ancestor.parent().with_context(|| {
            format!(
                "The target path {} has no existent parent directory to be built into",
                target_path.display()
            )
        })?;
    }
    // Check if the existing parent directory is read-only
    if fs::metadata(existing_ancestor)
        .with_context(|| {
            format!(
                "Failed to get metadata for '{}'",
                existing_ancestor.display()
            )
        })?
        .permissions()
        .readonly()
    {
        bail!(format!(
            "Failed to create directory '{}' for mount path because the \
            parent directory '{}' is read-only.",
            target_path.display(),
            existing_ancestor.display()
        ));
    }

    // Check if the target path is within a read-only mounted filesystem
    let metadata = FindMnt::run()
        .context("Failed to get current mount points")?
        .root()
        .context("Failed to get root mount point")?;
    // Fail in case the target path is within a read-only mounted filesystem
    if let Some(mount_point) = metadata.find_mount_point_for_path(target_path) {
        if mount_point.options.contains("ro") && target_path != mount_point.target {
            bail!(format!(
                "Failed to create the directory '{}' because the mount path \
                        is within a read-only mounted filesystem at '{}'.",
                target_path.display(),
                mount_point.target.display()
            ));
        }
    }

    Ok(())
}

/// Verify that target_path is suitable for a mount point. If the directory does
/// not exist, ensure it can be created and then attempt to do so with a warning.
fn prepare_mount_directory(target_path: &Path, is_newroot: bool) -> Result<(), Error> {
    if target_path.exists() {
        ensure!(
            target_path.is_dir(),
            "Mount path '{}' is not a directory",
            target_path.display()
        );
        // Check if the directory is empty
        if let Ok(entries) = fs::read_dir(target_path) {
            let entries_list = entries
                .filter_map(|e| match e {
                    Ok(ee) => Some(ee.path().to_string_lossy().into_owned()),
                    Err(err) => {
                        warn!(
                            "Failed to read entry in mount path '{}': {}",
                            target_path.display(),
                            err
                        );
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");
            if !entries_list.is_empty() {
                error!(
                    "Mount path '{}' already exists and is non-empty: {}\n",
                    target_path.display(),
                    entries_list
                );
                return Err(anyhow!(
                    "Mount path '{}' is not empty",
                    target_path.display()
                ));
            }
        }
        Ok(())
    } else {
        if !is_newroot {
            warn!(
                "Mount target: '{}' does not exist. Attempting to create it.",
                target_path.display()
            );
        }
        verify_write_access(target_path)?;
        files::create_dirs(target_path)
            .with_context(|| format!("Failed to create mount path '{}'", target_path.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs::File, path::PathBuf};

    use tempfile::TempDir;

    use trident_api::config::{
        FileSystemSource, HostConfiguration, MountOptions, MountPoint, Storage,
    };

    #[test]
    fn test_mount_point_ordering() {
        let host_config = HostConfiguration {
            storage: Storage {
                filesystems: vec![
                    FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/mnt/boot/efi"),
                            options: MountOptions::empty(),
                        }),
                        device_id: Some("sda3".to_string()),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/mnt"),
                            options: MountOptions::empty(),
                        }),
                        device_id: Some("sda1".to_string()),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/a"),
                            options: MountOptions::empty(),
                        }),
                        device_id: Some("sda1".to_string()),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/"),
                            options: MountOptions::empty(),
                        }),
                        device_id: Some("sda1".to_string()),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/mnt/boot"),
                            options: MountOptions::empty(),
                        }),
                        device_id: Some("sda2".to_string()),
                        source: FileSystemSource::Image,
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
    }

    #[test]
    fn test_prepare_mount_directory() {
        let temp_mount_dir = TempDir::new().unwrap();

        // Test case 1: Prepare a directory that exists and is empty
        prepare_mount_directory(temp_mount_dir.path(), true).unwrap();

        // Test case 2: Prepare a directory that does not exist
        let temp_mount_point_dir = temp_mount_dir.path().join("temp_dir");
        prepare_mount_directory(&temp_mount_point_dir, true).unwrap();
        assert!(temp_mount_point_dir.exists());

        // Test case 3: Prepare a directory that exists and is not empty
        assert_eq!(
            prepare_mount_directory(temp_mount_dir.path(), false)
                .unwrap_err()
                .to_string(),
            format!(
                "Mount path '{}' is not empty",
                temp_mount_dir.path().display()
            )
        );

        // Test case 4: Prepare a file path does not work
        let temp_mount_point_file = temp_mount_dir.path().join("temp_file");
        File::create(&temp_mount_point_file).unwrap();
        assert_eq!(
            prepare_mount_directory(&temp_mount_point_file, false)
                .unwrap_err()
                .to_string(),
            format!(
                "Mount path '{}' is not a directory",
                temp_mount_point_file.display()
            )
        );

        // Test case 5: Prepare a directory with no existent parent directory
        let non_valid_path = PathBuf::from("non_existent_dir/new_dir");
        assert_eq!(
            prepare_mount_directory(&non_valid_path, false)
                .unwrap_err()
                .to_string(),
            format!(
                "The target path {} has no existent parent directory to be built into",
                non_valid_path.display()
            )
        );

        // Test case 6: Prepare a directory inside a read-only directory
        let mut permissions_temp_mount_dir =
            fs::metadata(temp_mount_dir.path()).unwrap().permissions();
        // Set the permissions for parent directory to read-only
        permissions_temp_mount_dir.set_readonly(true);
        fs::set_permissions(temp_mount_dir.path(), permissions_temp_mount_dir).unwrap();
        // Target path within read-only directory
        let temp_in_ro = temp_mount_dir.path().join("new_dir_in_ro");
        assert_eq!(
            prepare_mount_directory(&temp_in_ro, false)
                .unwrap_err()
                .to_string(),
            format!(
                "Failed to create directory '{}' for mount path because the parent directory '{}' is read-only.",
                temp_in_ro.display(),
                temp_mount_dir.path().display()
            )
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use crate::engine::EngineContext;

    use super::*;

    use std::{
        fs::{self, File},
        io::Write,
        path::{Path, PathBuf},
        str::FromStr,
    };

    use maplit::btreemap;
    use path::join_relative;
    use tempfile::{NamedTempFile, TempDir};

    use osutils::{
        dependencies::Dependency,
        filesystems::MkfsFileSystemType,
        findmnt::FindMnt,
        mkfs, mountpoint,
        repart::{RepartEmptyMode, RepartPartitionEntry, SystemdRepartInvoker},
        testutils::{
            repart::{self, OS_DISK_DEVICE_PATH, TEST_DISK_DEVICE_PATH},
            tmp_mount,
        },
        udevadm,
    };
    use pytest_gen::functional_test;
    use sysdefs::partition_types::DiscoverablePartitionType;
    use trident_api::{
        config::{
            self, Disk, FileSystemSource, HostConfiguration, MountOptions, MountPoint, Partition,
            PartitionSize, PartitionType,
        },
        constants::{ESP_RELATIVE_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY},
        error::ErrorKind,
    };

    #[functional_test(feature = "engine")]
    fn test_mount_and_umount() {
        let loopback = repart::make_loopback_filesystem(MkfsFileSystemType::Vfat);

        let loop_device = Dependency::Losetup
            .cmd()
            .arg("-f")
            .arg("--show")
            .arg(loopback.path())
            .output_and_check()
            .unwrap();
        let loop_device = loop_device.trim();

        // Mount point
        let mount_point = Path::new("/mnt/mountpoint");

        if mountpoint::check_is_mountpoint(mount_point).unwrap() {
            mount::umount(mount_point, false).unwrap();
        }

        // Create the mount point directory if it doesn't exist yet
        fs::create_dir_all(mount_point).unwrap();

        let ctx = EngineContext {
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
                    filesystems: vec![FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/"),
                            options: MountOptions(MOUNT_OPTION_READ_ONLY.into()),
                        }),
                        device_id: Some("sr0".to_string()),
                        source: FileSystemSource::Image,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/sr"),
                "sr0".into() => PathBuf::from(&loop_device),
            },
            ..Default::default()
        };

        let mut newroot_mount = NewrootMount::new(mount_point.to_owned());
        newroot_mount
            .mount_newroot_partitions(&ctx.spec, &ctx.partition_paths, AbVolumeSelection::VolumeA)
            .unwrap();

        // Validate that the device has been successfully mounted
        assert!(
            is_device_mounted_at(loop_device, mount_point),
            "Device not mounted at the expected mount point"
        );

        let root_mount_dir = tempfile::tempdir().unwrap();

        // Partition test drive
        let partition_definition = repart::generate_partition_definition_esp_generic();
        let disk_bus_path = PathBuf::from(TEST_DISK_DEVICE_PATH);
        let repart = SystemdRepartInvoker::new(disk_bus_path, RepartEmptyMode::Force)
            .with_partition_entries(partition_definition.clone());
        let repart_result = repart.execute().unwrap();
        udevadm::settle().unwrap();

        let esp_dev = &repart_result[0].node;
        let root_dev = &repart_result[1].node;

        let ctx = EngineContext {
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
                    filesystems: vec![
                        FileSystem {
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/"),
                                options: MountOptions::default(),
                            }),
                            device_id: Some("root".to_string()),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/boot/efi"),
                                options: MountOptions("umask=0077".into()),
                            }),
                            device_id: Some("esp".to_string()),
                            source: FileSystemSource::Image,
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "os".into() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "esp".into() => esp_dev.to_owned(),
                "root".into() => root_dev.to_owned(),
            },
            ..Default::default()
        };

        // Fake file to create in a location where a bootloader would be installed
        let mock_bootloader_path = "EFI/BOOT/bootx64.efi";

        // Set up fake ESP and root partitions
        mkfs::run(esp_dev, MkfsFileSystemType::Vfat).unwrap();
        tmp_mount::mount(esp_dev, MountFileSystemType::Vfat, &[], |mount_dir| {
            files::create_file(mount_dir.join(mock_bootloader_path)).unwrap();
        });

        mkfs::run(root_dev, MkfsFileSystemType::Ext4).unwrap();
        tmp_mount::mount(root_dev, MountFileSystemType::Ext4, &[], |mount_dir| {
            ["/boot/efi", "/etc", "/usr", "/bin", "/lib"]
                .into_iter()
                .for_each(|dir| {
                    files::create_dirs(join_relative(mount_dir, dir)).unwrap();
                });
        });

        // Test recursive mounting
        let mut newroot_mount2 = NewrootMount::new(root_mount_dir.path().to_owned());
        newroot_mount2
            .mount_newroot_partitions(&ctx.spec, &ctx.partition_paths, AbVolumeSelection::VolumeA)
            .unwrap();

        assert!(root_mount_dir
            .path()
            .join(ESP_RELATIVE_MOUNT_POINT_PATH)
            .join(mock_bootloader_path)
            .exists());
        newroot_mount2.unmount_all().unwrap();
        assert!(!root_mount_dir.path().join("boot").exists());

        // Test unmount_dir function
        newroot_mount.unmount_all().unwrap();

        // Validate that the device has been successfully unmounted
        assert!(
            !is_device_mounted_at(loop_device, mount_point),
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
        let output = Dependency::Losetup
            .cmd()
            .arg("-j")
            .arg(file_path)
            .output()
            .context("Failed to execute losetup command")?;

        let output_str = output.output().clone();

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
        let loopback = repart::make_loopback_filesystem(MkfsFileSystemType::Ext4);

        let temp_mount_dir = TempDir::new().unwrap();

        // bad mount path
        let mut ctx = EngineContext {
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
                    filesystems: vec![FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("foobar"),
                            options: MountOptions("bad-options".into()),
                        }),
                        device_id: Some("sr0".to_string()),
                        source: FileSystemSource::Image,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/sr"),
                "sr0".into() => loopback.path().to_owned(),
            },
            ..Default::default()
        };

        let mut newroot_mount = NewrootMount::new(temp_mount_dir.path().to_owned());

        assert_eq!(
            newroot_mount
                .mount_newroot_partitions(
                    &ctx.spec,
                    &ctx.partition_paths,
                    AbVolumeSelection::VolumeA
                )
                .unwrap_err()
                .kind(),
            &ErrorKind::Servicing(ServicingError::MountNewroot)
        );

        // bad root path
        let mut value = ctx.spec.storage.filesystems.remove(0);
        value.mount_point = Some(MountPoint {
            path: PathBuf::from("/"),
            options: MountOptions("bad-options".into()),
        });
        ctx.spec.storage.filesystems.push(value);
        let temp_file = NamedTempFile::new().unwrap();

        let mut newroot_mount = NewrootMount::new(temp_file.path().to_owned());

        assert_eq!(
            newroot_mount
                .mount_newroot_partitions(
                    &ctx.spec,
                    &ctx.partition_paths,
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

        let ctx = EngineContext {
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
                    filesystems: vec![FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/"),
                            options: MountOptions(MOUNT_OPTION_READ_ONLY.into()),
                        }),
                        device_id: Some("sr0".to_string()),
                        source: FileSystemSource::Image,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
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
                    &ctx.spec,
                    &ctx.partition_paths,
                    AbVolumeSelection::VolumeA
                )
                .expect_err(
                    "Expected mount_new_root to fail because of populated directory as path"
                )
                .kind(),
            &ErrorKind::Servicing(ServicingError::MountNewroot)
        );
    }

    /// This function wipes the /dev/sdb device and ensures the /mnt
    /// directory exists.
    fn setup_test() {
        // Just zero-out the metadata so this is a fast operation.
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
        if !Path::new("/mnt").exists() {
            Dependency::Mkdir.cmd().arg("/mnt").run_and_check().unwrap();
        }
    }

    #[functional_test(feature = "engine")]
    fn test_mount_newroot_partitions_ntfs() {
        setup_test();

        // NTFS requires partitions, so create partitions on block device
        let repart = SystemdRepartInvoker::new(TEST_DISK_DEVICE_PATH, RepartEmptyMode::Force)
            .with_partition_entries(vec![RepartPartitionEntry {
                id: "1".to_string(),
                partition_type: DiscoverablePartitionType::Root,
                label: Some("1".to_string()),
                size_max_bytes: Some(10 * 1048576),
                size_min_bytes: Some(10 * 1048576),
            }]);
        let partition1 = &repart.execute().unwrap()[0];
        let ntfs_device = Path::new(&partition1.node);

        // Wait for udev to process pending events, so that the system recognizes the new partition
        udevadm::settle().unwrap();

        // Create an NTFS filesystem on the partition
        mkfs::run(ntfs_device, MkfsFileSystemType::Ntfs).unwrap();

        let ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "os".to_string(),
                        device: PathBuf::from(OS_DISK_DEVICE_PATH),
                        partitions: vec![Partition {
                            id: "staging".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: PartitionSize::from_str("1M").unwrap(),
                        }],
                        ..Default::default()
                    }],
                    filesystems: vec![FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/mnt/staging"),
                            options: MountOptions::empty(),
                        }),
                        device_id: Some("staging".to_string()),
                        source: FileSystemSource::Image,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "os".into() => PathBuf::from("OS_DISK_DEVICE_PATH"),
                "staging".into() => ntfs_device.to_path_buf()
            },
            ..Default::default()
        };

        // Create a temp directory to mount the NTFS partition
        let temp_mount_dir = TempDir::new().unwrap();
        // Create a full path to the mount point of the NTFS partition
        let mount_point = path::join_relative(temp_mount_dir.path(), Path::new("/mnt/staging"));
        // Create the mount point if it doesn't exist
        fs::create_dir_all(&mount_point).unwrap();
        if mountpoint::check_is_mountpoint(&mount_point).unwrap() {
            mount::umount(&mount_point, false).unwrap();
        }

        // Create a new NewrootMount object
        let mut newroot_mount = NewrootMount::new(temp_mount_dir.path().to_owned());
        // Mount NTFS partition
        newroot_mount
            .mount_newroot_partitions(&ctx.spec, &ctx.partition_paths, AbVolumeSelection::VolumeA)
            .unwrap();

        // If device is a file, fetch the name of loop device that was mounted at mount point;
        // otherwise, use the device path itself
        let loop_device = if ntfs_device.is_file() {
            find_loop_device(ntfs_device).unwrap()
        } else {
            ntfs_device.to_string_lossy().to_string()
        };

        // Validate that the device has been successfully mounted
        assert!(
            is_device_mounted_at(loop_device.clone(), &mount_point),
            "Device '{}' is not mounted at the expected mount point '{}'",
            loop_device,
            mount_point.display()
        );

        // Create a test file inside the mounted directory
        let test_file_path = mount_point.join("test_file");
        let mut test_file = File::create(test_file_path).unwrap();
        test_file.write_all(b"Hello, world!").unwrap();

        // Now, try to mount the same NTFS partition to a different mount point.
        // Create a new temp directory to mount the NTFS partition
        let temp_mount_dir2 = TempDir::new().unwrap();
        // Create a full path to the mount point of the NTFS partition
        let mount_point2 = path::join_relative(temp_mount_dir2.path(), Path::new("/mnt/staging"));
        // Create the mount point if it doesn't exist
        fs::create_dir_all(&mount_point2).unwrap();
        if mountpoint::check_is_mountpoint(&mount_point2).unwrap() {
            mount::umount(&mount_point2, false).unwrap();
        }

        // Create a new NewrootMount object
        let mut newroot_mount2 = NewrootMount::new(temp_mount_dir2.path().to_owned());
        // Re-mount the NTFS partition
        newroot_mount2
            .mount_newroot_partitions(&ctx.spec, &ctx.partition_paths, AbVolumeSelection::VolumeA)
            .unwrap();

        // Validate that the device has been successfully mounted
        assert!(
            is_device_mounted_at(loop_device.clone(), &mount_point2),
            "Device '{}' is not mounted at the expected mount point '{}'",
            loop_device,
            mount_point2.display()
        );

        // Ensure that the bind-mounted directory contains the test file, too
        let test_file_path2 = mount_point2.join("test_file");
        assert!(test_file_path2.exists());
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

    /// Attempt to prepare a directory within a read-only mounted filesystem
    #[functional_test(feature = "engine", negative = true)]
    fn test_prepare_mount_directory_ro() {
        let loopback = repart::make_loopback_filesystem(MkfsFileSystemType::Vfat);

        // Target path for the mount
        let temp_dir = TempDir::new().unwrap();
        let mount_point = temp_dir.path().join("mount_point");
        fs::create_dir_all(&mount_point).unwrap();

        // Mount the CDROM device and attempt to prepare a directory inside the read-only mount
        tmp_mount::mount(
            loopback.path(),
            MountFileSystemType::Vfat,
            &["ro".into()],
            |mount_dir| {
                // Target path within the read-only mount
                let target_path = mount_dir.join("test_dir");

                assert_eq!(
                    prepare_mount_directory(&target_path, false)
                        .unwrap_err()
                        .to_string(),
                    format!(
                        "Failed to create the directory '{}' because the mount path \
                        is within a read-only mounted filesystem at '{}'.",
                        target_path.display(),
                        mount_dir.display()
                    )
                );
            },
        );
    }
}
