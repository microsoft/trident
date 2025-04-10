#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use serde::{de::Error, Deserialize, Serialize};
use strum_macros::{EnumIter, IntoStaticStr};
use sysdefs::filesystems::{KernelFilesystemType, NodevFilesystemType, RealFilesystemType};

use crate::error::{InternalError, TridentError};

/// New file system types.
#[derive(
    Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq, EnumIter, IntoStaticStr,
)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[strum(serialize_all = "lowercase")]
pub enum NewFileSystemType {
    /// # Ext4 file system
    #[default]
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

    /// # Tmpfs
    ///
    /// [Kernel documentation](https://www.kernel.org/doc/html/latest/filesystems/tmpfs.html)
    Tmpfs,

    /// # Overlay file system
    ///
    /// Used internally but currently not exposed in the API.
    ///
    /// Serialization is disabled. But deserialization is enabled for use in the
    /// Display trait implementation.
    #[serde(skip_deserializing)]
    #[cfg_attr(feature = "schemars", schemars(skip))]
    Overlay,
}

impl TryFrom<&str> for NewFileSystemType {
    type Error = serde::de::value::Error;

    fn try_from(fs: &str) -> Result<Self, Self::Error> {
        match fs {
            "ext4" => Ok(NewFileSystemType::Ext4),
            "xfs" => Ok(NewFileSystemType::Xfs),
            "vfat" => Ok(NewFileSystemType::Vfat),
            "ntfs" => Ok(NewFileSystemType::Ntfs),
            "tmpfs" => Ok(NewFileSystemType::Tmpfs),
            "overlay" => Ok(NewFileSystemType::Overlay),
            _ => Err(serde::de::value::Error::custom(format!(
                "Invalid new filesystem type: '{fs}'",
            ))),
        }
    }
}

impl TryFrom<String> for NewFileSystemType {
    type Error = serde::de::value::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.as_str().try_into()
    }
}

impl From<NewFileSystemType> for String {
    fn from(val: NewFileSystemType) -> Self {
        let intermediary_str: &'static str = val.into();
        intermediary_str.to_string()
    }
}

impl TryFrom<NewFileSystemType> for RealFilesystemType {
    type Error = TridentError;

    fn try_from(value: NewFileSystemType) -> Result<RealFilesystemType, TridentError> {
        match value {
            NewFileSystemType::Ext4 => Ok(RealFilesystemType::Ext4),
            NewFileSystemType::Xfs => Ok(RealFilesystemType::Xfs),
            NewFileSystemType::Vfat => Ok(RealFilesystemType::Vfat),
            NewFileSystemType::Ntfs => Ok(RealFilesystemType::Ntfs),
            NewFileSystemType::Tmpfs => Err(TridentError::new(InternalError::Internal(
                "Cannot convert Tmpfs to a RealFilesystemType",
            ))),
            NewFileSystemType::Overlay => Err(TridentError::new(InternalError::Internal(
                "Cannot convert Overlay to a RealFilesystemType",
            ))),
        }
    }
}

impl From<NewFileSystemType> for KernelFilesystemType {
    fn from(value: NewFileSystemType) -> KernelFilesystemType {
        match value {
            NewFileSystemType::Ext4 => KernelFilesystemType::Real(RealFilesystemType::Ext4),
            NewFileSystemType::Xfs => KernelFilesystemType::Real(RealFilesystemType::Xfs),
            NewFileSystemType::Vfat => KernelFilesystemType::Real(RealFilesystemType::Vfat),
            NewFileSystemType::Ntfs => KernelFilesystemType::Real(RealFilesystemType::Ntfs),
            NewFileSystemType::Tmpfs => KernelFilesystemType::Nodev(NodevFilesystemType::Tmpfs),
            NewFileSystemType::Overlay => KernelFilesystemType::Nodev(NodevFilesystemType::Overlay),
        }
    }
}

/// Adopted file system types.
#[derive(
    Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq, EnumIter, IntoStaticStr,
)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[strum(serialize_all = "lowercase")]
pub enum AdoptedFileSystemType {
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

    /// # Auto
    ///
    /// Passed to `mount` to automatically detect the filesystem type. ONLY
    /// supported for adopted partitions.
    #[default]
    Auto,

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

impl From<AdoptedFileSystemType> for String {
    fn from(val: AdoptedFileSystemType) -> Self {
        let intermediary_str: &'static str = val.into();
        intermediary_str.to_string()
    }
}

impl TryFrom<&str> for AdoptedFileSystemType {
    type Error = serde::de::value::Error;

    fn try_from(fs: &str) -> Result<Self, Self::Error> {
        match fs {
            "ext4" => Ok(AdoptedFileSystemType::Ext4),
            "xfs" => Ok(AdoptedFileSystemType::Xfs),
            "vfat" => Ok(AdoptedFileSystemType::Vfat),
            "ntfs" => Ok(AdoptedFileSystemType::Ntfs),
            "auto" => Ok(AdoptedFileSystemType::Auto),
            "iso9660" => Ok(AdoptedFileSystemType::Iso9660),
            _ => Err(serde::de::value::Error::custom(format!(
                "Invalid adopted filesystem type: '{fs}'"
            ))),
        }
    }
}

impl TryFrom<String> for AdoptedFileSystemType {
    type Error = serde::de::value::Error;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value.as_str().try_into()
    }
}

impl TryFrom<AdoptedFileSystemType> for RealFilesystemType {
    type Error = TridentError;

    fn try_from(value: AdoptedFileSystemType) -> Result<RealFilesystemType, TridentError> {
        match value {
            AdoptedFileSystemType::Ext4 => Ok(RealFilesystemType::Ext4),
            AdoptedFileSystemType::Xfs => Ok(RealFilesystemType::Xfs),
            AdoptedFileSystemType::Vfat => Ok(RealFilesystemType::Vfat),
            AdoptedFileSystemType::Ntfs => Ok(RealFilesystemType::Ntfs),
            AdoptedFileSystemType::Iso9660 => Ok(RealFilesystemType::Iso9660),
            AdoptedFileSystemType::Auto => Err(TridentError::new(InternalError::Internal(
                "Cannot convert Auto to a RealFilesystemType",
            ))),
        }
    }
}

impl TryFrom<AdoptedFileSystemType> for KernelFilesystemType {
    type Error = TridentError;

    fn try_from(value: AdoptedFileSystemType) -> Result<KernelFilesystemType, TridentError> {
        Ok(RealFilesystemType::try_from(value)?.as_kernel())
    }
}
