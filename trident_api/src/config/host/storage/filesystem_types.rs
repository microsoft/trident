use std::fmt::{Display, Formatter};

use anyhow::bail;
#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use serde::{de::Error, Deserialize, Serialize};
use strum_macros::{EnumIter, IntoStaticStr};
use sysdefs::filesystems::{KernelFilesystemType, NodevFilesystemType, RealFilesystemType};

use crate::error::{InternalError, TridentError};

/// File system types.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, EnumIter)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
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

    /// # Tmpfs
    ///
    /// [Kernel documentation](https://www.kernel.org/doc/html/latest/filesystems/tmpfs.html)
    ///
    /// Tmpfs is only valid if the filesystem `source` is `new`.
    Tmpfs,

    /// # Auto
    ///
    /// Passed to `mount` to automatically detect the filesystem type.
    ///
    /// Auto is only valid if the filesystem `source` is `adopted`.
    Auto,

    /// # Overlay file system
    ///
    /// Used internally but currently not exposed in the API.
    ///
    /// Serialization is disabled. But deserialization is enabled for use in the
    /// Display trait implementation.
    #[cfg_attr(feature = "schemars", schemars(skip))]
    Overlay,

    /// # ISO9660 file system
    ///
    /// Used internally but currently not exposed in the API.
    ///
    /// Serialization is disabled. But deserialization is enabled for use in the
    /// Display trait implementation.
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

impl From<NewFileSystemType> for FileSystemType {
    fn from(value: NewFileSystemType) -> Self {
        match value {
            NewFileSystemType::Ext4 => FileSystemType::Ext4,
            NewFileSystemType::Xfs => FileSystemType::Xfs,
            NewFileSystemType::Vfat => FileSystemType::Vfat,
            NewFileSystemType::Ntfs => FileSystemType::Ntfs,
            NewFileSystemType::Tmpfs => FileSystemType::Tmpfs,
            NewFileSystemType::Overlay => FileSystemType::Overlay,
        }
    }
}

impl From<AdoptedFileSystemType> for FileSystemType {
    fn from(value: AdoptedFileSystemType) -> Self {
        match value {
            AdoptedFileSystemType::Ext4 => FileSystemType::Ext4,
            AdoptedFileSystemType::Xfs => FileSystemType::Xfs,
            AdoptedFileSystemType::Vfat => FileSystemType::Vfat,
            AdoptedFileSystemType::Ntfs => FileSystemType::Ntfs,
            AdoptedFileSystemType::Auto => FileSystemType::Auto,
            AdoptedFileSystemType::Iso9660 => FileSystemType::Iso9660,
        }
    }
}

impl TryFrom<FileSystemType> for NewFileSystemType {
    type Error = anyhow::Error;

    fn try_from(value: FileSystemType) -> Result<Self, Self::Error> {
        match value {
            FileSystemType::Ext4 => Ok(NewFileSystemType::Ext4),
            FileSystemType::Xfs => Ok(NewFileSystemType::Xfs),
            FileSystemType::Vfat => Ok(NewFileSystemType::Vfat),
            FileSystemType::Ntfs => Ok(NewFileSystemType::Ntfs),
            FileSystemType::Tmpfs => Ok(NewFileSystemType::Tmpfs),
            FileSystemType::Overlay => Ok(NewFileSystemType::Overlay),
            FileSystemType::Auto | FileSystemType::Iso9660 => {
                bail!("'{value}' is not a valid new filesystem type")
            }
        }
    }
}

impl TryFrom<FileSystemType> for AdoptedFileSystemType {
    type Error = anyhow::Error;

    fn try_from(value: FileSystemType) -> Result<Self, Self::Error> {
        match value {
            FileSystemType::Ext4 => Ok(AdoptedFileSystemType::Ext4),
            FileSystemType::Xfs => Ok(AdoptedFileSystemType::Xfs),
            FileSystemType::Vfat => Ok(AdoptedFileSystemType::Vfat),
            FileSystemType::Ntfs => Ok(AdoptedFileSystemType::Ntfs),
            FileSystemType::Auto => Ok(AdoptedFileSystemType::Auto),
            FileSystemType::Iso9660 => Ok(AdoptedFileSystemType::Iso9660),
            FileSystemType::Tmpfs | FileSystemType::Overlay => {
                bail!("'{value}' is not a valid adopted filesystem type")
            }
        }
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
            | Self::Tmpfs
            | Self::Overlay
            | Self::Iso9660
            | Self::Auto => false,
        }
    }

    /// Returns whether the filesystem should appear in the rules documentation.
    #[cfg(feature = "documentation")]
    pub fn document(&self) -> bool {
        !matches!(self, FileSystemType::Overlay | FileSystemType::Iso9660)
    }
}

