use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    path::PathBuf,
};

use clap::{Parser, Subcommand};
use log::LevelFilter;
use trident_api::config::{Operation, Operations};

use crate::TRIDENT_VERSION;

#[derive(Parser, Debug)]
#[clap(version = TRIDENT_VERSION)]
pub struct Cli {
    /// Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
    #[arg(global = true, short, long, default_value_t = LevelFilter::Debug)]
    pub verbosity: LevelFilter,

    #[clap(subcommand)]
    pub command: Commands,
}

/// The operations that Trident is allowed to perform
#[derive(clap::ValueEnum, Clone, Debug, Eq, PartialEq)]
pub enum AllowedOperation {
    Stage,
    Finalize,
}

pub fn to_operations(allowed_operations: &[AllowedOperation]) -> Operations {
    let mut ops = Operations::empty();
    for op in allowed_operations {
        match op {
            AllowedOperation::Stage => ops.0.insert(Operation::Stage),
            AllowedOperation::Finalize => ops.0.insert(Operation::Finalize),
        };
    }
    ops
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initiate an install of Azure Linux
    Install {
        /// The new configuration to apply
        #[clap(index = 1, default_value = "/etc/trident/config.yaml")]
        config: PathBuf,

        /// Comma-separated list of operations that Trident will be allowed to perform
        #[clap(long, value_delimiter = ',', num_args = 0.., default_value = "stage,finalize")]
        allowed_operations: Vec<AllowedOperation>,

        /// Path to save the resulting Host Status
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,

        /// Allow Trident to perform a multiboot install
        #[clap(long)]
        multiboot: bool,
    },

    /// Start or continue an A/B update from an existing install
    Update {
        /// The new configuration to apply
        #[clap(index = 1, default_value = "/etc/trident/config.yaml")]
        config: PathBuf,

        /// Comma-separated list of operations that Trident will be allowed to perform
        #[clap(long, value_delimiter = ',', num_args = 0.., default_value = "stage,finalize")]
        allowed_operations: Vec<AllowedOperation>,

        /// Path to save the resulting Host Status
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,
    },

    /// Detect whether an install or update succeeded, and update the boot order accordingly
    Commit {
        /// Path to save the resulting Host Status
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,
    },

    #[clap(hide(true))]
    Listen {
        /// Path to save the resulting Host Status
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,
    },

    /// Rebuild software RAID arrays managed by Trident
    #[clap(name = "rebuild-raid")]
    RebuildRaid {
        /// The new configuration to work from
        #[clap(short, long)]
        config: Option<PathBuf>,

        /// Path to save the resulting HostStatus
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,
    },

    /// Configure OS networking based on Trident Configuration
    #[clap(name = "start-network", hide(true))]
    StartNetwork {
        /// The new configuration to apply
        #[clap(index = 1, default_value = "/etc/trident/config.yaml")]
        config: PathBuf,
    },

    /// Query the current state of the system
    #[clap(name = "get")]
    Get {
        /// What data to retrieve
        #[clap(default_value = "status")]
        kind: GetKind,

        /// Path to save the resulting output
        #[clap(short, long)]
        outfile: Option<PathBuf>,
    },

    /// Validate the provided Host Configuration
    ///
    /// When no options are provided, the default Trident Configuration is
    /// validated.
    Validate {
        /// Path to a Host Configuration file
        #[clap(index = 1, default_value = "/etc/trident/config.yaml")]
        config: PathBuf,
    },

    #[cfg(feature = "pytest-generator")]
    /// Generate Pytest wrappers for functional tests
    Pytest,

    /// Initialize for a system that wasn't installed by Trident
    OfflineInitialize {
        /// Path to a Host Status file (deprecated)
        ///
        /// If not provided, Trident will infer one based on the state of the system and history
        /// information left by Image Customizer.
        #[arg(conflicts_with = "lazy_partitions")]
        hs_path: Option<PathBuf>,
        /// Provide lazy partition information overrides for `-b` partitions
        ///
        /// This is a comma-separated list of `<b-partition-name>`:`<b-partition-partuuid>` pairs.
        #[arg(long, value_delimiter = ',', num_args = 0.., conflicts_with = "hs_path")]
        lazy_partitions: Vec<String>,
    },
}

impl Commands {
    pub fn name(&self) -> &'static str {
        match self {
            Commands::Install { .. } => "install",
            Commands::Update { .. } => "update",
            Commands::Commit { .. } => "commit",
            Commands::Listen { .. } => "listen",
            Commands::RebuildRaid { .. } => "rebuild-raid",
            Commands::StartNetwork { .. } => "start-network",
            Commands::Get { .. } => "get",
            Commands::Validate { .. } => "validate",
            #[cfg(feature = "pytest-generator")]
            Commands::Pytest => "pytest",
            Commands::OfflineInitialize { .. } => "offline-initialize",
        }
    }
}

impl Display for Commands {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.name())
    }
}

#[derive(clap::ValueEnum, Copy, Clone, Debug, Eq, PartialEq)]
pub enum GetKind {
    Configuration,
    Status,
    LastError,
}
