use std::{cmp::Ordering, collections::HashSet, fmt::Display, path::PathBuf};

use anyhow::{ensure, Error};
use log::{trace, warn};
use serde::{Deserialize, Deserializer};
use strum_macros::Display;
use uuid::Uuid;

use osutils::osrelease::OsRelease;
use sysdefs::{
    arch::{PackageArchitecture, SystemArchitecture},
    osuuid::OsUuid,
    partition_types::DiscoverablePartitionType,
};
use trident_api::primitives::hash::Sha384Hash;

use crate::osimage::OsImageFileSystemType;

use super::error::{CosiMetadataError, CosiMetadataErrorKind};
use super::CosiEntry;

/// Enum of known COSI metadata versions up to the current implementation.
enum KnownMetadataVersion {
    /// Base version of the COSI metadata specification.
    #[allow(dead_code)]
    V1_0,

    /// COSI metadata specification version 1.1.
    ///
    /// Introduces bootloader metadata.
    V1_1,
}

impl KnownMetadataVersion {
    fn as_version(&self) -> MetadataVersion {
        match self {
            KnownMetadataVersion::V1_0 => MetadataVersion { major: 1, minor: 0 },
            KnownMetadataVersion::V1_1 => MetadataVersion { major: 1, minor: 1 },
        }
    }
}

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

    /// Template for a host configuration embedded within the image.
    #[serde(default)]
    pub host_configuration_template: Option<String>,
}

impl CosiMetadata {
    /// Validates the COSI metadata.
    pub(super) fn validate(&self) -> Result<(), CosiMetadataError> {
        let mk_err = |kind: CosiMetadataErrorKind| {
            Err(CosiMetadataError {
                version: self.version,
                kind,
            })
        };

        // Ensure that all mount points are unique.
        let mut mount_points = HashSet::new();
        for image in &self.images {
            if !mount_points.insert(&image.mount_point) {
                return mk_err(CosiMetadataErrorKind::V1_0DuplicateMountPoint(
                    image.mount_point.display().to_string(),
                ));
            }
        }

        // Validate bootloader on COSI version >= 1.1
        if self.version >= KnownMetadataVersion::V1_1.as_version() {
            let Some(bootloader) = self.bootloader.as_ref() else {
                return mk_err(CosiMetadataErrorKind::V1_1BootloaderRequired);
            };

            match (&bootloader.bootloader_type, &bootloader.systemd_boot) {
                // Grub with systemd-boot entries is invalid
                (BootloaderType::Grub, Some(_)) => {
                    return mk_err(CosiMetadataErrorKind::V1_1GrubWithSystemdBootEntries);
                }

                // Systemd-boot without entries is invalid
                (BootloaderType::SystemdBoot, None) => {
                    return mk_err(CosiMetadataErrorKind::V1_1SystemdBootMissingEntries);
                }

                // Systemd-boot with not exactly 1 UKI entry is invalid for this version of Trident
                (BootloaderType::SystemdBoot, Some(systemd_boot)) => {
                    match systemd_boot.entries.as_slice() {
                        // No entries is invalid
                        [] => {
                            return mk_err(
                                CosiMetadataErrorKind::V1_1SystemdBootEmptyEntries,
                            );
                        }

                        // First entry MUST be of type 'uki-standalone'
                        [entry, ..] if !entry.boot_type.eq(&SystemdBootloaderType::UkiStandalone) => warn!(
                            "First entry of bootloader type 'systemd-boot' is not of type 'uki-standalone'"
                        ),

                        // Having more than one entry is warned about, only the first is used in this version of Trident.
                        [_, rest @..] if !rest.is_empty() => warn!(
                            "Bootloader type 'systemd-boot' has more than one entry, only the first entry will be used"
                        ),

                        // Everything else is OK
                        _ => {}
                    }
                }

                // Unknown bootloader type is warned about, it may cause issues down the line
                (BootloaderType::Unknown(bootloader_type), _) => {
                    log::warn!("Unknown bootloader type: {}", bootloader_type)
                }

                // Everything else is OK
                _ => {}
            }

            // Ensure osPackages are present and all required info is provided.
            let Some(os_packages) = &self.os_packages else {
                return mk_err(CosiMetadataErrorKind::V1_1OsPackagesRequired);
            };

            // Ensure both release and arch are provided.
            for os_package in os_packages {
                if os_package.release.is_none() {
                    return mk_err(CosiMetadataErrorKind::V1_1OsPackageMissingRelease(
                        os_package.name.clone(),
                    ));
                }

                if os_package.arch.is_none() {
                    return mk_err(CosiMetadataErrorKind::V1_1OsPackageMissingArch(
                        os_package.name.clone(),
                    ));
                }
            }
        }

        Ok(())
    }

