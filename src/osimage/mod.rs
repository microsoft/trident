use std::{
    fmt::{Display, Formatter},
    io::{Error as IoError, Read},
    path::{Path, PathBuf},
};

use anyhow::Error;
use log::{debug, info};
use serde::{Deserialize, Serialize};
use url::Url;

use sysdefs::{
    arch::SystemArchitecture, filesystems::RealFilesystemType, osuuid::OsUuid,
    partition_types::DiscoverablePartitionType,
};
use trident_api::{
    config::{self, ImageSha384},
    constants::ROOT_MOUNT_POINT_PATH,
    error::{InvalidInputError, ReportError, TridentError},
    primitives::hash::Sha384Hash,
};

mod cosi;

/// Os Image mocking module. This module contains the mock implementation of an
/// OS image for testing purposes. It should not be tied to the specifics of any
/// one OS image implementation.
///
/// allow(dead_code) is used to suppress warnings about unused code in this
/// module because different tests may use different capabilities.
#[cfg(any(test, feature = "functional-test"))]
#[allow(dead_code)]
pub(crate) mod mock;

use cosi::Cosi;
#[cfg(test)]
use mock::MockOsImage;

/// Abstract representation of an OS image.
#[derive(Debug, Clone)]
pub struct OsImage(OsImageInner);

#[cfg_attr(test, allow(clippy::large_enum_variant))]
#[derive(Debug, Clone)]
enum OsImageInner {
    /// Composable OS Image (COSI)
    Cosi(Cosi),

    /// Mock implementation for testing purposes
    #[cfg(test)]
    Mock(Box<MockOsImage>),
}

impl OsImage {
    pub(crate) fn cosi(source: &config::OsImage) -> Result<Self, Error> {
        Ok(Self(OsImageInner::Cosi(Cosi::new(source)?)))
    }

    #[cfg(test)]
    pub(crate) fn mock(mock_os_image: MockOsImage) -> Self {
        Self(OsImageInner::Mock(Box::new(mock_os_image)))
    }

    /// Load the OS given the image source from the Host Configuration and either validate or
    /// populate the associated metadata sha384 checksum.
    pub(crate) fn load(image_source: &mut Option<config::OsImage>) -> Result<Self, TridentError> {
        let Some(ref mut image_source) = image_source else {
            return Err(TridentError::new(InvalidInputError::MissingOsImage));
        };

        debug!("Attempting to load COSI file from '{}'", image_source.url);
        let os_image = OsImage::cosi(image_source).structured(InvalidInputError::LoadCosi {
            url: image_source.url.clone(),
        })?;
        if image_source.sha384 == ImageSha384::Ignored {
            image_source.sha384 = ImageSha384::Checksum(os_image.metadata_sha384());
        }

        info!(
            "Loaded COSI file from '{}' with hash '{}'",
            os_image.source(),
            os_image.metadata_sha384()
        );

        // Ensure the OS image architecture matches the current system architecture
        if SystemArchitecture::current() != os_image.architecture() {
            return Err(TridentError::new(
                InvalidInputError::MismatchedArchitecture {
                    expected: SystemArchitecture::current().into(),
                    actual: os_image.architecture().into(),
                },
            ));
        }

        debug!(
            "OS image provides the following mount points: {}",
            os_image
                .available_mount_points()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );

        Ok(os_image)
    }

