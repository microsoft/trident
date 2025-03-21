use std::path::{Path, PathBuf};

use trident_api::{
    config::{FileSystem, FileSystemSource, FileSystemType, MountPoint},
    constants::{ESP_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    error::{InternalError, ReportError, TridentError},
    BlockDeviceId,
};

use crate::engine::EngineContext;

#[derive(Clone)]
pub enum FileSystemData {
    Image(FileSystemDataImage),
    New(FileSystemDataNew),
    Adopted(FileSystemDataAdopted),
    Swap(FileSystemDataSwap),
    Tmpfs(FileSystemDataTmpfs),
    Overlay(FileSystemDataOverlay),
}

#[derive(Clone)]
pub struct FileSystemDataImage {
    /// The mount point of the file system.
    ///
    /// Note: mount_point is required for Image filesystems.
    pub mount_point: MountPoint,

    /// The file system type.
    pub fs_type: FileSystemType,

    /// The id of the block device associated with this filesystem.
    ///
    /// Note: device_id is required for Image filesystems.
    pub device_id: BlockDeviceId,
}

#[derive(Clone)]
pub struct FileSystemDataNew {
    /// The mount point of the file system.
    pub mount_point: Option<MountPoint>,

    /// The file system type.
    pub fs_type: FileSystemType,

    /// The id of the block device associated with this filesystem.
    ///
    /// Note: since Tmpfs, Overlay, and Swap filesystems have a separate
    /// FileSystemData* type, all remaining New filesystems are expected to have
    /// a device ID
    pub device_id: BlockDeviceId,
}

#[derive(Clone)]
pub struct FileSystemDataAdopted {
    /// The mount point of the file system.
    pub mount_point: Option<MountPoint>,

    /// The file system type.
    pub fs_type: FileSystemType,

    /// The id of the block device associated with this filesystem.
    ///
    /// Note: device_id is required for Adopted filesystems.
    pub device_id: BlockDeviceId,
}

/// FileSystemData struct for Tmpfs filesystems.
///
/// Device id is not expected for a Tmpfs filesystem.
#[derive(Clone)]
pub struct FileSystemDataTmpfs {
    /// The mount point of the file system.
    pub mount_point: MountPoint,
}

/// FileSystemData struct for Overlay filesystems.
///
/// Device id is not expected for a Overlay filesystem.
#[derive(Clone)]
pub struct FileSystemDataOverlay {
    /// The mount point of the file system.
    pub mount_point: MountPoint,
}

/// FileSystemData struct for Swap filesystems.
///
/// Swap filesystems cannot have a mount point.
#[derive(Clone)]
pub struct FileSystemDataSwap {
    /// The id of the block device associated with this filesystem.
    ///
    /// Note: device_id is required for Swap filesystems.
    pub device_id: BlockDeviceId,
}

impl FileSystemData {
    /// Because filesystems don't have IDs that can uniquely identify them, this
    /// function can be used to create a description of the specific filesystem
    /// in lieu of an ID.
    pub fn description(&self) -> String {
        [
            (
                "src",
                Some(
                    match &self {
                        FileSystemData::New(_) => "new",
                        FileSystemData::Adopted(_) => "adopted",
                        FileSystemData::Image(_) => "image",
                        FileSystemData::Tmpfs(_) => "new",
                        FileSystemData::Swap(_) => "new",
                        FileSystemData::Overlay(_) => "new",
                    }
                    .to_owned(),
                ),
            ),
            ("type", Some(self.fs_type().to_string())),
            ("dev", self.device_id().cloned()),
            (
                "mnt",
                self.mount_point_path()
                    .map(|mpp| mpp.to_string_lossy().to_string()),
            ),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| format!("{}:{}", k, v)))
        .collect::<Vec<_>>()
        .join(", ")
    }

    /// Returns the inner FileSystemDataImage object, if applicable.
    pub fn as_image(&self) -> Option<&FileSystemDataImage> {
        match self {
            FileSystemData::Image(fs_data_img) => Some(fs_data_img),
            _ => None,
        }
    }

    /// Returns the inner FileSystemDataNew object, if applicable.
    pub fn as_new(&self) -> Option<&FileSystemDataNew> {
        match self {
            FileSystemData::New(fs_data_new) => Some(fs_data_new),
            _ => None,
        }
    }

    /// Returns the inner FileSystemDataAdopted object, if applicable.
    pub fn as_adopted(&self) -> Option<&FileSystemDataAdopted> {
        match self {
            FileSystemData::Adopted(fs_data_adopted) => Some(fs_data_adopted),
            _ => None,
        }
    }

    /// Returns whether the filesystem is the root filesystem, as determined by
    /// its mount point path.
    pub fn is_root(&self) -> bool {
        self.mount_point_path()
            .is_some_and(|mpp| mpp == Path::new(ROOT_MOUNT_POINT_PATH))
    }

    /// Returns whether the filesystem is the EFI System Partition (ESP), as
    /// determined by its mount point path.
    pub fn is_esp(&self) -> bool {
        self.mount_point_path()
            .is_some_and(|mpp| mpp == Path::new(ESP_MOUNT_POINT_PATH))
    }

    /// Returns the path of the mount point, if it exists.
    pub fn mount_point_path(&self) -> Option<&Path> {
        match self {
            FileSystemData::Adopted(fs_data_adopted) => fs_data_adopted
                .mount_point
                .as_ref()
                .map(|mp| mp.path.as_ref()),
            FileSystemData::Image(fs_data_image) => Some(&fs_data_image.mount_point.path),
            FileSystemData::New(fs_data_new) => {
                fs_data_new.mount_point.as_ref().map(|mp| mp.path.as_ref())
            }
            FileSystemData::Swap(_) => None,
            FileSystemData::Tmpfs(fs_data_tmpfs) => Some(&fs_data_tmpfs.mount_point.path),
            FileSystemData::Overlay(fs_data_overlay) => Some(&fs_data_overlay.mount_point.path),
        }
    }

    /// Returns the filesystem type.
    pub fn fs_type(&self) -> FileSystemType {
        match self {
            FileSystemData::Adopted(fs_data_adopted) => fs_data_adopted.fs_type,
            FileSystemData::Image(fs_data_image) => fs_data_image.fs_type,
            FileSystemData::New(fs_data_new) => fs_data_new.fs_type,
            FileSystemData::Swap(_) => FileSystemType::Swap,
            FileSystemData::Tmpfs(_) => FileSystemType::Tmpfs,
            FileSystemData::Overlay(_) => FileSystemType::Overlay,
        }
    }

    /// Returns the device ID of the filesystem, if it exists.
    pub fn device_id(&self) -> Option<&BlockDeviceId> {
        match self {
            FileSystemData::Adopted(fs_data_adopted) => Some(&fs_data_adopted.device_id),
            FileSystemData::Image(fs_data_image) => Some(&fs_data_image.device_id),
            FileSystemData::New(fs_data_new) => Some(&fs_data_new.device_id),
            FileSystemData::Swap(fs_data_swap) => Some(&fs_data_swap.device_id),
            FileSystemData::Tmpfs(_) | FileSystemData::Overlay(_) => None,
        }
    }

    /// Returns whether the filesystem's mount options include the `ro` option.
    pub fn is_read_only(&self) -> bool {
        match self {
            FileSystemData::Adopted(fs_data_adopted) => fs_data_adopted
                .mount_point
                .as_ref()
                .is_some_and(|mp| mp.options.contains(MOUNT_OPTION_READ_ONLY)),
            FileSystemData::Image(fs_data_image) => fs_data_image
                .mount_point
                .options
                .contains(MOUNT_OPTION_READ_ONLY),
            FileSystemData::New(fs_data_new) => fs_data_new
                .mount_point
                .as_ref()
                .is_some_and(|mp| mp.options.contains(MOUNT_OPTION_READ_ONLY)),
            FileSystemData::Swap(_) => false,
            FileSystemData::Tmpfs(fs_data_tmpfs) => fs_data_tmpfs
                .mount_point
                .options
                .contains(MOUNT_OPTION_READ_ONLY),
            FileSystemData::Overlay(fs_data_overlay) => fs_data_overlay
                .mount_point
                .options
                .contains(MOUNT_OPTION_READ_ONLY),
        }
    }
}

