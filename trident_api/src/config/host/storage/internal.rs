use std::path::PathBuf;

use log::trace;

use crate::{
    config::{FileSystemSource, FileSystemType, ImageFormat, ImageSha256},
    misc::IdGenerator,
    BlockDeviceId,
};

use super::Storage;

/// Verity configuration for a volume.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct InternalVerityDevice {
    /// Block device id of the verity device
    pub id: BlockDeviceId,

    /// Name of the verity device, used for the device mapper name
    pub device_name: String,

    /// Block device id of the data block device
    pub data_target_id: BlockDeviceId,

    /// Block device id of the hash block device
    pub hash_target_id: BlockDeviceId,
}

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
    /// encrypted volume, software raid array, or a/b update volume pair.
    pub target_id: BlockDeviceId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalImage {
    /// The URL of the image.
    ///
    /// Supported schemes are: `file`, `http`, and `https`.
    pub url: String,

    /// The SHA256 checksum of the compressed image.
    pub sha256: ImageSha256,

    /// The format of the image.
    pub format: ImageFormat,

    /// The ID of the partition that will be used to store the image.
    pub target_id: BlockDeviceId,
}

impl Storage {
    /// Populate internal storage configuration.
    ///
    /// This function assumes that the storage configuration has been validated.
    ///
    /// The function will populate:
    /// - `images` with the images to be written to the block devices
    /// - `mount_points` with the mount points to be created
    /// - `verity` with the verity devices to be created
    ///
    /// Based on the external API fields:
    /// - `filesystems`
    /// - `verity_filesystems`
    pub fn populate_internal(&mut self) {
        // First, go over all filesystems
        self.filesystems.iter().for_each(|fs| {
            let device_id = fs.device_id.as_deref().unwrap_or_default();

            if let FileSystemSource::Image(img) = &fs.source {
                self.internal_images.push(InternalImage {
                    url: img.url.clone(),
                    sha256: img.sha256.clone(),
                    format: img.format.clone(),
                    target_id: device_id.to_string(),
                });
            }

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

        let mut verity_id_gen = IdGenerator::new("verity");

        // Next, go over all verity filesystems
        for vfs in self.verity_filesystems.iter() {
            let verity_device_id = verity_id_gen.next_id();

            self.internal_verity.push(InternalVerityDevice {
                id: verity_device_id.clone(),
                device_name: vfs.name.clone(),
                data_target_id: vfs.data_device_id.clone(),
                hash_target_id: vfs.hash_device_id.clone(),
            });

            self.internal_images.push(InternalImage {
                url: vfs.data_image.url.clone(),
                sha256: vfs.data_image.sha256.clone(),
                format: vfs.data_image.format.clone(),
                target_id: vfs.data_device_id.clone(),
            });

            self.internal_images.push(InternalImage {
                url: vfs.hash_image.url.clone(),
                sha256: vfs.hash_image.sha256.clone(),
                format: vfs.hash_image.format.clone(),
                target_id: vfs.hash_device_id.clone(),
            });

            self.internal_mount_points.push(InternalMountPoint {
                path: vfs.mount_point.path.clone(),
                filesystem: vfs.fs_type,
                options: vfs.mount_point.options.to_string_vec(),
                target_id: verity_device_id,
            });
        }

        trace!("Internal image configuration:\n{:#?}", self.internal_images);
        trace!(
            "Internal mount point configuration:\n{:#?}",
            self.internal_mount_points
        );
        trace!(
            "Internal verity configuration:\n{:#?}",
            self.internal_verity
        );
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        config::{FileSystem, Image, MountOptions, MountPoint, VerityFileSystem},
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
            storage.internal_images,
            vec![InternalImage {
                url: "file:///path/to/image".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
                target_id: "/dev/sda1".to_string(),
            }]
        );

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from("/mnt/data"),
                filesystem: FileSystemType::Ext4,
                options: vec!["defaults".to_string()],
                target_id: "/dev/sda1".to_string(),
            }]
        );

        assert!(storage.verity_filesystems.is_empty());
    }

    #[test]
    fn test_populate_internal_regular_imageless() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: Some("/dev/sda1".to_string()),
                fs_type: FileSystemType::Ext4,
                source: FileSystemSource::Create,
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/mnt/data"),
                    options: MountOptions::defaults(),
                }),
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert!(storage.internal_images.is_empty());

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from("/mnt/data"),
                filesystem: FileSystemType::Ext4,
                options: vec!["defaults".to_string()],
                target_id: "/dev/sda1".to_string(),
            }]
        );

        assert!(storage.verity_filesystems.is_empty());
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

        assert_eq!(
            storage.internal_images,
            vec![InternalImage {
                url: "file:///path/to/image".to_string(),
                sha256: ImageSha256::Ignored,
                format: ImageFormat::RawZst,
                target_id: "/dev/sda1".to_string(),
            }]
        );

        assert!(storage.internal_mount_points.is_empty());

        assert!(storage.verity_filesystems.is_empty());
    }

    #[test]
    fn test_populate_internal_swap() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: Some("/dev/sda1".to_string()),
                fs_type: FileSystemType::Swap,
                source: FileSystemSource::Create,
                mount_point: None,
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert!(storage.internal_images.is_empty());

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from(SWAP_MOUNT_POINT),
                filesystem: FileSystemType::Swap,
                options: vec!["sw".to_string()],
                target_id: "/dev/sda1".to_string(),
            }]
        );

        assert!(storage.verity_filesystems.is_empty());
    }

    #[test]
    fn test_populate_internal_tmpfs() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: None,
                fs_type: FileSystemType::Tmpfs,
                source: FileSystemSource::Create,
                mount_point: Some(MountPoint {
                    path: PathBuf::from("/tmp"),
                    options: MountOptions::defaults(),
                }),
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert!(storage.internal_images.is_empty());

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from("/tmp"),
                filesystem: FileSystemType::Tmpfs,
                options: vec!["defaults".to_string()],
                target_id: "".into(),
            }]
        );

        assert!(storage.verity_filesystems.is_empty());
    }

    #[test]
    fn test_populate_internal_overlay() {
        let mut storage = Storage {
            filesystems: vec![FileSystem {
                device_id: None,
                fs_type: FileSystemType::Overlay,
                source: FileSystemSource::Create,
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

        assert!(storage.internal_images.is_empty());

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

        assert!(storage.verity_filesystems.is_empty());
    }

    #[test]
    fn test_populate_internal_verity() {
        let mut storage = Storage {
            verity_filesystems: vec![VerityFileSystem {
                name: "my-verity-device".to_string(),
                data_device_id: "/dev/sda1".to_string(),
                hash_device_id: "/dev/sda2".to_string(),
                data_image: Image {
                    url: "file:///path/to/data/image".to_string(),
                    sha256: ImageSha256::Checksum("aaaa".into()),
                    format: ImageFormat::RawZst,
                },
                hash_image: Image {
                    url: "file:///path/to/hash/image".to_string(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                },
                fs_type: FileSystemType::Ext4,
                mount_point: MountPoint {
                    path: PathBuf::from("/"),
                    options: MountOptions::defaults(),
                },
            }],
            ..Default::default()
        };

        storage.populate_internal();

        assert_eq!(
            storage.internal_images,
            vec![
                InternalImage {
                    url: "file:///path/to/data/image".to_string(),
                    sha256: ImageSha256::Checksum("aaaa".into()),
                    format: ImageFormat::RawZst,
                    target_id: "/dev/sda1".to_string(),
                },
                InternalImage {
                    url: "file:///path/to/hash/image".to_string(),
                    sha256: ImageSha256::Ignored,
                    format: ImageFormat::RawZst,
                    target_id: "/dev/sda2".to_string(),
                },
            ]
        );

        assert_eq!(
            storage.internal_mount_points,
            vec![InternalMountPoint {
                path: PathBuf::from("/"),
                filesystem: FileSystemType::Ext4,
                options: vec!["defaults".to_string()],
                target_id: "verity-0".to_string(),
            }]
        );

        assert_eq!(
            storage.internal_verity,
            vec![InternalVerityDevice {
                id: "verity-0".to_string(),
                device_name: "my-verity-device".to_string(),
                data_target_id: "/dev/sda1".to_string(),
                hash_target_id: "/dev/sda2".to_string(),
            }]
        );
    }
}
