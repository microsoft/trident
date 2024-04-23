use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use trident_api::constants::ROOT_MOUNT_POINT_PATH;

use crate::filesystems::TabFileSystemType;

/// A representation of a fstab file.
#[derive(Debug, Default)]
pub struct TabFile {
    pub entries: Vec<TabFileEntry>,
}

/// A representation of a single entry in a tab file.
#[derive(Debug, PartialEq, Eq)]
pub struct TabFileEntry {
    pub device: TabDevice,
    pub mount_point: TabMountPoint,
    pub fs_type: TabFileSystemType,
    pub options: Vec<String>,
}

/// A representation of a device in a tab file.
#[derive(Debug, PartialEq, Eq)]
pub enum TabDevice {
    None,
    Overlay,
    Tmpfs,
    BlockDevice(PathBuf),
}

/// A representation of a mount point in a tab file.
#[derive(Debug, PartialEq, Eq)]
pub enum TabMountPoint {
    None,
    Path(PathBuf),
}

impl TabFile {
    /// Write this tab file to disk at location `tab_file_path`.
    pub fn write(&self, tab_file_path: impl AsRef<Path>) -> Result<(), Error> {
        std::fs::write(tab_file_path.as_ref(), self.render().as_bytes())
            .with_context(|| format!("Failed to write new {}", tab_file_path.as_ref().display()))
    }

    /// Render this tab file as a string.
    pub fn render(&self) -> String {
        self.entries.iter().map(|entry| entry.render()).collect()
    }
}

impl TabFileEntry {
    /// Create a new regular entry for a block device mounted at a path.
    pub fn new_path(
        device: impl Into<PathBuf>,
        mount_point: impl Into<PathBuf>,
        fs_type: TabFileSystemType,
    ) -> Self {
        Self {
            device: TabDevice::BlockDevice(device.into()),
            mount_point: TabMountPoint::Path(mount_point.into()),
            fs_type,
            options: Vec::new(),
        }
    }

    /// Create a new entry for a block device mounted as swap.
    pub fn new_swap(device: impl Into<PathBuf>) -> Self {
        Self {
            device: TabDevice::BlockDevice(device.into()),
            mount_point: TabMountPoint::None,
            fs_type: TabFileSystemType::Swap,
            options: Vec::new(),
        }
    }

    /// Create a new entry for a tmpfs mount.
    pub fn new_tmpfs(mount_point: impl Into<PathBuf>) -> Self {
        Self {
            device: TabDevice::Tmpfs,
            mount_point: TabMountPoint::Path(mount_point.into()),
            fs_type: TabFileSystemType::Tmpfs,
            options: Vec::new(),
        }
    }

    /// Create a new entry for an overlay mount.
    pub fn new_overlay(mount_point: impl Into<PathBuf>) -> Self {
        Self {
            device: TabDevice::Overlay,
            mount_point: TabMountPoint::Path(mount_point.into()),
            fs_type: TabFileSystemType::Overlay,
            options: Vec::new(),
        }
    }

    /// Add options to this entry.
    pub fn with_options(mut self, options: Vec<String>) -> Self {
        self.options = options;
        self
    }

    /// Render this entry as a string suitable for writing to a tab file.
    pub fn render(&self) -> String {
        // fsck pass is 1 for root, 2 for everything else, 0 for none
        let fsck_pass = match self.mount_point {
            TabMountPoint::None => 0,
            TabMountPoint::Path(ref path) if path == Path::new(ROOT_MOUNT_POINT_PATH) => 1,
            _ => 2,
        };

        // If the options are empty, use "defaults" as the default
        let options = if self.options.is_empty() {
            "defaults".into()
        } else {
            self.options.join(",")
        };

        format!(
            "{} {} {} {} 0 {}\n",
            self.device.render(),
            self.mount_point.render(),
            self.fs_type.name(),
            options,
            fsck_pass,
        )
    }
}

impl TabDevice {
    /// Render this device as a string.
    pub fn render(&self) -> String {
        match self {
            TabDevice::None => "none".to_string(),
            TabDevice::Overlay => "overlay".to_string(),
            TabDevice::Tmpfs => "tmpfs".to_string(),
            TabDevice::BlockDevice(path) => path.to_string_lossy().to_string(),
        }
    }
}

impl TabMountPoint {
    /// Render this mount point as a string.
    pub fn render(&self) -> String {
        match self {
            TabMountPoint::None => "none".to_string(),
            TabMountPoint::Path(path) => path.to_string_lossy().to_string(),
        }
    }
}