    pub(crate) fn is_uki(&self) -> bool {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.is_uki(),
            #[cfg(test)]
            OsImageInner::Mock(mock) => mock.is_uki,
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

    /// Find the mount point which contains the given path.
    pub(crate) fn path_to_filesystem(&self, path: &Path) -> Option<OsImageFileSystem> {
        self.filesystems()
            .filter(|fs| path.starts_with(&fs.mount_point))
            .max_by_key(|fs| fs.mount_point.components().count())
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
    pub(crate) fn filesystems(&self) -> Box<dyn Iterator<Item = OsImageFileSystem> + '_> {
        match &self.0 {
            OsImageInner::Cosi(cosi) => Box::new(cosi.filesystems()),
            #[cfg(test)]
            OsImageInner::Mock(mock) => Box::new(mock.filesystems()),
        }
    }

    /// Returns the root filesystem image.
    pub(crate) fn root_filesystem(&self) -> Option<OsImageFileSystem> {
        self.filesystems()
            .find(|fs| fs.mount_point == Path::new(ROOT_MOUNT_POINT_PATH))
    }

    pub(crate) fn metadata_sha384(&self) -> Sha384Hash {
        match &self.0 {
            OsImageInner::Cosi(cosi) => cosi.metadata_sha384(),
            #[cfg(test)]
            OsImageInner::Mock(mock) => mock.metadata_sha384(),
        }
    }
}

#[derive(Debug)]
pub struct OsImageFileSystem<'a> {
    pub mount_point: PathBuf,
    pub fs_type: OsImageFileSystemType,
    pub fs_uuid: OsUuid,
    pub part_type: DiscoverablePartitionType,
    pub image_file: OsImageFile<'a>,
    pub verity: Option<OsImageVerityHash<'a>>,
}

impl OsImageFileSystem<'_> {
    /// Returns whether the image has a verity hash.
    pub fn has_verity(&self) -> bool {
        self.verity.is_some()
    }
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

#[derive(Debug)]
pub struct OsImageVerityHash<'a> {
    pub roothash: String,
    pub hash_image_file: OsImageFile<'a>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Eq, PartialEq)]
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

impl From<OsImageFileSystemType> for RealFilesystemType {
    fn from(value: OsImageFileSystemType) -> Self {
        match value {
            OsImageFileSystemType::Ext4 => Self::Ext4,
            OsImageFileSystemType::Ext3 => Self::Ext4,
            OsImageFileSystemType::Ext2 => Self::Ext4,
            OsImageFileSystemType::Cramfs => Self::Cramfs,
            OsImageFileSystemType::Squashfs => Self::Squashfs,
            OsImageFileSystemType::Vfat => Self::Vfat,
            OsImageFileSystemType::Msdos => Self::Msdos,
            OsImageFileSystemType::Exfat => Self::Exfat,
            OsImageFileSystemType::Iso9660 => Self::Iso9660,
            OsImageFileSystemType::Ntfs => Self::Ntfs,
            OsImageFileSystemType::Btrfs => Self::Btrfs,
            OsImageFileSystemType::Xfs => Self::Xfs,
        }
    }
}

