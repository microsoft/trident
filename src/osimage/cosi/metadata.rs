use std::{collections::HashSet, path::PathBuf};

use anyhow::{bail, ensure, Error};
use log::trace;
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use osutils::{
    arch::SystemArchitecture, osrelease::OsRelease, osuuid::OsUuid,
    partition_types::DiscoverablePartitionType,
};
use trident_api::primitives::hash::Sha384Hash;

use crate::osimage::OsImageFileSystemType;

use super::CosiEntry;

/// COSI metadata version reader.
///
/// This struct only attempts to parse the COSI metadata version to ensure that
/// the version is supported by the current implementation.
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct CosiMetadataVersion {
    /// The spec version of this COSI file.
    pub version: MetadataVersion,
}

/// COSI metadata as defined by the COSI specification.
///
/// [COSI Specification](/dev-docs/specs/Composable-OS-Image.md)

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CosiMetadata {
    /// The spec version of this COSI file.
    pub version: MetadataVersion,

    /// The architecture of the OS.
    pub os_arch: SystemArchitecture,

    /// The release of the OS.
    #[allow(dead_code)]
    pub os_release: OsRelease,

    /// The images that make up the OS.
    pub images: Vec<Image>,

    /// The list of OS packages that are part of the OS.
    ///
    /// The option is important to know if the list is present and empty or not
    /// present at all.
    #[allow(dead_code)]
    #[serde(default)]
    pub os_packages: Option<Vec<OsPackage>>,

    /// The unique ID of this COSI file.
    #[allow(dead_code)]
    #[serde(default)]
    pub id: Option<Uuid>,
}

impl CosiMetadata {
    /// Validates the COSI metadata.
    pub(super) fn validate(&self) -> Result<(), Error> {
        // Ensure that all mount points are unique.
        let mut mount_points = HashSet::new();
        for image in &self.images {
            if !mount_points.insert(&image.mount_point) {
                bail!("Duplicate mount point: '{}'", image.mount_point.display());
            }
        }

        Ok(())
    }

    /// Returns the ESP filesystem image.
    pub(super) fn get_esp_filesystem(&self) -> Result<&Image, Error> {
        let matches = self
            .images
            .iter()
            .filter(|image| image.is_esp())
            .collect::<Vec<_>>();

        ensure!(
            matches.len() == 1,
            "Expected exactly one ESP filesystem image, found {}",
            matches.len()
        );

        let esp_image = matches[0];

        trace!(
            "Found ESP filesystem image at '{}':   {:#?}",@
            esp_image.mount_point.display(),
            esp_image
        );

        Ok(esp_image)
    }

    /// Returns an iterator over all images that are NOT the ESP filesystem image.
    pub(super) fn get_regular_filesystems(&self) -> impl Iterator<Item = &Image> {
        self.images.iter().filter(|image| !image.is_esp())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct MetadataVersion {
    pub major: u32,
    pub minor: u32,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Image {
    #[serde(rename = "image")]
    pub file: ImageFile,

    pub mount_point: PathBuf,

    #[serde(deserialize_with = "display_fs_type_field_name")]
    pub fs_type: OsImageFileSystemType,

    #[allow(dead_code)]
    pub fs_uuid: OsUuid,

    pub part_type: DiscoverablePartitionType,

    pub verity: Option<VerityMetadata>,
}

impl Image {
    pub fn is_esp(&self) -> bool {
        self.part_type == DiscoverablePartitionType::Esp
    }
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImageFile {
    pub path: PathBuf,

    pub compressed_size: u64,

    pub uncompressed_size: u64,

    pub sha384: Sha384Hash,

    #[serde(skip)]
    pub(super) entry: CosiEntry,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VerityMetadata {
    #[serde(rename = "image")]
    pub file: ImageFile,

    pub roothash: String,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OsPackage {
    #[allow(dead_code)]
    pub name: String,

    #[allow(dead_code)]
    pub version: String,

    #[allow(dead_code)]
    #[serde(default)]
    pub release: Option<String>,

    #[allow(dead_code)]
    #[serde(default)]
    pub arch: Option<SystemArchitecture>,
}

impl<'de> Deserialize<'de> for MetadataVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let ver_str = String::deserialize(deserializer)?;
        let (major, minor) = ver_str.split_once('.').ok_or_else(|| {
            serde::de::Error::custom("version string must be in the format of 'major.minor'")
        })?;
        let major = major
            .parse::<u32>()
            .map_err(|_| serde::de::Error::custom("major version must be a valid u32"))?;
        let minor = minor
            .parse::<u32>()
            .map_err(|_| serde::de::Error::custom("minor version must be a valid u32"))?;
        Ok(MetadataVersion { major, minor })
    }
}

/// Displays a custom error message when deserializing `fs_type` field in an OS image, indicating
/// the name of the field that resulted in the deserialization error.
fn display_fs_type_field_name<'de, D>(deserializer: D) -> Result<OsImageFileSystemType, D::Error>
where
    D: serde::Deserializer<'de>,
{
    OsImageFileSystemType::deserialize(deserializer)
        .map_err(|err| serde::de::Error::custom(format!("Unknown filesystem type: {}", err)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_version_deserialization() {
        fn parse_version(version_str: &str, expected_major: u32, expected_minor: u32) {
            let version: MetadataVersion = serde_json::from_str(version_str).unwrap();
            assert_eq!(version.major, expected_major);
            assert_eq!(version.minor, expected_minor);
        }

        parse_version(r#""1.0""#, 1, 0);
        parse_version(r#""1.1""#, 1, 1);
        parse_version(r#""2.0""#, 2, 0);
        parse_version(r#""2.1""#, 2, 1);
    }

    #[test]
    fn test_metadata_version_deserialization_invalid() {
        fn assert_invalid_version(version_str: &str) {
            serde_json::from_str::<MetadataVersion>(version_str).unwrap_err();
        }

        assert_invalid_version(r#""1""#);
        assert_invalid_version(r#""1.0.0""#);
        assert_invalid_version(r#""abcd.efgh""#);
        assert_invalid_version(r#""hello there""#);
    }
}
