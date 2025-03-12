use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KnownFilesystemType {
    Ext4,
    Ext3,
    Ext2,
    Cramfs,
    Squashfs,
    Vfat,
    Msdos,
    Exfat,
    Iso9660,
    Ntfs,
    Btrfs,
    Xfs,
    Tmpfs,
    Swap,
    Overlay,
    #[serde(untagged)]
    Other(String),
}
