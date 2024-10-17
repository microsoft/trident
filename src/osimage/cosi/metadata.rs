use std::path::PathBuf;

use osutils::osuuid::OsUuid;
use serde::{Deserialize, Deserializer};

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(super) struct CosiMetadata {
    pub version: MetadataVersion,
    pub os_release: String,
    pub images: Vec<Image>,
    #[serde(default)]
    pub os_packages: Vec<OsPackage>,
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct MetadataVersion {
    pub major: u32,
    pub minor: u32,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Image {
    pub image: ImageFile,
    pub mount_point: PathBuf,
    pub fs_type: String,
    pub fs_uuid: OsUuid,
    pub part_type: String,
    pub verity: Option<VerityMetadata>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ImageFile {
    pub path: PathBuf,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub sha384: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct VerityMetadata {
    pub image: ImageFile,
    pub roothash: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct OsPackage {
    pub name: String,
    pub version: String,
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
