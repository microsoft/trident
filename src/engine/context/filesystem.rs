use std::path::{Path, PathBuf};

use trident_api::{
    config::{FileSystemSource, FileSystemType, MountPoint},
    constants::{ESP_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    BlockDeviceId,
};

use crate::engine::EngineContext;

#[derive(Clone)]
pub struct FileSystemData<'a> {
    /// The mount point the file system.
    pub mount_point: Option<&'a MountPoint>,

    /// The file system type.
    pub fs_type: FileSystemType,

    /// The id of the block device associated with this filesystem.
    pub device_id: Option<&'a BlockDeviceId>,

    /// The file system source.
    pub source: FileSystemSource,
}

impl FileSystemData<'_> {
    /// Because filesystems don't have IDs that can uniquely identify them, this
    /// function can be used to create a description of the specific filesystem
    /// in lieu of an ID.
    pub fn description(&self) -> String {
        [
            (
                "src",
                Some(
                    match &self.source {
                        FileSystemSource::New => "new",
                        FileSystemSource::Adopted => "adopted",
                        FileSystemSource::Image => "image",
                    }
                    .to_owned(),
                ),
            ),
            ("type", Some(self.fs_type.to_string())),
            ("dev", self.device_id.cloned()),
            (
                "mnt",
                self.mount_point
                    .as_ref()
                    .map(|mp| mp.path.to_string_lossy().to_string()),
            ),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| format!("{}:{}", k, v)))
        .collect::<Vec<_>>()
        .join(", ")
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
        self.mount_point.as_ref().map(|mp| mp.path.as_ref())
    }

    /// Returns whether the filesystem's mount options include the `ro` option.
    pub fn is_read_only(&self) -> bool {
        self.mount_point
            .as_ref()
            .map_or(false, |mp| mp.options.contains(MOUNT_OPTION_READ_ONLY))
    }
}

impl EngineContext {
    /// Get an interator over all filesystems from all sources.
    ///
    /// TODO: Currently this function just takes information from the HC since
    /// all filesystem information can be found there. However, once filesystem
    /// type is removed for image-sourced filesystems, this function should
    /// combine filesystems from OS image and HC.
    pub fn filesystems(&self) -> impl Iterator<Item = FileSystemData> + '_ {
        self.spec
            .storage
            .filesystems
            .iter()
            .map(|fs| FileSystemData {
                mount_point: fs.mount_point.as_ref(),
                fs_type: fs.fs_type,
                source: fs.source,
                device_id: fs.device_id.as_ref(),
            })
    }

    /// Get the root filesystem.
    pub fn root_filesystem(&self) -> Option<FileSystemData> {
        self.filesystems().find(|fs| {
            fs.mount_point
                .as_ref()
                .is_some_and(|mp| *mp.path == PathBuf::from(ROOT_MOUNT_POINT_PATH))
        })
    }

    /// Get the ESP filesystem.
    pub fn esp_filesystem(&self) -> Option<FileSystemData> {
        self.filesystems().find(|fs| {
            fs.mount_point
                .as_ref()
                .is_some_and(|mp| *mp.path == PathBuf::from(ESP_MOUNT_POINT_PATH))
        })
    }
}