    // Returns whether the COSI metadata describes a standalone-UKI-based
    // bootloader. In this version of Trident, only the FIRST entry is
    // considered.
    pub(crate) fn is_uki(&self) -> bool {
        let Some(bootloader) = &self.bootloader else {
            return false;
        };

        let Some(sdb) = &bootloader.systemd_boot else {
            return false;
        };

        let Some(first_entry) = sdb.entries.first() else {
            return false;
        };

        first_entry.boot_type == SystemdBootloaderType::UkiStandalone
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

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct MetadataVersion {
    pub major: u32,
    pub minor: u32,
}

impl PartialOrd for MetadataVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MetadataVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.major.cmp(&other.major) {
            Ordering::Equal => self.minor.cmp(&other.minor),
            ord => ord,
        }
    }
}

impl Display for MetadataVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub(crate) struct ImageFile {
    pub path: PathBuf,

    pub compressed_size: u64,

    pub uncompressed_size: u64,

    pub sha384: Sha384Hash,

    #[serde(skip)]
    pub(super) entry: CosiEntry,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct VerityMetadata {
    #[serde(rename = "image")]
    pub file: ImageFile,

    pub roothash: String,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
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
#[serde(rename_all = "camelCase")]
pub(crate) struct Bootloader {
    #[allow(dead_code)]
    #[serde(rename = "type")]
    pub bootloader_type: BootloaderType,

    #[allow(dead_code)]
    pub systemd_boot: Option<SystemdBoot>,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
pub(crate) enum BootloaderType {
    #[serde(rename = "systemd-boot")]
    SystemdBoot,

    #[serde(rename = "grub")]
    Grub,

    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SystemdBoot {
    #[allow(dead_code)]
    pub entries: Vec<BootloaderEntry>,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootloaderEntry {
    #[allow(dead_code)]
    #[serde(rename = "type")]
    pub boot_type: SystemdBootloaderType,

    #[allow(dead_code)]
    pub kernel: String,

    #[allow(dead_code)]
    pub path: String,

    #[allow(dead_code)]
    pub cmdline: String,
}

#[derive(Debug, Deserialize, Clone, Eq, PartialEq, Display)]
pub(crate) enum SystemdBootloaderType {
    #[serde(rename = "uki-standalone")]
    #[strum(to_string = "uki-standalone")]
    UkiStandalone,

    #[serde(rename = "uki-config")]
    #[strum(to_string = "uki-config")]
    UkiConfig,

    #[serde(rename = "config")]
    #[strum(to_string = "config")]
    Config,

    #[serde(untagged)]
    #[strum(to_string = "{0}")]
    Unknown(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const SAMPLE_SHA384: &str = "1d0f284efe3edea4b9ca3bd514fa134b17eae361ccc7a1eefeff801b9bd6604e01f21f6bf249ef030599f0c218f2ba8c";

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
            host_configuration_template: None,
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
            host_configuration_template: None,
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

    #[test]
    fn test_extraneous_fields_deserialization() {
        // Ensure that unknown fields are ignored by serde for all objects we deserialize.
        // This helps with forward-compatibility when the COSI spec adds new fields.
        let metadata_with_extras = json!({
            "version": "1.1",
            "osArch": "amd64",
            "osRelease": "ID=azurelinux\nVERSION_ID=3.0\n",
            "images": [
                {
                    "image": {
                        "path": "/path/to/image1",
                        "compressedSize": 100,
                        "uncompressedSize": 200,
                        "sha384": SAMPLE_SHA384,
                        "unknownImageField": {"nested": true}
                    },
                    "mountPoint": "/mnt",
                    "fsType": "ext4",
                    "fsUuid": "550e8400-e29b-41d4-a716-446655440000",
                    "partType": "linux-generic",
                    "verity": {
                        "image": {
                            "path": "/path/to/verity1",
                            "compressedSize": 50,
                            "uncompressedSize": 100,
                            "sha384": SAMPLE_SHA384,
                            "unknownVerityImageField": 123
                        },
                        "roothash": "abcd",
                        "unknownVerityField": "ignored"
                    },
                    "unknownImageObjField": [1, 2, 3]
                }
            ],
            "osPackages": [
                {
                    "name": "bash",
                    "version": "1.0.0",
                    "release": "1",
                    "arch": "noarch",
                    "unknownOsPackageField": "ignored"
                }
            ],
            "bootloader": {
                "type": "systemd-boot",
                "systemdBoot": {
                    "entries": [
                        {
                            "type": "uki-standalone",
                            "kernel": "vmlinuz",
                            "path": "EFI/Linux/uki.efi",
                            "cmdline": "quiet",
                            "unknownBootEntryField": {"k": "v"}
                        }
                    ],
                    "unknownSystemdBootField": true
                },
                "unknownBootloaderField": "ignored"
            },
            "hostConfigurationTemplate": "ignored",
            "unknownTopLevelField": {"future": "field"}
        });

        // This unwrap ensures deserialization succeeds even with extra fields.
        // The validate call ensures we don't accidentally construct a shape that can't be used.
        parse_and_validate(metadata_with_extras).unwrap();
    }

    #[test]
    fn test_is_uki() {
        fn parse(value: Value) -> CosiMetadata {
            serde_json::from_value(value).unwrap()
        }

        // Base COSI metadata (v1.1) with a standalone UKI as the first systemd-boot entry.
        let base = json!({
            "version": "1.1",
            "osArch": "amd64",
            "osRelease": "ID=azurelinux\nVERSION_ID=3.0\n",
            "images": [],
            "osPackages": [
                {
                    "name": "bash",
                    "version": "1.0.0",
                    "release": "1",
                    "arch": "noarch"
                }
            ],
            "bootloader": {
                "type": "systemd-boot",
                "systemdBoot": {
                    "entries": [
                        {
                            "type": "uki-standalone",
                            "kernel": "vmlinuz",
                            "path": "EFI/Linux/uki.efi",
                            "cmdline": "quiet"
                        }
                    ]
                }
            }
        });

        assert!(parse(base.clone()).is_uki());

        // No bootloader => false.
        let mut no_bootloader = base.clone();
        no_bootloader.as_object_mut().unwrap().remove("bootloader");
        assert!(!parse(no_bootloader).is_uki());

        // No systemdBoot section => false.
        let mut no_systemd_boot = base.clone();
        no_systemd_boot["bootloader"]
            .as_object_mut()
            .unwrap()
            .remove("systemdBoot");
        assert!(!parse(no_systemd_boot).is_uki());

        // Empty entries => false.
        let mut empty_entries = base.clone();
        empty_entries["bootloader"]["systemdBoot"]["entries"] = json!([]);
        assert!(!parse(empty_entries).is_uki());

        // First entry is not uki-standalone => false.
        let mut first_not_uki = base.clone();
        first_not_uki["bootloader"]["systemdBoot"]["entries"][0]["type"] = json!("config");
        assert!(!parse(first_not_uki).is_uki());

        // Only the FIRST entry is considered.
        // If first is not uki-standalone but later entries are, result is still false.
        let mut second_is_uki = base.clone();
        second_is_uki["bootloader"]["systemdBoot"]["entries"] = json!([
            {
                "type": "config",
                "kernel": "vmlinuz",
                "path": "EFI/Linux/other.efi",
                "cmdline": "quiet"
            },
            {
                "type": "uki-standalone",
                "kernel": "vmlinuz",
                "path": "EFI/Linux/uki.efi",
                "cmdline": "quiet"
            }
        ]);
        assert!(!parse(second_is_uki).is_uki());
    }

    /// Helper to parse and validate COSI metadata, returning only the validation error kind.
    fn parse_and_validate(value: Value) -> Result<(), CosiMetadataErrorKind> {
        let metadata: CosiMetadata = serde_json::from_value(value).unwrap();
        metadata.validate().map_err(|e| e.kind)
    }

    /// Helper to assert that parsing and validating the given COSI metadata value
    /// results in the expected validation error kind.
    fn assert_validate_err_kind(value: Value, expected: CosiMetadataErrorKind) {
        let err_kind = parse_and_validate(value).unwrap_err();
        assert_eq!(err_kind, expected);
    }

    #[test]
    fn test_cosi_1_0_validation() {
        // Base COSI metadata (v1.0) that is valid for this version of Trident.
        let base = json!({
            "version": "1.0",
            "osArch": "amd64",
            "osRelease": "ID=azurelinux\nVERSION_ID=3.0\n",
            "images": [
                {
                    "image": {
                        "path": "/path/to/image1",
                        "compressedSize": 100,
                        "uncompressedSize": 200,
                        "sha384": SAMPLE_SHA384
                    },
                    "mountPoint": "/mnt",
                    "fsType": "ext4",
                    "fsUuid": "550e8400-e29b-41d4-a716-446655440000",
                    "partType": "linux-generic"
                },
                {
                    "image": {
                        "path": "/path/to/image2",
                        "compressedSize": 150,
                        "uncompressedSize": 300,
                        "sha384": SAMPLE_SHA384
                    },
                    "mountPoint": "/var",
                    "fsType": "ext4",
                    "fsUuid": "550e8400-e29b-41d4-a716-446655440001",
                    "partType": "linux-generic"
                }
            ]
        });

        // Sanity: base should validate.
        parse_and_validate(base.clone()).unwrap();

        // Duplicate mount point should error.
        let mut duplicate_mount = base.clone();
        duplicate_mount["images"][1]["mountPoint"] = json!("/mnt");
        assert_validate_err_kind(
            duplicate_mount,
            CosiMetadataErrorKind::V1_0DuplicateMountPoint("/mnt".to_string()),
        );
    }

    #[test]
    fn test_cosi_1_1_validation() {
        // Base COSI metadata (v1.1) that is valid for this version of Trident.
        // We keep `images` empty to focus coverage on the v1.1 bootloader validation.
        let base = json!({
            "version": "1.1",
            "osArch": "amd64",
            "osRelease": "ID=azurelinux\nVERSION_ID=3.0\n",
            "images": [],
            "osPackages": [
                {
                    "name": "bash",
                    "version": "1.0.0",
                    "release": "1",
                    "arch": "noarch"
                }
            ],
            "bootloader": {
                "type": "systemd-boot",
                "systemdBoot": {
                    "entries": [
                        {
                            "type": "uki-standalone",
                            "kernel": "vmlinuz",
                            "path": "EFI/Linux/uki.efi",
                            "cmdline": "quiet"
                        }
                    ]
                }
            }
        });

        // Sanity: base should validate.
        parse_and_validate(base.clone()).unwrap();

        // v1.0 does not require bootloader metadata.
        let mut v1_0 = base.clone();
        v1_0["version"] = json!("1.0");
        if let Some(obj) = v1_0.as_object_mut() {
            obj.remove("bootloader");
        }
        parse_and_validate(v1_0).unwrap();

        // v1.1 requires bootloader metadata.
        let mut no_bootloader = base.clone();
        if let Some(obj) = no_bootloader.as_object_mut() {
            obj.remove("bootloader");
        }
        assert_validate_err_kind(no_bootloader, CosiMetadataErrorKind::V1_1BootloaderRequired);

        // Grub with systemd-boot entries is invalid.
        let mut grub_with_sdb = base.clone();
        grub_with_sdb["bootloader"]["type"] = json!("grub");
        assert_validate_err_kind(
            grub_with_sdb,
            CosiMetadataErrorKind::V1_1GrubWithSystemdBootEntries,
        );

        // systemd-boot without systemd-boot entries is invalid.
        let mut sdb_missing_entries = base.clone();
        if let Some(obj) = sdb_missing_entries["bootloader"].as_object_mut() {
            obj.remove("systemdBoot");
        }
        assert_validate_err_kind(
            sdb_missing_entries,
            CosiMetadataErrorKind::V1_1SystemdBootMissingEntries,
        );

        // systemd-boot with empty entries is invalid.
        let mut sdb_empty_entries = base.clone();
        sdb_empty_entries["bootloader"]["systemdBoot"]["entries"] = json!([]);
        assert_validate_err_kind(
            sdb_empty_entries,
            CosiMetadataErrorKind::V1_1SystemdBootEmptyEntries,
        );

        // systemd-boot with first entry NOT uki-standalone only warns.
        let mut sdb_first_not_uki = base.clone();
        sdb_first_not_uki["bootloader"]["systemdBoot"]["entries"][0]["type"] = json!("config");
        parse_and_validate(sdb_first_not_uki).unwrap();

        // systemd-boot with more than one entry only warns.
        let mut sdb_multiple_entries = base.clone();
        sdb_multiple_entries["bootloader"]["systemdBoot"]["entries"] = json!([
            {
                "type": "uki-standalone",
                "kernel": "vmlinuz",
                "path": "EFI/Linux/uki.efi",
                "cmdline": "quiet"
            },
            {
                "type": "config",
                "kernel": "vmlinuz2",
                "path": "EFI/Linux/other.efi",
                "cmdline": "debug"
            }
        ]);
        parse_and_validate(sdb_multiple_entries).unwrap();

        // Unknown bootloader type only warns.
        let mut unknown_bootloader = base.clone();
        unknown_bootloader["bootloader"]["type"] = json!("lilo");
        parse_and_validate(unknown_bootloader).unwrap();

        // v1.1 requires osPackages metadata.
        let mut no_os_packages = base.clone();
        if let Some(obj) = no_os_packages.as_object_mut() {
            obj.remove("osPackages");
        }
        assert_validate_err_kind(
            no_os_packages,
            CosiMetadataErrorKind::V1_1OsPackagesRequired,
        );

        // v1.1 requires per-package release.
        let mut missing_release = base.clone();
        missing_release["osPackages"][0]
            .as_object_mut()
            .unwrap()
            .remove("release");
        assert_validate_err_kind(
            missing_release,
            CosiMetadataErrorKind::V1_1OsPackageMissingRelease("bash".to_string()),
        );

        // v1.1 requires per-package arch.
        let mut missing_arch = base.clone();
        missing_arch["osPackages"][0]
            .as_object_mut()
            .unwrap()
            .remove("arch");
        assert_validate_err_kind(
            missing_arch,
            CosiMetadataErrorKind::V1_1OsPackageMissingArch("bash".to_string()),
        );
    }
}
