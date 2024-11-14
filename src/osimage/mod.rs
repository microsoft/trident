use std::{io::Read, path::Path};

use anyhow::Error;
use osutils::{arch::SystemArchitecture, partition_types::DiscoverablePartitionType};
use trident_api::primitives::hash::Sha384Hash;
use url::Url;

mod cosi;

use cosi::{Cosi, CosiFileSystemImage};

/// Abstract representation of an OS image.
#[derive(Debug, Clone)]
pub struct OsImage(OsImageInner);

#[derive(Debug, Clone)]
enum OsImageInner {
    /// Composable OS Image (COSI)
    Cosi(Cosi),
}

impl OsImage {
    pub(crate) fn cosi(url: &Url) -> Result<Self, anyhow::Error> {
        Ok(Self(OsImageInner::Cosi(Cosi::new(url)?)))
    }

    /// Returns the name of the OS image type.
    pub(crate) fn name(&self) -> &'static str {
        match &self.0 {
            OsImageInner::Cosi(_) => "COSI",
        }
    }

    /// Returns the source URL of the OS image.
    pub(crate) fn source(&self) -> &Url {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.source(),
        }
    }

    /// Returns an iterator over the available mount points provided by the OS image. It does not
    /// include the ESP filesystem mount point.
    pub(crate) fn available_mount_points(&self) -> impl Iterator<Item = &Path> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.filesystems().map(|fs| fs.image.mount_point.as_path()),
        }
    }

    /// Returns the OS architecture of the image.
    pub(crate) fn architecture(&self) -> SystemArchitecture {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.architecture(),
        }
    }

    /// Returns the ESP filesystem image.
    pub(crate) fn esp_filesystem(&self) -> Result<FileSystemImage, Error> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.esp_filesystem().map(FileSystemImage::cosi),
        }
    }

    /// Returns an iterator over all images that are NOT the ESP filesystem image.
    pub(crate) fn filesystems(&self) -> impl Iterator<Item = FileSystemImage> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.filesystems().map(FileSystemImage::cosi),
        }
    }
}

/// Abstract representation of a filesystem image.
pub struct FileSystemImage<'a>(FileSystemImageType<'a>);

enum FileSystemImageType<'a> {
    Cosi(CosiFileSystemImage<'a>),
}

impl<'a> FileSystemImage<'a> {
    /// Creates a new filesystem image from a COSI filesystem image.
    fn cosi(fs: CosiFileSystemImage<'a>) -> Self {
        Self(FileSystemImageType::Cosi(fs))
    }

    /// Returns the path where this filesystem image should be mounted.
    pub fn mount_point(&self) -> &Path {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => &fs.image.mount_point,
        }
    }

    /// Returns the filesystem type of this filesystem image.
    pub fn fs_type(&self) -> &str {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => &fs.image.fs_type,
        }
    }

    /// Returns the partition type of this filesystem image.
    pub fn part_type(&self) -> DiscoverablePartitionType {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.image.part_type,
        }
    }

    /// Returns a reader for the filesystem image.
    pub fn reader(&self) -> Result<Box<dyn Read>, Error> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.reader(),
        }
    }

    /// Returns the size of the raw filesystem image.
    pub fn size(&self) -> u64 {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.image.file.uncompressed_size,
        }
    }

    /// Returns the SHA384 checksum of the compressed filesystem image.
    pub fn sha384(&self) -> &Sha384Hash {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => &fs.image.file.sha384,
        }
    }

    /// Returns the verity roothash of this filesystem, if available.
    pub fn verity_roothash(&self) -> Option<&str> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.image.verity.as_ref().map(|v| v.roothash.as_str()),
        }
    }

    /// Returns a reader to the verity hash image of this filesystem, if available.
    pub fn verity_reader(&self) -> Option<Result<Box<dyn Read>, Error>> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.reader_verity(),
        }
    }

    /// Returns the size of the verity hash image of this filesystem, if available.
    pub fn verity_size(&self) -> Option<u64> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => {
                fs.image.verity.as_ref().map(|v| v.file.uncompressed_size)
            }
        }
    }

    /// Returns the SHA384 checksum of verity hash image of this filesystem image.
    pub fn verity_sha384(&self) -> Option<&Sha384Hash> {
        match &self.0 {
            FileSystemImageType::Cosi(fs) => fs.image.verity.as_ref().map(|v| &v.file.sha384),
        }
    }
}
