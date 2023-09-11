use clap::{Parser, ValueEnum};

use crate::{parser::ParsedData, types::KSLine, SetsailError};

use super::CommandHandler;

#[derive(Parser, Debug)]
pub struct Partition {
    #[clap(skip)]
    pub line: KSLine,

    pub mntpoint: PartitionMount,

    #[arg(long)]
    pub asprimary: bool,

    #[arg(long)]
    pub fstype: Option<FsType>,

    #[arg(long)]
    pub size: Option<u64>,

    #[arg(long)]
    pub grow: bool,

    #[arg(long)]
    pub ondisk: Option<String>,

    #[arg(long)]
    pub label: Option<String>,

    #[arg(long)]
    pub fsoptions: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PartitionMount {
    Path(String),
    Swap,
    Raid(String),
    Pv(String),
    Btrfs(String),
    BiosBoot,
}

impl std::str::FromStr for PartitionMount {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "swap" => Ok(Self::Swap),
            "biosboot" => Ok(Self::BiosBoot),
            s if s.starts_with('/') => Ok(Self::Path(s.to_string())),
            s if s.starts_with("raid.") => Ok(Self::Raid(
                s.split('.').nth(1).unwrap_or_default().to_string(),
            )),
            s if s.starts_with("pv.") => Ok(Self::Pv(
                s.split('.').nth(1).unwrap_or_default().to_string(),
            )),
            s if s.starts_with("btrfs.") => Ok(Self::Btrfs(
                s.split('.').nth(1).unwrap_or_default().to_string(),
            )),
            _ => Err("Provided mountpoint does not match any known type".to_string()),
        }
    }
}

impl std::fmt::Display for PartitionMount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartitionMount::Path(path) => write!(f, "{}", path),
            PartitionMount::Swap => write!(f, "swap"),
            PartitionMount::Raid(raid) => write!(f, "raid.{}", raid),
            PartitionMount::Pv(pv) => write!(f, "pv.{}", pv),
            PartitionMount::Btrfs(btrfs) => write!(f, "btrfs.{}", btrfs),
            PartitionMount::BiosBoot => write!(f, "biosboot"),
        }
    }
}

#[derive(Debug, Clone, ValueEnum)]
#[clap(rename_all = "kebab_case")]
pub enum FsType {
    Ext4,
    Ext3,
    Ext2,
    Xfs,
    Swap,
    Vfat,
    Efi,
}

impl std::fmt::Display for FsType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", format!("{:?}", self).to_lowercase())
    }
}

impl CommandHandler for Partition {
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

        self.line = line;
        data.partitions.push(self);

        result
    }
}
