use std::path::{Path, PathBuf};

use trident_api::{
    config::{FileSystem, FileSystemSource, FileSystemType, MountPoint},
    constants::{ESP_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    error::{InternalError, ReportError, TridentError},
    BlockDeviceId,
};

use crate::engine::EngineContext;

#[derive(Clone)]
pub enum FileSystemData<'a> {
    Image(FileSystemDataImage<'a>),
    New(FileSystemDataNew<'a>),
    Adopted(FileSystemDataAdopted<'a>),
    Swap(FileSystemDataSwap<'a>),
    Tmpfs(FileSystemDataTmpfs<'a>),
    Overlay(FileSystemDataOverlay<'a>),
}

#[derive(Clone)]
pub struct FileSystemDataImage<'a> {
    /// The mount point of the file system.
    ///
    /// Note: mount_point is required for Image filesystems.
    pub mount_point: &'a MountPoint,

    /// The file system type.
    pub fs_type: FileSystemType,

    /// The id of the block device associated with this filesystem.
    ///
    /// Note: device_id is required for Image filesystems.
    pub device_id: &'a BlockDeviceId,
}

#[derive(Clone)]
pub struct FileSystemDataNew<'a> {
    /// The mount point of the file system.
    pub mount_point: Option<&'a MountPoint>,

    /// The file system type.
    pub fs_type: FileSystemType,

    /// The id of the block device associated with this filesystem.
    ///
    /// Note: since Tmpfs, Overlay, and Swap filesystems have a separate
    /// FileSystemData* type, all remaining New filesystems are expected to have
    /// a device ID
    pub device_id: &'a BlockDeviceId,
}

#[derive(Clone)]
pub struct FileSystemDataAdopted<'a> {
    /// The mount point of the file system.
    pub mount_point: Option<&'a MountPoint>,

    /// The file system type.
    pub fs_type: FileSystemType,

    /// The id of the block device associated with this filesystem.
    ///
    /// Note: device_id is required for Adopted filesystems.
    pub device_id: &'a BlockDeviceId,
}

/// FileSystemData struct for Tmpfs filesystems.
///
/// Device id is not expected for a Tmpfs filesystem.
#[derive(Clone)]
pub struct FileSystemDataTmpfs<'a> {
    /// The mount point of the file system.
    pub mount_point: &'a MountPoint,
}

/// FileSystemData struct for Overlay filesystems.
///
/// Device id is not expected for a Overlay filesystem.
#[derive(Clone)]
pub struct FileSystemDataOverlay<'a> {
    /// The mount point of the file system.
    pub mount_point: &'a MountPoint,
}

/// FileSystemData struct for Swap filesystems.
///
/// Swap filesystems cannot have a mount point.
#[derive(Clone)]
pub struct FileSystemDataSwap<'a> {
    /// The id of the block device associated with this filesystem.
    ///
    /// Note: device_id is required for Swap filesystems.
    pub device_id: &'a BlockDeviceId,
}

