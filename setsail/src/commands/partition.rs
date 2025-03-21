use std::path::PathBuf;

use clap::{ArgGroup, Parser, ValueEnum};

use crate::{data::ParsedData, types::KSLine, SetsailError};

use super::HandleCommand;

#[derive(Parser, Debug)]
#[command(name = "partition", aliases = &["part"], help_expected = true)]
#[clap(group(ArgGroup::new("sizing").required(true)))]
pub struct Partition {
    #[clap(skip)]
    pub line: KSLine,

    /// The mountpoint of the partition
    ///
    /// Format: String
    ///
    /// Accepted values:
    ///
    /// - A path to a directory (e.g. `/`, `/boot`, `/home`)
    /// - `swap`
    #[arg(verbatim_doc_comment)]
    pub mntpoint: PartitionMount,

    /// The filesystem type of the partition
    ///
    /// Format: String
    #[arg(long)]
    #[clap(default_value = "ext4")]
    pub fstype: FsType,

    /// Minimum size of the partition in MiB
    ///
    /// Format: u64
    #[arg(long, group = "sizing")]
    pub size: Option<u64>,

    /// Whether the partition should grow to fill the available space
    #[arg(long, group = "sizing")]
    pub grow: bool,

    // /// Maximum size of the partition in MiB
    // ///
    // /// Requires: `grow`
    // ///
    // /// Format: u64
    // Disabled: Having this flag leads to unexpected behavior,
    // we are keeping it unsupported for now.
    // #[arg(long)]
    // #[arg(requires = "grow")]
    // pub maxsize: Option<u64>,
    /// The target disk for this partition
    ///
    /// Format: String
    ///
    /// Default: `/dev/sda`
    ///
    /// Examples: `sda`, `/dev/sdb`
    #[arg(long)]
    #[arg(alias = "ondrive")]
    pub ondisk: Option<String>,

    /// Label to add to the partition
    ///
    /// Format: String
    #[arg(long)]
    pub label: Option<String>,

    /// Options to be used when mounting the filesystem
    ///
    /// These are passed to /etc/fstab
    ///
    /// Format: Comma-delimited list of strings
    #[arg(long)]
    #[arg(value_delimiter = ',')]
    pub fsoptions: Vec<String>,

    /// Optional URL of an image to write to this partition
    ///
    /// Format: String
    ///
    /// Examples:
    /// - `file:///rootfs.raw.zst`
    /// - `http://example.com/image.img`
    #[arg(verbatim_doc_comment)]
    #[arg(long)]
    #[arg(value_delimiter = ',')]
    pub image: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartitionMount {
    Path(PathBuf),
    Swap,
    Raid(String),
    Pv(String),
    Btrfs(String),
    BiosBoot,
}

impl std::str::FromStr for PartitionMount {
    type Err = Box<dyn std::error::Error + Send + Sync>;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "swap" => Ok(Self::Swap),
            "biosboot" => Ok(Self::BiosBoot),
            s if s.starts_with('/') => Ok(Self::Path(PathBuf::from_str(s)?)),
            s if s.starts_with("raid.") => Ok(Self::Raid(
                s.split('.').nth(1).unwrap_or_default().to_string(),
            )),
            s if s.starts_with("pv.") => Ok(Self::Pv(
                s.split('.').nth(1).unwrap_or_default().to_string(),
            )),
            s if s.starts_with("btrfs.") => Ok(Self::Btrfs(
                s.split('.').nth(1).unwrap_or_default().to_string(),
            )),
            _ => Err("Provided mountpoint does not match any known type".into()),
        }
    }
}

impl std::fmt::Display for PartitionMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartitionMount::Path(path) => write!(f, "{}", path.display()),
            PartitionMount::Swap => write!(f, "swap"),
            PartitionMount::Raid(raid) => write!(f, "raid.{}", raid),
            PartitionMount::Pv(pv) => write!(f, "pv.{}", pv),
            PartitionMount::Btrfs(btrfs) => write!(f, "btrfs.{}", btrfs),
            PartitionMount::BiosBoot => write!(f, "biosboot"),
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
#[clap(rename_all = "kebab_case")]
pub enum FsType {
    /// Ext4 filesystem
    Ext4,
    /// (UNSUPPORTED) Ext3 filesystem
    // Ext3,
    /// (UNSUPPORTED) Ext2 filesystem
    // Ext2,
    /// (UNSUPPORTED) XFS filesystem
    // Xfs,
    /// Swap partition
    Swap,
    /// Vfat filesystem
    Vfat,
    /// EFI filesystem
    Efi,
}

impl std::fmt::Display for FsType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format!("{:?}", self).to_lowercase())
    }
}

impl HandleCommand for Partition {
    fn handle(mut self, line: KSLine, data: &mut ParsedData) -> Result<(), SetsailError> {
        let mut result = Ok(());
        if data
            .partitions
            .iter()
            .filter(|p| p.mntpoint == self.mntpoint)
            .count()
            > 0
        {
            // Kickstart precedence works by "last keyword wins", so we need to remove any existing
            // partition with the same mountpoint before adding this one.
            data.partitions.retain(|p| p.mntpoint != self.mntpoint);
            result = Err(SetsailError::new_sem_warn(
                line.clone(),
                format!(
                    "Overriding partition with matching mountpoint: {}",
                    self.mntpoint
                ),
            ));
        }

        // Images only make sense for partitions mounted to a path
        if self.image.is_some() && !matches!(self.mntpoint, PartitionMount::Path(_)) {
            return Err(SetsailError::new_semantic(
                line,
                format!(
                    "Images can only be added to partitions mounted to a path: {}",
                    self.mntpoint
                ),
            ));
        }

        // Disabled: see note on `--maxsize` above
        // If size and maxsize are both specified,
        // Ensure that size <= maxsize
        // if self.size.is_some()
        //     && self.maxsize.is_some()
        //     && self.size.unwrap() > self.maxsize.unwrap()
        // {
        //     return Err(SetsailError::new_semantic(
        //         line.clone(),
        //         format!(
        //             "maxsize ({}) must be greater than size ({})",
        //             self.maxsize.unwrap(),
        //             self.size.unwrap()
        //         ),
        //     ));
        // }

        self.line = line;
        data.partitions.push(self);

        result
    }
}
