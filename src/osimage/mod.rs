use std::{
    fmt::{Display, Formatter},
    io::{Error as IoError, Read},
    path::{Path, PathBuf},
};

use anyhow::Error;
use serde::{Deserialize, Serialize};
use url::Url;

use osutils::{arch::SystemArchitecture, partition_types::DiscoverablePartitionType};
use trident_api::primitives::hash::Sha384Hash;

mod cosi;
#[cfg(test)]
pub(crate) mod mock;

use cosi::Cosi;
#[cfg(test)]
use mock::MockOsImage;

/// Abstract representation of an OS image.
#[derive(Debug, Clone)]
pub struct OsImage(OsImageInner);

#[derive(Debug, Clone)]
enum OsImageInner {
    /// Composable OS Image (COSI)
    Cosi(Cosi),

    /// Mock implementation for testing purposes
    #[cfg(test)]
    Mock(Box<MockOsImage>),
}

impl OsImage {
    pub(crate) fn cosi(url: &Url) -> Result<Self, anyhow::Error> {
        Ok(Self(OsImageInner::Cosi(Cosi::new(url)?)))
    }

    #[cfg(test)]
    pub(crate) fn mock(mock_os_image: MockOsImage) -> Self {
        Self(OsImageInner::Mock(Box::new(mock_os_image)))
    }

    /// Returns the name of the OS image type.
    pub(crate) fn name(&self) -> &'static str {
        match &self.0 {
            OsImageInner::Cosi(_) => "COSI",
            #[cfg(test)]
            OsImageInner::Mock(_) => "Mock",
        }
    }

    /// Returns the source URL of the OS image.
    pub(crate) fn source(&self) -> &Url {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.source(),
            #[cfg(test)]
            OsImageInner::Mock(mock) => &mock.source,
        }
    }

    /// Returns an iterator over the available mount points provided by the OS image. It does not
    /// include the ESP filesystem mount point.
    pub(crate) fn available_mount_points<'a>(&'a self) -> Box<dyn Iterator<Item = &'a Path> + 'a> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => Box::new(cosi.available_mount_points()),
            #[cfg(test)]
            OsImageInner::Mock(mock) => Box::new(mock.available_mount_points()),
        }
    }

    /// Returns the OS architecture of the image.
    pub(crate) fn architecture(&self) -> SystemArchitecture {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.architecture(),
            #[cfg(test)]
            OsImageInner::Mock(mock) => mock.architecture(),
        }
    }

    /// Returns the ESP filesystem image.
    pub(crate) fn esp_filesystem(&self) -> Result<OsImageFileSystem, Error> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.esp_filesystem(),
            #[cfg(test)]
            OsImageInner::Mock(mock) => mock.esp_filesystem(),
        }
    }

    /// Returns an iterator over all images that are NOT the ESP filesystem image.
    pub(crate) fn filesystems<'a>(&'a self) -> Box<dyn Iterator<Item = OsImageFileSystem> + 'a> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => Box::new(cosi.filesystems()),
            #[cfg(test)]
            OsImageInner::Mock(mock) => Box::new(mock.filesystems()),
        }
    }
}

pub struct OsImageFileSystem<'a> {
    pub mount_point: PathBuf,
    pub fs_type: OsImageFileSystemType,
    pub part_type: DiscoverablePartitionType,
    pub image_file: OsImageFile<'a>,
    pub image_file_verity: Option<OsImageVerityHash<'a>>,
}

pub struct OsImageFile<'a> {
    pub compressed_size: u64,
    pub sha384: Sha384Hash,
    pub uncompressed_size: u64,
    reader: Box<dyn Fn() -> Result<Box<dyn Read>, IoError> + 'a>,
}

impl OsImageFile<'_> {
    /// Returns a reader for the image file.
    pub fn reader(&self) -> Result<Box<dyn Read>, IoError> {
        (self.reader)()
    }
}

pub struct OsImageVerityHash<'a> {
    pub roothash: String,
    pub hash_image_file: OsImageFile<'a>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsImageFileSystemType {
    /// # Ext4 file system
    Ext4,

    /// # Ext3 file system
    Ext3,

    /// # Ext2 file system
    Ext2,

    /// # Cramfs file system
    Cramfs,

    /// # SquashFS file system
    Squashfs,

    /// # VFAT file system
    Vfat,

    /// # MS-DOS file system
    Msdos,

    /// # exFAT file system
    Exfat,

    /// # ISO9660 file system
    Iso9660,

    /// # NTFS file system
    Ntfs,

    /// # BTRFS file system
    Btrfs,

    /// # XFS file system
    Xfs,
}

impl Display for OsImageFileSystemType {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            serde_yaml::to_string(self)
                .map_err(|_| std::fmt::Error)?
                .trim()
        )
    }
}
