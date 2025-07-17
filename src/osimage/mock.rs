use std::{
    io::Cursor,
    path::{Path, PathBuf},
};

use anyhow::Error;
use serde::Deserialize;
use url::Url;
use uuid::Uuid;

use osutils::osrelease::OsRelease;
use sysdefs::{
    arch::SystemArchitecture, osuuid::OsUuid, partition_types::DiscoverablePartitionType,
};
use trident_api::primitives::hash::Sha384Hash;

use super::{OsImageFile, OsImageFileSystem, OsImageFileSystemType, OsImageVerityHash};

/// Content returned by the reader of a mock OS image file.
pub const MOCK_OS_IMAGE_CONTENT: &str = "mock-os-image-content-lorem-ipsum";

/// This is a generic abstraction of what an OS image is, which can be used to
/// mock an OS image for testing purposes. It should not be tied to the
/// specifics of any one OS image implementation. Currently does not include
/// verity.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MockOsImage {
    pub source: Url,

    pub os_arch: SystemArchitecture,

    pub os_release: OsRelease,

    pub images: Vec<MockImage>,

    pub is_uki: bool,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MockImage {
    pub mount_point: PathBuf,

    pub fs_type: OsImageFileSystemType,

    pub fs_uuid: OsUuid,

    pub part_type: DiscoverablePartitionType,

    pub verity: Option<MockVerity>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MockVerity {
    pub roothash: String,
}

fn mock_os_image_file() -> OsImageFile<'static> {
    OsImageFile {
        compressed_size: 0,
        sha384: Sha384Hash::from("mock-sha384"),
        uncompressed_size: 0,
        reader: Box::new(|| {
            Ok(Box::new(Cursor::new(
                MOCK_OS_IMAGE_CONTENT.as_bytes().to_vec(),
            )))
        }),
    }
}

impl MockOsImage {
    /// Returns a new mock OS image with dummy data.
    pub fn new() -> Self {
        Self {
            source: Url::parse("mock://").unwrap(),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            is_uki: false,
            images: vec![],
        }
    }

    /// Adds an image to the mock OS image.
    pub fn with_image(mut self, image: MockImage) -> Self {
        self.images.push(image);
        self
    }

    /// Adds an image to the mock OS image.
    pub fn with_images(mut self, images: impl IntoIterator<Item = MockImage>) -> Self {
        self.images.extend(images);
        self
    }

    /// Returns an iterator of available mount points in the COSI file.
    pub(super) fn available_mount_points(&self) -> impl Iterator<Item = &Path> {
        self.images
            .iter()
            .filter(|fs| fs.part_type != DiscoverablePartitionType::Esp)
            .map(|image| image.mount_point.as_path())
    }

    /// Returns the ESP filesystem image.
    pub fn esp_filesystem(&self) -> Result<OsImageFileSystem, Error> {
        if let Some(esp_img) = self
            .images
            .iter()
            .find(|fs| fs.part_type == DiscoverablePartitionType::Esp)
        {
            Ok(OsImageFileSystem {
                mount_point: esp_img.mount_point.clone(),
                fs_type: esp_img.fs_type,
                fs_uuid: esp_img.fs_uuid.clone(),
                part_type: esp_img.part_type,
                image_file: mock_os_image_file(),
                verity: esp_img.verity.as_ref().map(|verity| OsImageVerityHash {
                    roothash: verity.roothash.clone(),
                    hash_image_file: mock_os_image_file(),
                }),
            })
        } else {
            Err(Error::msg("No ESP filesystem found"))
        }
    }

    /// Returns non-ESP filesystems.
    pub fn filesystems(&self) -> impl Iterator<Item = OsImageFileSystem> {
        self.images
            .iter()
            .filter(|fs| fs.part_type != DiscoverablePartitionType::Esp)
            .map(|image| OsImageFileSystem {
                mount_point: image.mount_point.clone(),
                fs_type: image.fs_type,
                fs_uuid: image.fs_uuid.clone(),
                part_type: image.part_type,
                image_file: mock_os_image_file(),
                verity: image.verity.as_ref().map(|verity| OsImageVerityHash {
                    roothash: verity.roothash.clone(),
                    hash_image_file: mock_os_image_file(),
                }),
            })
    }

    /// Returns the OS architecture of the image.
    pub fn architecture(&self) -> SystemArchitecture {
        self.os_arch
    }

    pub fn metadata_sha384(&self) -> Sha384Hash {
        Sha384Hash::from("0".repeat(96))
    }
}

impl MockImage {
    /// Returns a new mock image with dummy data.
    pub fn new(
        mount_point: impl AsRef<Path>,
        fs_type: OsImageFileSystemType,
        part_type: DiscoverablePartitionType,
        roothash: Option<impl AsRef<str>>,
    ) -> Self {
        Self {
            mount_point: mount_point.as_ref().to_owned(),
            fs_type,
            part_type,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            verity: roothash.map(|roothash| MockVerity {
                roothash: roothash.as_ref().to_owned(),
            }),
        }
    }
}