impl FileSystemDataImage {
    pub fn mount_point_path(&self) -> &Path {
        &self.mount_point.path
    }

    pub fn is_read_only(&self) -> bool {
        self.mount_point.options.contains(MOUNT_OPTION_READ_ONLY)
    }

    pub fn is_esp(&self) -> bool {
        self.mount_point_path() == Path::new(ESP_MOUNT_POINT_PATH)
    }

    pub fn is_root(&self) -> bool {
        self.mount_point_path() == Path::new(ROOT_MOUNT_POINT_PATH)
    }
}

impl EngineContext {
    /// Populate the `filesystems` field in EngineContext from all sources.
    ///
    /// TODO: Currently this function just takes information from the HC since
    /// all filesystem information can be found there. However, once filesystem
    /// type is removed for image-sourced filesystems, this function should
    /// combine filesystems from OS image and HC.
    pub fn populate_filesystems(&mut self) -> Result<(), TridentError> {
        self.filesystems = self
            .spec
            .storage
            .filesystems
            .iter()
            .map(FileSystemData::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(())
    }

    /// Get an interator over all filesystems from all sources.
    pub fn filesystems(&self) -> impl Iterator<Item = FileSystemData> {
        self.filesystems.clone().into_iter()
    }

    /// Get the root filesystem.
    ///
    /// Note: root filesystem must be sourced from an Image.
    pub fn root_filesystem(&self) -> Option<FileSystemDataImage> {
        self.filesystems()
            .filter_map(|fs| fs.as_image().cloned())
            .find(|img_fs| img_fs.mount_point.path == PathBuf::from(ROOT_MOUNT_POINT_PATH))
    }

    /// Get the ESP filesystem.
    ///
    /// Note: ESP filesystem must be sourced from an Image.
    pub fn esp_filesystem(&self) -> Option<FileSystemDataImage> {
        self.filesystems()
            .filter_map(|fs| fs.as_image().cloned())
            .find(|img_fs| img_fs.mount_point.path == PathBuf::from(ESP_MOUNT_POINT_PATH))
    }
}

impl<'a> TryFrom<&'a FileSystem> for FileSystemData {
    type Error = TridentError;

