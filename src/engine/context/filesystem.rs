use std::path::PathBuf;

use trident_api::{
    config::{FileSystemType, MountPoint},
    constants::{ESP_MOUNT_POINT_PATH, ROOT_MOUNT_POINT_PATH},
    BlockDeviceId,
};

use crate::engine::EngineContext;

pub struct FileSystemData {
    /// The mount point the file system.
    pub mount_point: Option<MountPoint>,

    /// The file system type.
    pub fs_type: FileSystemType,

    /// The id of the block device associated with this filesystem.
    pub device_id: Option<BlockDeviceId>,
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
                mount_point: fs.mount_point.clone(),
                fs_type: fs.fs_type,
                device_id: fs.device_id.clone(),
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