impl std::fmt::Debug for OsImageFile<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OsImageFile")
            .field("compressed_size", &self.compressed_size)
            .field("sha384", &self.sha384)
            .field("uncompressed_size", &self.uncompressed_size)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashSet;

    use osutils::osrelease::OsRelease;

    use mock::{MockImage, MOCK_OS_IMAGE_CONTENT};
    use uuid::Uuid;

    #[test]
    fn test_basic_properties() {
        let source_url = Url::parse("mock://").unwrap();
        let arch = SystemArchitecture::Amd64;
        let os_release = OsRelease {
            id: Some("os-id".into()),
            name: Some("os-name".into()),
            version: Some("os-version".into()),
            version_id: Some("os-version-id".into()),
            pretty_name: Some("pretty-name-1234".into()),
        };

        let mock = OsImage::mock(MockOsImage {
            source: source_url.clone(),
            os_arch: arch,
            os_release: os_release.clone(),
            is_uki: false,
            images: vec![
                MockImage::new(
                    "/boot/efi",
                    OsImageFileSystemType::Ext4,
                    DiscoverablePartitionType::Esp,
                    None::<&str>,
                ),
                MockImage::new(
                    "/boot",
                    OsImageFileSystemType::Ext4,
                    DiscoverablePartitionType::Xbootldr,
                    None::<&str>,
                ),
                MockImage::new(
                    "/",
                    OsImageFileSystemType::Ext4,
                    DiscoverablePartitionType::Root,
                    None::<&str>,
                ),
                MockImage::new(
                    "/var",
                    OsImageFileSystemType::Ext4,
                    DiscoverablePartitionType::Var,
                    None::<&str>,
                ),
            ],
        });

        assert_eq!(mock.source(), &source_url);
        assert_eq!(mock.architecture(), arch);

        assert_eq!(
            mock.available_mount_points().collect::<HashSet<&Path>>(),
            HashSet::from([Path::new("/boot"), Path::new("/"), Path::new("/var")])
        );
    }

    #[test]
    fn test_filesystem_getters() {
        // Array of the mount points in the OS image and random uuids to use as
        // verity hashed to validate we're grabbing the right filesystems.
        let some_uuid = || Uuid::new_v4().to_string();
        let mntpoints = [
            ("/boot/efi", DiscoverablePartitionType::Esp, some_uuid()),
            (
                "/boot",
                DiscoverablePartitionType::LinuxGeneric,
                some_uuid(),
            ),
            ("/", DiscoverablePartitionType::LinuxGeneric, some_uuid()),
            ("/var", DiscoverablePartitionType::LinuxGeneric, some_uuid()),
        ];

        let mock = OsImage::mock(MockOsImage::new().with_images(mntpoints.iter().map(
            |(mnt, part_type, verity)| {
                MockImage::new(*mnt, OsImageFileSystemType::Ext4, *part_type, Some(verity))
            },
        )));

        // TEST GET ALL FILESYSTEMS
        let filesystems = mock.filesystems().collect::<Vec<_>>();

        assert_eq!(filesystems.len(), 3);

        // We shouldn't have the ESP filesystem in the list of filesystems.
        let esp_fs = filesystems
            .iter()
            .find(|fs| fs.mount_point == Path::new("/boot/efi"));
        assert!(esp_fs.is_none());

        // We should have all filesystems EXCEPT the ESP filesystem.
        for (mnt, part_type, verity) in &mntpoints[1..] {
            let fs = filesystems
                .iter()
                .find(|fs| fs.mount_point == Path::new(*mnt))
                .unwrap();

            assert_eq!(fs.mount_point, Path::new(*mnt));
            assert_eq!(fs.part_type, *part_type);
            assert_eq!(fs.verity.as_ref().unwrap().roothash, *verity);
        }

        // TEST GET ESP FILESYSTEM
        let expected = mntpoints
            .iter()
            .find(|(_, part_type, _)| *part_type == DiscoverablePartitionType::Esp)
            .unwrap();
        let esp_fs = mock.esp_filesystem().unwrap();

        assert_eq!(esp_fs.mount_point, Path::new(expected.0));
        assert_eq!(esp_fs.part_type, expected.1);
        assert_eq!(
            esp_fs.verity.as_ref().unwrap().roothash,
            expected.2.to_string()
        );

        // TEST GET ROOT FILESYSTEM
        let expected = mntpoints.iter().find(|(mntp, _, _)| mntp == &"/").unwrap();
        let root_fs = mock.root_filesystem().unwrap();

        assert_eq!(root_fs.mount_point, Path::new(expected.0));
        assert_eq!(root_fs.part_type, expected.1);
        assert_eq!(
            root_fs.verity.as_ref().unwrap().roothash,
            expected.2.to_string()
        );
    }

    #[test]
    fn test_reader() {
        let mock = OsImage::mock(MockOsImage::new().with_images(vec![MockImage::new(
            "/boot",
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::LinuxGeneric,
            Some("some-verity-hash"),
        )]));

        let fs = mock.filesystems().next().unwrap();
        let mut reader = fs.image_file.reader().unwrap();

        let mut buf = String::new();
        reader.read_to_string(&mut buf).unwrap();

        assert_eq!(buf, MOCK_OS_IMAGE_CONTENT);
    }
}