impl<'a> FileSystemData<'a> {
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
    pub fn inner_image(self) -> Option<FileSystemDataImage<'a>> {
        match self {
            FileSystemData::Image(fs_data_img) => Some(fs_data_img),
            _ => None,
        }
    }

    /// Returns the inner FileSystemDataNew object, if applicable.
    pub fn inner_new(self) -> Option<FileSystemDataNew<'a>> {
        match self {
            FileSystemData::New(fs_data_new) => Some(fs_data_new),
            _ => None,
        }
    }

    /// Returns the inner FileSystemDataAdopted object, if applicable.
    pub fn inner_adopted(self) -> Option<FileSystemDataAdopted<'a>> {
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
            FileSystemData::Adopted(fs_data_adopted) => Some(fs_data_adopted.device_id),
            FileSystemData::Image(fs_data_image) => Some(fs_data_image.device_id),
            FileSystemData::New(fs_data_new) => Some(fs_data_new.device_id),
            FileSystemData::Swap(fs_data_swap) => Some(fs_data_swap.device_id),
            FileSystemData::Tmpfs(_) | FileSystemData::Overlay(_) => None,
        }
    }

    /// Returns whether the filesystem's mount options include the `ro` option.
    pub fn is_read_only(&self) -> bool {
        match self {
            FileSystemData::Adopted(fs_data_adopted) => fs_data_adopted
                .mount_point
                .as_ref()
                .map_or(false, |mp| mp.options.contains(MOUNT_OPTION_READ_ONLY)),
            FileSystemData::Image(fs_data_image) => fs_data_image
                .mount_point
                .options
                .contains(MOUNT_OPTION_READ_ONLY),
            FileSystemData::New(fs_data_new) => fs_data_new
                .mount_point
                .as_ref()
                .map_or(false, |mp| mp.options.contains(MOUNT_OPTION_READ_ONLY)),
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

impl FileSystemDataImage<'_> {
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
    /// Get an interator over all filesystems from all sources.
    ///
    /// TODO: Currently this function just takes information from the HC since
    /// all filesystem information can be found there. However, once filesystem
    /// type is removed for image-sourced filesystems, this function should
    /// combine filesystems from OS image and HC.
    pub fn filesystems(&self) -> Result<impl Iterator<Item = FileSystemData> + '_, TridentError> {
        let fs = self
            .spec
            .storage
            .filesystems
            .iter()
            .map(FileSystemData::try_from)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(fs.into_iter())
    }

    /// Get the root filesystem.
    ///
    /// Note: root filesystem must be sourced from an Image.
    pub fn root_filesystem(&self) -> Option<FileSystemDataImage> {
        self.filesystems()
            .ok()?
            .filter_map(FileSystemData::inner_image)
            .find(|img_fs| img_fs.mount_point.path == PathBuf::from(ROOT_MOUNT_POINT_PATH))
    }

    /// Get the ESP filesystem.
    ///
    /// Note: ESP filesystem must be sourced from an Image.
    pub fn esp_filesystem(&self) -> Option<FileSystemDataImage> {
        self.filesystems()
            .ok()?
            .filter_map(FileSystemData::inner_image)
            .find(|img_fs| img_fs.mount_point.path == PathBuf::from(ESP_MOUNT_POINT_PATH))
    }
}

impl<'a> TryFrom<&'a FileSystem> for FileSystemData<'a> {
    type Error = TridentError;

    fn try_from(fs: &'a FileSystem) -> Result<Self, Self::Error> {
        match fs.source {
            FileSystemSource::Adopted => Ok(FileSystemData::Adopted(FileSystemDataAdopted {
                mount_point: fs.mount_point.as_ref(),
                fs_type: fs.fs_type,
                device_id: fs.device_id.as_ref().structured(InternalError::Internal(
                    "Expected device id for Adopted filesystem but found none",
                ))?,
            })),
            FileSystemSource::Image => Ok(FileSystemData::Image(FileSystemDataImage {
                mount_point: fs.mount_point.as_ref().structured(InternalError::Internal(
                    "Expected mount point for Image filesystem but found none",
                ))?,
                fs_type: fs.fs_type,
                device_id: fs.device_id.as_ref().structured(InternalError::Internal(
                    "Expected device id for Image filesystem but found none",
                ))?,
            })),
            FileSystemSource::New => match fs.fs_type {
                FileSystemType::Swap => Ok(FileSystemData::Swap(FileSystemDataSwap {
                    device_id: fs.device_id.as_ref().structured(InternalError::Internal(
                        "Expected device id for New filesystem but found none",
                    ))?,
                })),
                FileSystemType::Tmpfs => Ok(FileSystemData::Tmpfs(FileSystemDataTmpfs {
                    mount_point: fs.mount_point.as_ref().structured(InternalError::Internal(
                        "Expected mount point for Tmpfs filesystem but found none",
                    ))?,
                })),
                FileSystemType::Overlay => Ok(FileSystemData::Overlay(FileSystemDataOverlay {
                    mount_point: fs.mount_point.as_ref().structured(InternalError::Internal(
                        "Expected mount point for Overlay filesystem but found none",
                    ))?,
                })),
                _ => Ok(FileSystemData::New(FileSystemDataNew {
                    mount_point: fs.mount_point.as_ref(),
                    fs_type: fs.fs_type,
                    device_id: fs.device_id.as_ref().structured(InternalError::Internal(
                        "Expected device id for New filesystem but found none",
                    ))?,
                })),
            },
        }
    }
}
