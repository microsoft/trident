use std::{collections::HashSet, path::PathBuf};

use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

use super::host::HostConfiguration;

/// HostConfigurationSource is the source of the host configuration.
/// Used internally by Trident.
#[derive(Debug)]
pub enum HostConfigurationSource {
    File(PathBuf),
    Embedded(Box<HostConfiguration>),
    #[cfg(feature = "setsail")]
    KickstartFile(PathBuf),
    #[cfg(feature = "setsail")]
    KickstartEmbedded(String),
}

impl std::fmt::Display for HostConfigurationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostConfigurationSource::File(path) => write!(f, "file: {}", path.display()),
            HostConfigurationSource::Embedded(_) => write!(f, "embedded"),
            #[cfg(feature = "setsail")]
            HostConfigurationSource::KickstartFile(path) => {
                write!(f, "kickstart file: {}", path.display())
            }
            #[cfg(feature = "setsail")]
            HostConfigurationSource::KickstartEmbedded(_) => write!(f, "kickstart embedded"),
        }
    }
}

/// GrpcConfiguration is the configuration for the gRPC server.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrpcConfiguration {
    /// Port for the gRPC server (defaults to 50051 if not set).
    pub listen_port: Option<u16>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case", transparent)]
pub struct Operations(pub HashSet<Operation>);

impl Default for Operations {
    fn default() -> Self {
        Self(Operation::iter().collect())
    }
}

impl Operations {
    pub fn contains(&self, op: Operation) -> bool {
        self.0.contains(&op)
    }

    pub fn has_finalize(&self) -> bool {
        self.contains(Operation::Finalize)
    }

    pub fn has_stage(&self) -> bool {
        self.contains(Operation::Stage)
    }
}

#[derive(
    Serialize, Deserialize, Debug, Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, EnumIter,
)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Operation {
    Stage,
    Finalize,
}
