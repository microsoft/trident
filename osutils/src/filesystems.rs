use std::fmt::{Display, Formatter, Result as FmtResult};

use anyhow::{bail, Error};

use sysdefs::filesystems::{KernelFilesystemType, NodevFilesystemType, RealFilesystemType};
use trident_api::config::FileSystemType;

/// File system types for `mount`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MountFileSystemType {
    Ext2,
    Ext3,
    Ext4,
    Xfs,
    Vfat,
    Iso9660,
    Tmpfs,
    Auto,
    Overlay,
    Ntfs,
}

/// File system types for `mkfs`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MkfsFileSystemType {
    Ext2,
    Ext3,
    Ext4,
    Xfs,
    Vfat,
    Ntfs,
}

/// File system types for fstab file
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabFileSystemType {
    Kernel(KernelFilesystemType),
    Auto,
    Swap,
}

impl MountFileSystemType {
    pub fn name(self) -> &'static str {
        match self {
            MountFileSystemType::Ext2 => "ext2",
            MountFileSystemType::Ext3 => "ext3",
            MountFileSystemType::Ext4 => "ext4",
            MountFileSystemType::Xfs => "xfs",
            MountFileSystemType::Vfat => "vfat",
            MountFileSystemType::Iso9660 => "iso9660",
            MountFileSystemType::Tmpfs => "tmpfs",
            MountFileSystemType::Auto => "auto",
            MountFileSystemType::Overlay => "overlay",
            MountFileSystemType::Ntfs => "ntfs",
        }
    }

    pub fn from_api_type(api_type: FileSystemType) -> Result<Self, anyhow::Error> {
        Ok(match api_type {
            FileSystemType::Auto => Self::Auto,
            FileSystemType::Ext4 => Self::Ext4,
            FileSystemType::Xfs => Self::Xfs,
            FileSystemType::Vfat => Self::Vfat,
            FileSystemType::Ntfs => Self::Ntfs,
            FileSystemType::Iso9660 => Self::Iso9660,
            FileSystemType::Tmpfs => Self::Tmpfs,
            FileSystemType::Overlay => Self::Overlay,
        })
    }
}

impl Display for MountFileSystemType {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.name())
    }
}

/// Provides a conversion from `MountFileSystemType` to `sys_mount::FilesystemType`
impl From<MountFileSystemType> for sys_mount::FilesystemType<'static> {
    fn from(s: MountFileSystemType) -> Self {
        sys_mount::FilesystemType::Manual(s.name())
    }
}

impl MkfsFileSystemType {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ext2 => "ext2",
            Self::Ext3 => "ext3",
            Self::Ext4 => "ext4",
            Self::Xfs => "xfs",
            Self::Vfat => "vfat",
            Self::Ntfs => "ntfs",
        }
    }
}

impl TryFrom<RealFilesystemType> for MkfsFileSystemType {
    type Error = Error;

    fn try_from(real_fs_type: RealFilesystemType) -> Result<Self, Error> {
        match real_fs_type {
            RealFilesystemType::Ext2 => Ok(Self::Ext2),
            RealFilesystemType::Ext3 => Ok(Self::Ext3),
            RealFilesystemType::Ext4 => Ok(Self::Ext4),
            RealFilesystemType::Ntfs => Ok(Self::Ntfs),
            RealFilesystemType::Vfat => Ok(Self::Vfat),
            RealFilesystemType::Xfs => Ok(Self::Xfs),
            RealFilesystemType::Btrfs
            | RealFilesystemType::Cramfs
            | RealFilesystemType::Exfat
            | RealFilesystemType::Fuseblk
            | RealFilesystemType::Iso9660
            | RealFilesystemType::Msdos
            | RealFilesystemType::Squashfs
            | RealFilesystemType::Udf => {
                bail!(
                    "'{}' filesystem type cannot be used for creating new filesystems",
                    real_fs_type.as_kernel().name()
                )
            }
        }
    }
}

impl Display for MkfsFileSystemType {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.name())
    }
}

/// Anything that can be turned into a `KernelFilesystemType` can be turned into
/// a `TabFileSystemType`.
impl<T> From<T> for TabFileSystemType
where
    T: Into<KernelFilesystemType>,
{
    fn from(fs: T) -> Self {
        Self::Kernel(fs.into())
    }
}

impl TabFileSystemType {
    // Returns the name of the filesystem type as consumed by fstab.
    pub fn name(&self) -> &str {
        match self {
            Self::Auto => "auto",
            Self::Swap => "swap",
            Self::Kernel(fs) => fs.name(),
        }
    }

    /// Converts a `FileSystemType` from the API into a `TabFileSystemType`.
    pub fn from_api_type(api_type: FileSystemType) -> Result<Self, Error> {
        Ok(match api_type {
            FileSystemType::Ext4 => RealFilesystemType::Ext4.into(),
            FileSystemType::Xfs => RealFilesystemType::Xfs.into(),
            FileSystemType::Vfat => RealFilesystemType::Vfat.into(),
            FileSystemType::Ntfs => RealFilesystemType::Ntfs.into(),
            FileSystemType::Iso9660 => RealFilesystemType::Iso9660.into(),
            FileSystemType::Tmpfs => NodevFilesystemType::Tmpfs.into(),
            FileSystemType::Overlay => NodevFilesystemType::Overlay.into(),
            FileSystemType::Auto => Self::Auto,
        })
    }
}
