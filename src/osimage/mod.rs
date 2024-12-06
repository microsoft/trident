use std::{
    fmt::{Display, Formatter},
    io::Read,
    path::Path,
};

use anyhow::Error;
use serde::{Deserialize, Serialize};
use url::Url;

use osutils::{arch::SystemArchitecture, partition_types::DiscoverablePartitionType};
use trident_api::primitives::hash::Sha384Hash;

mod cosi;
#[cfg(test)]
pub(crate) mod mock;

use cosi::{Cosi, CosiFileSystemImage};
#[cfg(test)]
use mock::{MockFileSystemImage, MockOsImage};

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
            OsImageInner::Cosi(cosi) => {
                Box::new(cosi.filesystems().map(|fs| fs.image.mount_point.as_path()))
            }
            #[cfg(test)]
            OsImageInner::Mock(mock) => {
                Box::new(mock.filesystems().map(|fs| fs.image.mount_point.as_path()))
            }
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
    pub(crate) fn esp_filesystem(&self) -> Result<FileSystemImage, Error> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.esp_filesystem().map(FileSystemImage::cosi),
            #[cfg(test)]
            OsImageInner::Mock(mock) => mock.esp_filesystem().map(FileSystemImage::mock),
        }
    }

    /// Returns an iterator over all images that are NOT the ESP filesystem image.
    pub(crate) fn filesystems<'a>(&'a self) -> Box<dyn Iterator<Item = FileSystemImage> + 'a> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => Box::new(cosi.filesystems().map(FileSystemImage::cosi)),
            #[cfg(test)]
            OsImageInner::Mock(fs) => Box::new(fs.filesystems().map(FileSystemImage::mock)),
        }
    }
}

/// Abstract representation of a filesystem image.
pub struct FileSystemImage<'a>(FileSystemImageType<'a>);

enum FileSystemImageType<'a> {
    Cosi(CosiFileSystemImage<'a>),
    #[cfg(test)]
    Mock(MockFileSystemImage<'a>),
}

impl<'a> FileSystemImage<'a> {
    /// Creates a new filesystem image from a COSI filesystem image.
    fn cosi(fs: CosiFileSystemImage<'a>) -> Self {
        Self(FileSystemImageType::Cosi(fs))
    }

    #[cfg(test)]
    fn mock(fs: MockFileSystemImage<'a>) -> Self {
        Self(FileSystemImageType::Mock(fs))
    }

    /// Returns the path where this filesystem image should be mounted.
    pub fn mount_point(&self) -> &Path {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => &fs.image.mount_point,
            #[cfg(test)]
            FileSystemImageType::Mock(fs) => &fs.image.mount_point,
        }
    }

    /// Returns the filesystem type of this filesystem image.
    pub fn fs_type(&self) -> OsImageFileSystemType {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.image.fs_type,
            #[cfg(test)]
            FileSystemImageType::Mock(fs) => fs.image.fs_type,
        }
    }

    /// Returns the partition type of this filesystem image.
    pub fn part_type(&self) -> DiscoverablePartitionType {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.image.part_type,
            #[cfg(test)]
            FileSystemImageType::Mock(fs) => fs.image.part_type,
        }
    }

    /// Returns a reader for the filesystem image.
    pub fn reader(&self) -> Result<Box<dyn Read>, Error> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.reader(),
            #[cfg(test)]
            FileSystemImageType::Mock(_) => {
                unimplemented!("Mock filesystem image does not implement reader() method")
            }
        }
    }

    /// Returns the size of the raw filesystem image.
    pub fn size(&self) -> u64 {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.image.file.uncompressed_size,
            #[cfg(test)]
            FileSystemImageType::Mock(_) => {
                unimplemented!("Mock filesystem image does not implement size() method")
            }
        }
    }

    /// Returns the SHA384 checksum of the compressed filesystem image.
    pub fn sha384(&self) -> &Sha384Hash {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => &fs.image.file.sha384,
            #[cfg(test)]
            FileSystemImageType::Mock(_) => {
                unimplemented!("Mock filesystem image does not implement sha384() method")
            }
        }
    }

    /// Returns the verity roothash of this filesystem, if available.
    pub fn verity_roothash(&self) -> Option<&str> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.image.verity.as_ref().map(|v| v.roothash.as_str()),
            #[cfg(test)]
            FileSystemImageType::Mock(_) => {
                unimplemented!("Mock filesystem image does not implement verity_roothash() method")
            }
        }
    }

    /// Returns a reader to the verity hash image of this filesystem, if available.
    pub fn verity_reader(&self) -> Option<Result<Box<dyn Read>, Error>> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.reader_verity(),
            #[cfg(test)]
            FileSystemImageType::Mock(_) => {
                unimplemented!("Mock filesystem image does not implement verity_reader() method")
            }
        }
    }

    /// Returns the size of the verity hash image of this filesystem, if available.
    pub fn verity_size(&self) -> Option<u64> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => {
                fs.image.verity.as_ref().map(|v| v.file.uncompressed_size)
            }
            #[cfg(test)]
            FileSystemImageType::Mock(_) => {
                unimplemented!("Mock filesystem image does not implement verity_size() method")
            }
        }
    }

    /// Returns the SHA384 checksum of verity hash image of this filesystem image.
    pub fn verity_sha384(&self) -> Option<&Sha384Hash> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.image.verity.as_ref().map(|v| &v.file.sha384),
            #[cfg(test)]
            FileSystemImageType::Mock(_) => {
                unimplemented!("Mock filesystem image does not implement verity_sha384() method")
            }
        }
    }
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
