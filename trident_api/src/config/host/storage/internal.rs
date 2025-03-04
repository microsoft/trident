use std::path::PathBuf;

use log::trace;

use crate::{config::FileSystemType, BlockDeviceId};

use super::Storage;

/// Mount point configuration.
///
/// These are used by Trident to update the `/etc/fstab` in the runtime OS to
/// correctly mount the volumes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalMountPoint {
    /// The path of the mount point.
    ///
    /// This is the path where the volume will be mounted in the runtime OS.
    /// For `swap` partitions, the path should be `none`.
    pub path: PathBuf,

    /// The filesystem to be used for this mount point.
    ///
    /// This value will be used to format the partition.
    pub filesystem: FileSystemType,

    /// A list of options to be used for this mount point.
    ///
    /// These will be passed as is to the `/etc/fstab` file.
    pub options: Vec<String>,

    /// The id of the block device that will be mounted at this mount
    /// point.
    ///
    /// This parameter is required. It must be the ID of a disk partition,
    /// encrypted volume, software raid array, or A/B update volume pair.
    pub target_id: BlockDeviceId,
}

impl Storage {
    /// Populate internal storage configuration.
    ///
    /// This function assumes that the storage configuration has been validated.
    ///
    /// The function will populate:
    /// - `internal_mount_points` with the mount points to be created
    ///
    /// Based on the external API fields:
    /// - `filesystems`
    pub fn populate_internal(&mut self) {
        // Clear any previous internal configuration
        self.internal_mount_points.clear();

        // First, go over all filesystems
        self.filesystems.iter().for_each(|fs| {
            let device_id = fs.device_id.as_deref().unwrap_or_default();

            if let Some(mp) = fs.mount_point.as_ref() {
                self.internal_mount_points.push(InternalMountPoint {
                    path: mp.path.clone(),
                    filesystem: fs.fs_type,
                    options: mp.options.to_string_vec(),
                    target_id: device_id.to_string(),
                });
            // In the new API swap partitions don't have mount points, so we
            // have to fill them in.
            } else if fs.fs_type == FileSystemType::Swap {
                self.internal_mount_points.push(InternalMountPoint {
                    path: PathBuf::from(crate::constants::SWAP_MOUNT_POINT),
                    filesystem: FileSystemType::Swap,
                    options: vec!["sw".to_string()],
                    target_id: device_id.to_string(),
                });
            }
        });

        trace!(
            "Internal mount point configuration:\n{:#?}",
            self.internal_mount_points
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        config::{
            FileSystem, FileSystemSource, Image, ImageFormat, ImageSha256, MountOptions, MountPoint,
        },
        constants::SWAP_MOUNT_POINT,
    };

    use super::*;

    #[test]
    fn test_populate_internal_regular() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: Some("/dev/sda1".to_string()),
                fs_type: FileSystemType::Ext4,
                source: FileSystemSource::Image(Image {
                    url: "file:///path/to/image".to_string(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                }),
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/mnt/data"),
                    options: MountOptions::defaults(),
                }),
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from("/mnt/data"),
                filesystem: FileSystemType::Ext4,
                options: vec!["defaults".to_string()],
                target_id: "/dev/sda1".to_string(),
            }]
        );
    }

    #[test]
    fn test_populate_internal_regular_imageless() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: Some("/dev/sda1".to_string()),
                fs_type: FileSystemType::Ext4,
                source: FileSystemSource::New,
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/mnt/data"),
                    options: MountOptions::defaults(),
                }),
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from("/mnt/data"),
                filesystem: FileSystemType::Ext4,
                options: vec!["defaults".to_string()],
                target_id: "/dev/sda1".to_string(),
            }]
        );
    }

    #[test]
    fn test_populate_internal_regular_mountpointless() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: Some("/dev/sda1".to_string()),
                fs_type: FileSystemType::Ext4,
                source: FileSystemSource::Image(Image {
                    url: "file:///path/to/image".to_string(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                }),
                mount_point: None,
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert!(storage.internal_mount_points.is_empty());
    }

    #[test]
    fn test_populate_internal_swap() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: Some("/dev/sda1".to_string()),
                fs_type: FileSystemType::Swap,
                source: FileSystemSource::New,
                mount_point: None,
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from(SWAP_MOUNT_POINT),
                filesystem: FileSystemType::Swap,
                options: vec!["sw".to_string()],
                target_id: "/dev/sda1".to_string(),
            }]
        );
    }

    #[test]
    fn test_populate_internal_tmpfs() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: None,
                fs_type: FileSystemType::Tmpfs,
                source: FileSystemSource::New,
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/tmp"),
                    options: MountOptions::defaults(),
                }),
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from("/tmp"),
                filesystem: FileSystemType::Tmpfs,
                options: vec!["defaults".to_string()],
                target_id: "".into(),
            }]
        );
    }

    #[test]
    fn test_populate_internal_overlay() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: None,
                fs_type: FileSystemType::Overlay,
                source: FileSystemSource::New,
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/usr/path/data"),
                    options: MountOptions::new(
                        "defaults,lowerdir=/usr/path/data,upperdir=/mnt/data-overlay",
                    ),
                }),
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from("/usr/path/data"),
                filesystem: FileSystemType::Overlay,
                options: vec![
                    "defaults".into(),
                    "lowerdir=/usr/path/data".into(),
                    "upperdir=/mnt/data-overlay".into(),
                ],
                target_id: "".to_string(),
            }]
        );
    }
}