    fn try_from(fs: &'a FileSystem) -> Result<Self, Self::Error> {
        match fs.source {
            FileSystemSource::Adopted => Ok(FileSystemData::Adopted(FileSystemDataAdopted {
                mount_point: fs.mount_point.clone(),
                fs_type: fs.fs_type,
                device_id: fs.device_id.clone().structured(InternalError::Internal(
                    "Expected device id for Adopted filesystem but found none",
                ))?,
            })),
            FileSystemSource::Image => Ok(FileSystemData::Image(FileSystemDataImage {
                mount_point: fs.mount_point.clone().structured(InternalError::Internal(
                    "Expected mount point for Image filesystem but found none",
                ))?,
                fs_type: fs.fs_type,
                device_id: fs.device_id.clone().structured(InternalError::Internal(
                    "Expected device id for Image filesystem but found none",
                ))?,
            })),
            FileSystemSource::New => match fs.fs_type {
                FileSystemType::Swap => Ok(FileSystemData::Swap(FileSystemDataSwap {
                    device_id: fs.device_id.clone().structured(InternalError::Internal(
                        "Expected device id for New filesystem but found none",
                    ))?,
                })),
                FileSystemType::Tmpfs => Ok(FileSystemData::Tmpfs(FileSystemDataTmpfs {
                    mount_point: fs.mount_point.clone().structured(InternalError::Internal(
                        "Expected mount point for Tmpfs filesystem but found none",
                    ))?,
                })),
                FileSystemType::Overlay => Ok(FileSystemData::Overlay(FileSystemDataOverlay {
                    mount_point: fs.mount_point.clone().structured(InternalError::Internal(
                        "Expected mount point for Overlay filesystem but found none",
                    ))?,
                })),
                _ => Ok(FileSystemData::New(FileSystemDataNew {
                    mount_point: fs.mount_point.clone(),
                    fs_type: fs.fs_type,
                    device_id: fs.device_id.clone().structured(InternalError::Internal(
                        "Expected device id for New filesystem but found none",
                    ))?,
                })),
            },
        }
    }
}
