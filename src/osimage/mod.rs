use std::{
    fmt::{Display, Formatter},
    io::{Error as IoError, Read},
    path::{Path, PathBuf},
};

use anyhow::Error;
use serde::{Deserialize, Serialize};
use url::Url;

use osutils::{arch::SystemArchitecture, partition_types::DiscoverablePartitionType};
use trident_api::{constants::ROOT_MOUNT_POINT_PATH, primitives::hash::Sha384Hash};

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
    pub(crate) fn cosi(url: &Url) -> Result<Self, Error> {
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

    /// Returns the root filesystem image.
    pub(crate) fn root_filesystem(&self) -> Option<OsImageFileSystem> {
        self.filesystems()
            .find(|fs| fs.mount_point == Path::new(ROOT_MOUNT_POINT_PATH))
    }
}

#[derive(Debug)]
pub struct OsImageFileSystem<'a> {
    pub mount_point: PathBuf,
    pub fs_type: OsImageFileSystemType,
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
        let arch = SystemArchitecture::Other;
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

        assert_eq!(mock.name(), "Mock");
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
