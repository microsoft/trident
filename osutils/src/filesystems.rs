use anyhow::bail;
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
}

/// File system types for `mkfs`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MkfsFileSystemType {
    Ext2,
    Ext3,
    Ext4,
    Xfs,
    Vfat,
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
        }
    }

    pub fn from_api_type(api_type: FileSystemType) -> Result<Self, anyhow::Error> {
        Ok(match api_type {
            FileSystemType::Ext4 => MountFileSystemType::Ext4,
            FileSystemType::Xfs => MountFileSystemType::Xfs,
            FileSystemType::Vfat => MountFileSystemType::Vfat,
            FileSystemType::Iso9660 => MountFileSystemType::Iso9660,
            FileSystemType::Overlay => MountFileSystemType::Overlay,
            FileSystemType::Tmpfs => MountFileSystemType::Tmpfs,
            FileSystemType::Swap => {
                bail!("'swap' FS type cannot be used for mounting")
            }
        })
    }
}

impl std::fmt::Display for MountFileSystemType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
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
        }
    }

    pub fn from_api_type(api_type: FileSystemType) -> Result<Self, anyhow::Error> {
        Ok(match api_type {
            FileSystemType::Ext4 => Self::Ext4,
            FileSystemType::Xfs => Self::Xfs,
            FileSystemType::Vfat => Self::Vfat,
            FileSystemType::Swap
            | FileSystemType::Iso9660
            | FileSystemType::Overlay
            | FileSystemType::Tmpfs => {
                bail!(
                    "'{}' filesystem type cannot be used for creating new filesystems",
                    api_type
                )
            }
        })
    }
}

impl std::fmt::Display for MkfsFileSystemType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}
