use std::{collections::HashSet, path::PathBuf};

use anyhow::{bail, ensure, Error};
use log::trace;
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use osutils::osrelease::OsRelease;
use sysdefs::{
    arch::{PackageArchitecture, SystemArchitecture},
    osuuid::OsUuid,
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

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
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

    /// The bootloader of this COSI file.
    #[allow(dead_code)]
    #[serde(default)]
    pub bootloader: Option<Bootloader>,
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

        // Validate bootloader
        match &self.bootloader {
            Some(Bootloader {
                bootloader_type,
                systemd_boot,
            }) => {
                match &**bootloader_type {
                    "grub" => {
                        // Validate that for grub, there are no systemd-boot entries
                        if systemd_boot.is_some() {
                            bail!("Bootloader type 'grub' cannot have systemd-boot entries");
                        }
                    }
                    "systemd-boot" => {
                        // Validate that for systemd, there is exactly 1 entry and it is uki
                        if systemd_boot.is_none()
                            || systemd_boot.as_ref().unwrap().entries.len() != 1
                        {
                            bail!("Bootloader type 'systemd-boot' must have exactly one entry");
                        }
                        let entry = &systemd_boot.as_ref().unwrap().entries[0];
                        if entry.boot_type != "uki-standalone" {
                            bail!(
                                "Unsupported boot entry type for 'systemd-boot': {}",
                                entry.boot_type
                            );
                        }
                    }
                    _ => bail!("Unsupported bootloader type: {}", bootloader_type),
                }
            }
            None => {
                if self.version.major > 1 || (self.version.major == 1 && self.version.minor > 0) {
                    bail!("Bootloader is required for COSI version >= 1.1, but not provided");
                }
            }
        }

        Ok(())
    }

    pub(super) fn is_uki(&self) -> bool {
        match &self.bootloader {
            Some(bootloader) => bootloader.systemd_boot.iter().any(|sb| {
                sb.entries
                    .iter()
                    .any(|entry| entry.boot_type == "uki-standalone")
            }),
            None => false,
        }
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
            "Found ESP filesystem image at '{}': {:#?}",
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

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct MetadataVersion {
    pub major: u32,
    pub minor: u32,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Image {
    #[serde(rename = "image")]
    pub file: ImageFile,

    pub mount_point: PathBuf,

    #[serde(deserialize_with = "display_fs_type_field_name")]
    pub fs_type: OsImageFileSystemType,

    pub fs_uuid: OsUuid,

    pub part_type: DiscoverablePartitionType,

    pub verity: Option<VerityMetadata>,
}

impl Image {
    pub fn is_esp(&self) -> bool {
        self.part_type == DiscoverablePartitionType::Esp
    }
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImageFile {
    pub path: PathBuf,

    pub compressed_size: u64,

    pub uncompressed_size: u64,

    pub sha384: Sha384Hash,

    #[serde(skip)]
    pub(super) entry: CosiEntry,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VerityMetadata {
    #[serde(rename = "image")]
    pub file: ImageFile,

    pub roothash: String,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
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
    pub arch: Option<PackageArchitecture>,
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
        .map_err(|err| serde::de::Error::custom(format!("Unknown filesystem type: {err}")))
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Bootloader {
    #[allow(dead_code)]
    #[serde(rename = "type")]
    pub bootloader_type: String,

    #[allow(dead_code)]
    pub systemd_boot: Option<SystemdBoot>,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SystemdBoot {
    #[allow(dead_code)]
    pub entries: Vec<BootloaderEntry>,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BootloaderEntry {
    #[allow(dead_code)]
    #[serde(rename = "type")]
    pub boot_type: String,

    #[allow(dead_code)]
    pub kernel: String,

    #[allow(dead_code)]
    pub path: String,

    #[allow(dead_code)]
    pub cmdline: String,
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

    fn mock_image_file() -> ImageFile {
        ImageFile {
            path: PathBuf::from("/path/to/image"),
            compressed_size: 50,
            uncompressed_size: 100,
            sha384: Sha384Hash::from("sample_sha384"),
            entry: CosiEntry::default(),
        }
    }

    #[test]
    fn test_get_esp_filesystem() {
        let mut metadata = CosiMetadata {
            version: MetadataVersion { major: 1, minor: 0 },
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            images: vec![], // Empty images
            os_packages: None,
            id: None,
            bootloader: None,
        };

        // No images
        assert_eq!(
            metadata.get_esp_filesystem().unwrap_err().to_string(),
            "Expected exactly one ESP filesystem image, found 0"
        );

        // Two images, neither is ESP
        metadata.images = vec![
            Image {
                file: mock_image_file(),
                mount_point: PathBuf::from("/mnt"),
                fs_type: OsImageFileSystemType::Ext4,
                fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                part_type: DiscoverablePartitionType::LinuxGeneric,
                verity: None,
            },
            Image {
                file: mock_image_file(),
                mount_point: PathBuf::from("/var"),
                fs_type: OsImageFileSystemType::Ext4,
                fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                part_type: DiscoverablePartitionType::LinuxGeneric,
                verity: None,
            },
        ];

        assert_eq!(
            metadata.get_esp_filesystem().unwrap_err().to_string(),
            "Expected exactly one ESP filesystem image, found 0"
        );

        // Three images, one is ESP
        let esp_img = Image {
            file: mock_image_file(),
            mount_point: PathBuf::from("/boot/efi"),
            fs_type: OsImageFileSystemType::Vfat,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::Esp,
            verity: None,
        };
        metadata.images.push(esp_img.clone());
        assert_eq!(metadata.get_esp_filesystem().unwrap(), &esp_img);

        // Four images, two are ESP
        metadata.images.push(Image {
            file: mock_image_file(),
            mount_point: PathBuf::from("/boot/efi2"),
            fs_type: OsImageFileSystemType::Vfat,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::Esp,
            verity: None,
        });
        assert_eq!(
            metadata.get_esp_filesystem().unwrap_err().to_string(),
            "Expected exactly one ESP filesystem image, found 2"
        );
    }

    #[test]
    fn test_get_regular_filesystems() {
        let mut metadata = CosiMetadata {
            version: MetadataVersion { major: 1, minor: 0 },
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            images: vec![], // Empty images
            os_packages: None,
            id: None,
            bootloader: None,
        };

        // No images
        assert_eq!(metadata.get_regular_filesystems().count(), 0);

        // Two images, neither is ESP
        metadata.images = vec![
            Image {
                file: mock_image_file(),
                mount_point: PathBuf::from("/mnt"),
                fs_type: OsImageFileSystemType::Ext4,
                fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                part_type: DiscoverablePartitionType::LinuxGeneric,
                verity: None,
            },
            Image {
                file: mock_image_file(),
                mount_point: PathBuf::from("/var"),
                fs_type: OsImageFileSystemType::Ext4,
                fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                part_type: DiscoverablePartitionType::LinuxGeneric,
                verity: None,
            },
        ];
        assert_eq!(metadata.get_regular_filesystems().count(), 2);

        // Three images, one is ESP
        metadata.images.push(Image {
            file: mock_image_file(),
            mount_point: PathBuf::from("/boot/efi"),
            fs_type: OsImageFileSystemType::Vfat,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::Esp,
            verity: None,
        });
        assert_eq!(metadata.get_regular_filesystems().count(), 2);

        // Four images, two are ESP
        metadata.images.push(Image {
            file: mock_image_file(),
            mount_point: PathBuf::from("/boot/efi2"),
            fs_type: OsImageFileSystemType::Vfat,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::Esp,
            verity: None,
        });
        assert_eq!(metadata.get_regular_filesystems().count(), 2);

        // Two images, both are ESP
        metadata.images = vec![
            Image {
                file: mock_image_file(),
                mount_point: PathBuf::from("/boot/efi"),
                fs_type: OsImageFileSystemType::Vfat,
                fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                part_type: DiscoverablePartitionType::Esp,
                verity: None,
            },
            Image {
                file: mock_image_file(),
                mount_point: PathBuf::from("/boot/efi2"),
                fs_type: OsImageFileSystemType::Vfat,
                fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                part_type: DiscoverablePartitionType::Esp,
                verity: None,
            },
        ];
        assert_eq!(metadata.get_regular_filesystems().count(), 0);
    }

    #[test]
    fn test_noarch_os_packages() {
        let noarch_os_package_json = r#"
        {
            "name": "package1",
            "version": "1.0.0",
            "arch": "noarch"
        }
        "#;
        let _noarch_os_package: OsPackage = serde_json::from_str(noarch_os_package_json).unwrap();

        let amd64_os_package_json = r#"
        {
            "name": "package1",
            "version": "1.0.0",
            "arch": "x86_64"
        }
        "#;
        let _amd64_os_package: OsPackage = serde_json::from_str(amd64_os_package_json).unwrap();

        let amd64_os_package_json = r#"
        {
            "name": "package1",
            "version": "1.0.0",
            "arch": "amd64"
        }
        "#;
        let _amd64_os_package: OsPackage = serde_json::from_str(amd64_os_package_json).unwrap();

        let none_os_package_json = r#"
        {
            "name": "gpg-pubkey",
            "version": "3135ce90",
            "release": "5e6fda74",
            "arch": "(none)"
        }
        "#;
        let _none_os_package: OsPackage = serde_json::from_str(none_os_package_json).unwrap();
    }
}