/// New file system types.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, EnumIter, IntoStaticStr)]
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
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, EnumIter, IntoStaticStr)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_convert_newfstype() {
        // Text to NewFileSystemType: Success
        assert_eq!(
            NewFileSystemType::try_from("ext4").unwrap(),
            NewFileSystemType::Ext4
        );
        assert_eq!(
            NewFileSystemType::try_from("tmpfs").unwrap(),
            NewFileSystemType::Tmpfs
        );
        assert_eq!(
            NewFileSystemType::try_from("xfs").unwrap(),
            NewFileSystemType::Xfs
        );
        assert_eq!(
            NewFileSystemType::try_from(String::from("overlay")).unwrap(),
            NewFileSystemType::Overlay
        );
        assert_eq!(
            NewFileSystemType::try_from(String::from("vfat")).unwrap(),
            NewFileSystemType::Vfat
        );

        // NewFileSystemType to text: Success
        assert_eq!(String::from(NewFileSystemType::Ext4), "ext4".to_string());

        // Text to NewFileSystemType: Failure case - improper capitalization
        assert!(NewFileSystemType::try_from("Ext4")
            .unwrap_err()
            .to_string()
            .contains("Invalid new filesystem type"));

        // Text to NewFileSystemType: Failure case - nonexistent variant
        assert!(NewFileSystemType::try_from("iso9660")
            .unwrap_err()
            .to_string()
            .contains("Invalid new filesystem type"));
    }

    #[test]
    fn test_text_convert_adoptedfstype() {
        // Text to AdoptedFileSystemType: Success
        assert_eq!(
            AdoptedFileSystemType::try_from("ext4").unwrap(),
            AdoptedFileSystemType::Ext4
        );
        assert_eq!(
            AdoptedFileSystemType::try_from("iso9660").unwrap(),
            AdoptedFileSystemType::Iso9660
        );
        assert_eq!(
            AdoptedFileSystemType::try_from("xfs").unwrap(),
            AdoptedFileSystemType::Xfs
        );
        assert_eq!(
            AdoptedFileSystemType::try_from(String::from("auto")).unwrap(),
            AdoptedFileSystemType::Auto
        );
        assert_eq!(
            AdoptedFileSystemType::try_from(String::from("vfat")).unwrap(),
            AdoptedFileSystemType::Vfat
        );

        // AdoptedFileSystemType to text: Success
        assert_eq!(
            String::from(AdoptedFileSystemType::Ext4),
            "ext4".to_string()
        );

        // Text to AdoptedFileSystemType: Failure case - improper capitalization
        assert!(AdoptedFileSystemType::try_from("Ext4")
            .unwrap_err()
            .to_string()
            .contains("Invalid adopted filesystem type"));

        // Text to AdoptedFileSystemType: Failure case - nonexistent variant
        assert!(AdoptedFileSystemType::try_from("tmpfs")
            .unwrap_err()
            .to_string()
            .contains("Invalid adopted filesystem type"));
    }

    #[test]
    fn test_convert_to_realfstype() {
        // NewFileSystemType
        assert_eq!(
            RealFilesystemType::try_from(NewFileSystemType::Ext4).unwrap(),
            RealFilesystemType::Ext4
        );
        assert_eq!(
            RealFilesystemType::try_from(NewFileSystemType::Vfat).unwrap(),
            RealFilesystemType::Vfat
        );
        RealFilesystemType::try_from(NewFileSystemType::Tmpfs).unwrap_err();

        // AdoptedFileSystemType
        assert_eq!(
            RealFilesystemType::try_from(AdoptedFileSystemType::Ext4).unwrap(),
            RealFilesystemType::Ext4
        );
        assert_eq!(
            RealFilesystemType::try_from(AdoptedFileSystemType::Vfat).unwrap(),
            RealFilesystemType::Vfat
        );
        RealFilesystemType::try_from(AdoptedFileSystemType::Auto).unwrap_err();
    }

    #[test]
    fn test_convert_to_kernelfstype() {
        // NewFileSystemType
        assert_eq!(
            KernelFilesystemType::from(NewFileSystemType::Ext4),
            KernelFilesystemType::Real(RealFilesystemType::Ext4)
        );
        assert_eq!(
            KernelFilesystemType::from(NewFileSystemType::Tmpfs),
            KernelFilesystemType::Nodev(NodevFilesystemType::Tmpfs)
        );

        // AdoptedFileSystemType
        assert_eq!(
            KernelFilesystemType::try_from(AdoptedFileSystemType::Xfs).unwrap(),
            KernelFilesystemType::Real(RealFilesystemType::Xfs)
        );
        assert_eq!(
            KernelFilesystemType::try_from(AdoptedFileSystemType::Iso9660).unwrap(),
            KernelFilesystemType::Real(RealFilesystemType::Iso9660)
        );
        KernelFilesystemType::try_from(AdoptedFileSystemType::Auto).unwrap_err();
    }
}
