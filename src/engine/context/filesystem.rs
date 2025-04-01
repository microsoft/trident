use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use derive_more::From;

use sysdefs::filesystems::{KernelFilesystemType, NodevFilesystemType, RealFilesystemType};
use trident_api::{
    config::{FileSystem, FileSystemSource, FileSystemType, MountPoint},
    constants::{ESP_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    error::{InternalError, ReportError, TridentError, TridentResultExt},
    BlockDeviceId,
};

use crate::{engine::EngineContext, osimage::OsImageFileSystemType};

#[derive(Clone, From)]
pub enum FileSystemData {
    Image(FileSystemDataImage),
    New(FileSystemDataNew),
    Adopted(FileSystemDataAdopted),
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
    pub fs_type: RealFilesystemType,

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
    pub fs_type: RealFilesystemType,

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
    pub fs_type: Option<RealFilesystemType>,

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
                        FileSystemData::Overlay(_) => "new",
                    }
                    .to_owned(),
                ),
            ),
            (
                "type",
                self.fs_type().map(|fs_type| fs_type.name().to_string()),
            ),
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

    /// Returns the inner FileSystemDataTmpfs object, if applicable
    pub fn as_tmpfs(&self) -> Option<&FileSystemDataTmpfs> {
        match self {
            FileSystemData::Tmpfs(fs_data_tmpfs) => Some(fs_data_tmpfs),
            _ => None,
        }
    }

    /// Returns the inner FileSystemDataOverlay object, if applicable
    pub fn as_overlay(&self) -> Option<&FileSystemDataOverlay> {
        match self {
            FileSystemData::Overlay(fs_data_overlay) => Some(fs_data_overlay),
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
            FileSystemData::Tmpfs(fs_data_tmpfs) => Some(&fs_data_tmpfs.mount_point.path),
            FileSystemData::Overlay(fs_data_overlay) => Some(&fs_data_overlay.mount_point.path),
        }
    }

    /// Returns the filesystem type if one exists. This function will return
    /// None in the case that the filesystem type is set to "Auto".
    pub fn fs_type(&self) -> Option<KernelFilesystemType> {
        match self {
            FileSystemData::Adopted(fs_data_adopted) => {
                fs_data_adopted.fs_type.map(|fs_type| fs_type.as_kernel())
            }
            FileSystemData::Image(fs_data_image) => Some(fs_data_image.fs_type.as_kernel()),
            FileSystemData::New(fs_data_new) => Some(fs_data_new.fs_type.as_kernel()),
            FileSystemData::Tmpfs(_) => Some(NodevFilesystemType::Tmpfs.as_kernel()),
            FileSystemData::Overlay(_) => Some(NodevFilesystemType::Overlay.as_kernel()),
        }
    }

    /// Returns the device ID of the filesystem, if it exists.
    pub fn device_id(&self) -> Option<&BlockDeviceId> {
        match self {
            FileSystemData::Adopted(fs_data_adopted) => Some(&fs_data_adopted.device_id),
            FileSystemData::Image(fs_data_image) => Some(&fs_data_image.device_id),
            FileSystemData::New(fs_data_new) => Some(&fs_data_new.device_id),
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
    pub fn populate_filesystems(&mut self) -> Result<(), TridentError> {
        // Get all New and Adopted filesystems
        self.filesystems = self
            .spec
            .storage
            .filesystems
            .iter()
            .filter(|fs| fs.source != FileSystemSource::Image)
            .map(FileSystemData::try_from)
            .collect::<Result<Vec<_>, _>>()
            .message("Failed to create FileSystemData objects from New and Adopted filesystems in the Host Configuration")?;

        let Some(image) = &self.image else {
            // If there are no Image filesystems, we can return here
            return Ok(());
        };

        // Create a map to get the filesystem type from the Image
        let image_fs_map = image
            .filesystems()
            .chain(image.esp_filesystem())
            .map(|fs| (fs.mount_point, fs.fs_type))
            .collect::<HashMap<PathBuf, OsImageFileSystemType>>();

        // Get all Image filesystems in the Host Configuration
        for img_fs in self
            .spec
            .storage
            .filesystems
            .iter()
            .filter(|fs| fs.source == FileSystemSource::Image)
        {
            let mount_point =
                img_fs
                    .mount_point
                    .clone()
                    .structured(InternalError::PopulateFilesystems(
                        "Expected mount point for Image filesystem but found none".to_string(),
                    ))?;
            let fs_type = (*image_fs_map.get(&mount_point.path).structured(
                InternalError::PopulateFilesystems(format!(
                    "Failed to find filesystem type for Image filesystem mounted at {}",
                    mount_point.path.display()
                )),
            )?)
            .into();
            let device_id =
                img_fs
                    .device_id
                    .clone()
                    .structured(InternalError::PopulateFilesystems(format!(
                        "Expected device id for Image filesystem mounted at {} but found none",
                        mount_point.path.display()
                    )))?;

            // Add the filesystem to EngineContext's filesystems
            self.filesystems.push(
                FileSystemDataImage {
                    mount_point,
                    fs_type,
                    device_id,
                }
                .into(),
            )
        }

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
        let mpp = fs
            .mount_point_path()
            .map(|path| format!(" mounted at {}", path.display()))
            .unwrap_or("".into());

        match fs.source {
            FileSystemSource::Adopted => Ok(FileSystemData::Adopted(FileSystemDataAdopted {
                mount_point: fs.mount_point.clone(),
                fs_type: fs.fs_type.try_into().ok(),
                device_id: fs
                    .device_id
                    .clone()
                    .structured(InternalError::PopulateFilesystems(format!(
                        "Expected device id for Adopted filesystem{} but found none",
                        mpp
                    )))?,
            })),
            FileSystemSource::New => match fs.fs_type {
                FileSystemType::Tmpfs => Ok(FileSystemData::Tmpfs(FileSystemDataTmpfs {
                    mount_point: fs.mount_point.clone().structured(
                        InternalError::PopulateFilesystems(
                            "Expected mount point for Tmpfs filesystem but found none".to_string(),
                        ),
                    )?,
                })),
                FileSystemType::Overlay => Ok(FileSystemData::Overlay(FileSystemDataOverlay {
                    mount_point: fs.mount_point.clone().structured(
                        InternalError::PopulateFilesystems(
                            "Expected mount point for Overlay filesystem but found none"
                                .to_string(),
                        ),
                    )?,
                })),
                _ => Ok(FileSystemData::New(FileSystemDataNew {
                    mount_point: fs.mount_point.clone(),
                    fs_type: fs.fs_type.try_into()?,
                    device_id: fs.device_id.clone().structured(
                        InternalError::PopulateFilesystems(format!(
                            "Expected device id for New filesystem{} but found none",
                            mpp
                        )),
                    )?,
                })),
            },
            FileSystemSource::Image => Err(TridentError::new(InternalError::PopulateFilesystems(
                "Image filesystems should be handled separately".to_string(),
            ))),
        }
    }
}
