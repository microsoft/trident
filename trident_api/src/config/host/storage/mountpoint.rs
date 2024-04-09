use std::{
    fmt::{Display, Formatter},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::BlockDeviceId;

#[cfg(feature = "schemars")]
use crate::schema_helpers::block_device_id_schema;

/// Mount point configuration.
///
/// These are used by Trident to update the `/etc/fstab` in the runtime OS to
/// correctly mount the volumes.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct MountPoint {
    /// The path of the mount point.
    ///
    /// This is the path where the volume will be mounted in the runtime OS.
    /// For `swap` partitions, the path should be `none`.
    pub path: PathBuf,

    /// The filesystem to be used for this mount point.
    ///
    /// This value will be used to format the partition.
    pub filesystem: FileSystemType,

    /// A list of options to be used for this mount point.
    ///
    /// These will be passed as is to the `/etc/fstab` file.
    pub options: Vec<String>,

    /// The id of the block device that will be mounted at this mount
    /// point.
    ///
    /// This parameter is required. It must be the ID of a disk partition,
    /// encrypted volume, software raid array, or a/b update volume pair.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub target_id: BlockDeviceId,
}

/// File system types.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum FileSystemType {
    /// # Ext4 file system
    Ext4,

    /// # XFS file system
    Xfs,

    /// # Vfat file system
    Vfat,

    /// # Swap partition
    Swap,

    /// # ISO9660 file system
    Iso9660,

    /// # Overlay file system
    Overlay,

    /// # Tmpfs
    ///
    /// [Kernel documentation](https://www.kernel.org/doc/html/latest/filesystems/tmpfs.html)
    Tmpfs,
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
            FileSystemType::Ext4 => true,
            FileSystemType::Xfs
            | FileSystemType::Vfat
            | FileSystemType::Swap
            | FileSystemType::Iso9660
            | FileSystemType::Overlay
            | FileSystemType::Tmpfs => false,
        }
    }
}
