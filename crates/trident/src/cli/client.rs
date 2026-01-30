use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    path::PathBuf,
};

use clap::{Args, Subcommand};

use super::{AllowedOperation, GetKind};

#[derive(Args, Debug)]
pub struct ClientArgs {
    /// The server address to connect to
    #[clap(short, long, default_value = "unix:///run/trident/trident.sock")]
    pub server: String,

    #[clap(subcommand)]
    pub command: ClientCommands,
}

#[derive(Subcommand, Debug)]
pub enum ClientCommands {
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

        /// Path to save the resulting Host Status
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

    /// Trigger manual rollback to previous state
    #[clap(name = "rollback")]
    Rollback {
        /// Check operation that would be performed
        #[arg(long)]
        check: bool,

        /// Invoke rollback only if next available rollback is runtime rollback.
        /// If allowed-operations is specified, this argument is only applicable for
        /// stage operation and will be ignored for finalize.
        #[arg(long, conflicts_with = "ab")]
        runtime: bool,

        /// Invoke available A/B rollback
        /// If allowed-operations is specified, this argument is only applicable for
        /// stage operation and will be ignored for finalize.
        #[arg(long, conflicts_with = "runtime")]
        ab: bool,

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

    StreamImage {
        /// URL of the image to stream
        #[clap(index = 1)]
        image: url::Url,

        /// Hash of the image manifest
        #[clap(long)]
        hash: String,

        /// Path to save the resulting Host Status
        #[clap(short, long)]
        status: Option<PathBuf>,

        /// Path to save an eventual fatal error
        #[clap(short, long)]
        error: Option<PathBuf>,
    },

    Version,
}

impl ClientCommands {
    pub fn name(&self) -> &'static str {
        // TODO: remove "client-" prefix once the old CLI is removed
        match self {
            Self::Install { .. } => "client-install",
            Self::Update { .. } => "client-update",
            Self::Commit { .. } => "client-commit",
            Self::Listen { .. } => "client-listen",
            Self::RebuildRaid { .. } => "client-rebuild-raid",
            Self::Rollback { .. } => "client-rollback",
            Self::StartNetwork { .. } => "client-start-network",
            Self::Get { .. } => "client-get",
            Self::Validate { .. } => "client-validate",
            Self::StreamImage { .. } => "client-stream-image",
            Self::Version => "client-version",
        }
    }
}

impl Display for ClientCommands {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.name())
    }
}
