use std::path::{Path, PathBuf};

use anyhow::Error;
use serde::Deserialize;
use url::Url;

use osutils::{
    arch::SystemArchitecture, osrelease::OsRelease, osuuid::OsUuid,
    partition_types::DiscoverablePartitionType,
};
use trident_api::primitives::hash::Sha384Hash;

use super::{OsImageFile, OsImageFileSystem, OsImageFileSystemType};

/// This is a generic abstraction of what an OS image is, which can be used to
/// mock an OS image for testing purposes. It should not be tied to the
/// specifics of any one OS image implementation. Currently does not include
/// verity.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MockOsImage {
    pub source: Url,

    pub os_arch: SystemArchitecture,

    #[allow(dead_code)]
    pub os_release: OsRelease,

    pub images: Vec<MockImage>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MockImage {
    pub mount_point: PathBuf,

    pub fs_type: OsImageFileSystemType,

    #[allow(dead_code)]
    pub fs_uuid: OsUuid,

    pub part_type: DiscoverablePartitionType,
}

fn mock_os_image_file() -> OsImageFile<'static> {
    OsImageFile {
        compressed_size: 0,
        sha384: Sha384Hash::from("mock-sha384"),
        uncompressed_size: 0,
        reader: Box::new(|| unimplemented!("Mock image reader is not implemented!")),
    }
}

impl MockOsImage {
    /// Returns an iterator of available mount points in the COSI file.
    pub(super) fn available_mount_points(&self) -> impl Iterator<Item = &Path> {
        self.images
            .iter()
            .filter(|fs| fs.part_type != DiscoverablePartitionType::Esp)
            .map(|image| image.mount_point.as_path())
    }

    /// Returns the ESP filesystem image.
    #[allow(dead_code)]
    pub fn esp_filesystem(&self) -> Result<OsImageFileSystem, Error> {
        if let Some(esp_img) = self
            .images
            .iter()
            .find(|fs| fs.part_type == DiscoverablePartitionType::Esp)
        {
            Ok(OsImageFileSystem {
                mount_point: esp_img.mount_point.clone(),
                fs_type: esp_img.fs_type,
                part_type: esp_img.part_type,
                image_file: mock_os_image_file(),
                image_file_verity: None,
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
                part_type: image.part_type,
                image_file: mock_os_image_file(),
                image_file_verity: None,
            })
    }

    /// Returns the OS architecture of the image.
    #[allow(dead_code)]
    pub fn architecture(&self) -> SystemArchitecture {
        self.os_arch
    }
}
