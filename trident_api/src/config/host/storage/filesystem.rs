use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
    str::FromStr,
};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{is_default, BlockDeviceId};

use super::imaging::Image;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct FileSystem {
    /// The ID of the block device to associate with the file system.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<BlockDeviceId>,

    /// The file system type.
    #[serde(rename = "type")]
    pub fs_type: FileSystemType,

    /// The source of the file system.
    ///
    /// If not specified, a new filesystem will be created.
    ///
    /// When making a `swap` filesystem the field must be skipped.
    #[serde(default, skip_serializing_if = "is_default")]
    pub source: FileSystemSource,

    /// The mount point of the file system.
    ///
    /// It can be provided as an object for more control over the mount options,
    /// or as a just a string when `defaults` is sufficient.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::primitives::shortcuts::opt_string_or_struct"
    )]
    #[cfg_attr(
        feature = "schemars",
        schemars(
            schema_with = "crate::primitives::shortcuts::opt_string_or_struct_schema::<MountPoint>"
        )
    )]
    pub mount_point: Option<MountPoint>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct VerityFileSystem {
    /// Name of the verity device, used for the device mapper name
    pub name: String,

    /// The ID of the block device to associate with the file system.
    pub data_device_id: BlockDeviceId,

    /// The ID of the block device containing the hash.
    pub hash_device_id: BlockDeviceId,

    /// The image to use for the data device.
    pub data_image: Image,

    /// The image to use for the hash device.
    pub hash_image: Image,

    /// The file system type.
    #[serde(rename = "type")]
    pub fs_type: FileSystemType,

    /// The mount point of the file system.
    pub mount_point: MountPoint,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, tag = "type")]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum FileSystemSource {
    /// # Create
    ///
    /// Create a new file system.
    #[default]
    Create,

    /// # Image
    ///
    /// Use an existing file system from a partition image. **Cannot** be used
    /// for ESP/EFI partitions.
    Image(Image),

    /// # ESP Image
    ///
    /// Use an existing file system from an ESP image. Can **only** be used for
    /// ESP/EFI partitions.
    EspImage(Image),

    /// # Adopted
    ///
    /// Use an existing file system from an adopted partition.
    Adopted,

    /// # OS Image
    ///
    /// Not officially part of the API yet.
    #[cfg_attr(feature = "schemars", schemars(skip))]
    OsImage,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct MountPoint {
    /// The path to mount the file system.
    pub path: PathBuf,

    /// The mount options.
    #[serde(default)]
    pub options: MountOptions,
}

impl FromStr for MountPoint {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(MountPoint {
            path: PathBuf::from(s),
            options: MountOptions::defaults(),
        })
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(transparent)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct MountOptions(pub String);

impl MountOptions {
    pub fn new(options: impl Into<String>) -> Self {
        MountOptions(options.into())
    }

    pub fn defaults() -> Self {
        MountOptions("defaults".to_string())
    }

    pub fn empty() -> Self {
        MountOptions("".to_string())
    }

    pub fn contains(&self, option: impl AsRef<str>) -> bool {
        self.0.split(',').any(|o| o == option.as_ref())
    }

    pub fn str(&self) -> &str {
        &self.0
    }

    pub fn with(mut self, option: impl Into<String>) -> Self {
        self.append(option);
        self
    }

    pub fn append(&mut self, option: impl Into<String>) {
        if self.0.is_empty() {
            self.0 = option.into();
        } else {
            self.0.push(',');
            self.0.push_str(&option.into());
        }
    }

    pub fn to_str_vec(&self) -> Vec<&str> {
        self.0.split(',').collect()
    }

    pub fn to_string_vec(&self) -> Vec<String> {
        self.0.split(',').map(|s| s.to_string()).collect()
    }
}

impl Default for MountOptions {
    fn default() -> Self {
        MountOptions::defaults()
    }
}

/// File system types.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[cfg_attr(feature = "documentation", derive(strum_macros::EnumIter))]
pub enum FileSystemType {
    /// # Ext4 file system
    Ext4,

    /// # XFS file system
    Xfs,

    /// # Vfat file system
    Vfat,

    /// # NTFS file system
    ///
    /// Using NTFS on Linux comes with some limitations. For more information,
    /// see:
    /// [Limitations of NTFS](/docs/Explanation/Limitations-Of-NTFS.md)
    Ntfs,

    /// # Swap partition
    Swap,

    /// # Tmpfs
    ///
    /// [Kernel documentation](https://www.kernel.org/doc/html/latest/filesystems/tmpfs.html)
    Tmpfs,

    /// # Auto
    ///
    /// Passed to `mount` to automatically detect the filesystem type. ONLY
    /// supported for adopted partitions.
    Auto,

    /// # Other
    ///
    /// Used for any other arbitrary data from an image or for filesystems not
    /// supported by Trident or Linux.
    Other,

    /// # Overlay file system
    ///
    /// Used internally but currently not exposed in the API.
    ///
    /// Serialization is disabled. But deserialization is enabled for use in the
    /// Display trait implementation.
    #[serde(skip_deserializing)]
    #[cfg_attr(feature = "schemars", schemars(skip))]
    Overlay,

    /// # ISO9660 file system
    ///
    /// Used internally but currently not exposed in the API.
    ///
    /// Serialization is disabled. But deserialization is enabled for use in the
    /// Display trait implementation.
    #[serde(skip_deserializing)]
    #[cfg_attr(feature = "schemars", schemars(skip))]
    Iso9660,
}

impl Display for FileSystemType {
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

impl FileSystemType {
    /// Returns true if the file system is `ext*`.
    pub fn is_ext(&self) -> bool {
        // Added all on purpose (no wildcards) so that we update this when we
        // add new filesystem.
        match self {
            Self::Ext4 => true,
            Self::Xfs
            | Self::Vfat
            | Self::Ntfs
            | Self::Swap
            | Self::Tmpfs
            | Self::Overlay
            | Self::Iso9660
            | Self::Auto
            | Self::Other => false,
        }
    }

    /// Returns whether the filesystem should appear in the rules documentation.
    #[cfg(feature = "documentation")]
    pub fn document(&self) -> bool {
        !matches!(self, FileSystemType::Overlay | FileSystemType::Iso9660)
    }
}

/// Helper struct to communicate information about a mount point.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountPointInfo<'a> {
    pub mount_point: &'a MountPoint,
    pub fs_type: FileSystemType,
    pub is_verity: bool,
    pub device_id: Option<&'a BlockDeviceId>,
}

impl FileSystem {
    /// Because filesystems don't have IDs that can uniquely identify them, this
    /// function can be used to create a description of the specific filesystem
    /// in lieu of an ID.
    pub fn description(&self) -> String {
        [
            (
                "src",
                Some(
                    match &self.source {
                        FileSystemSource::Create => "new",
                        FileSystemSource::Adopted => "adopted",
                        FileSystemSource::Image(_) => "image",
                        FileSystemSource::EspImage(_) => "esp-image",
                        FileSystemSource::OsImage => "os-image",
                    }
                    .to_owned(),
                ),
            ),
            ("type", Some(self.fs_type.to_string())),
            ("dev", self.device_id.clone()),
            (
                "mnt",
                self.mount_point
                    .as_ref()
                    .map(|mp| mp.path.to_string_lossy().to_string()),
            ),
        ]
        .into_iter()
        .filter_map(|(k, v)| v.map(|v| format!("{}:{}", k, v)))
        .collect::<Vec<_>>()
        .join(", ")
    }
}

impl VerityFileSystem {
    /// Provide a quick description of the verity filesystem.
    pub fn description(&self) -> String {
        format!(
            "'{}' on devices data: '{}', hash: '{}'",
            self.name, self.data_device_id, self.hash_device_id
        )
    }
}

impl FileSystemSource {
    /// Returns the image associated with the filesystem source, if any.
    pub fn image(&self) -> Option<&Image> {
        match self {
            Self::Image(image) => Some(image),
            _ => None,
        }
    }

    /// Returns the ESP image associated with the filesystem source, if any.
    pub fn esp_image(&self) -> Option<&Image> {
        match self {
            Self::EspImage(image) => Some(image),
            _ => None,
        }
    }

    /// Returns whether the given filesystem source belongs to the old API.
    ///
    /// TODO: REMOVE WHEN THE OLD API IS REMOVED!!
    pub fn is_old_api(&self) -> bool {
        match self {
            FileSystemSource::Image(_) | FileSystemSource::EspImage(_) => true,
            FileSystemSource::Create | FileSystemSource::Adopted | FileSystemSource::OsImage => {
                false
            }
        }
    }
}
