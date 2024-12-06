use anyhow::Error;
use serde::Deserialize;
use std::path::PathBuf;
use url::Url;

use osutils::{
    arch::SystemArchitecture, osrelease::OsRelease, osuuid::OsUuid,
    partition_types::DiscoverablePartitionType,
};

use super::OsImageFileSystemType;

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

#[derive(Debug, Clone)]
pub struct MockFileSystemImage<'a> {
    pub image: &'a MockImage,
}

impl MockOsImage {
    /// Returns the ESP filesystem image.
    #[allow(dead_code)]
    pub fn esp_filesystem(&self) -> Result<MockFileSystemImage, Error> {
        if let Some(esp_img) = self
            .images
            .iter()
            .find(|fs| fs.part_type == DiscoverablePartitionType::Esp)
        {
            Ok(MockFileSystemImage { image: esp_img })
        } else {
            Err(Error::msg("No ESP filesystem found"))
        }
    }

    /// Returns non-ESP filesystems.
    pub fn filesystems(&self) -> impl Iterator<Item = MockFileSystemImage<'_>> {
        self.images
            .iter()
            .filter(|fs| fs.part_type != DiscoverablePartitionType::Esp)
            .map(|image| MockFileSystemImage { image })
    }

    /// Returns the OS architecture of the image.
    #[allow(dead_code)]
    pub fn architecture(&self) -> SystemArchitecture {
        self.os_arch
    }
}
