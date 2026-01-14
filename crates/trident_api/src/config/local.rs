use std::{collections::HashSet, path::PathBuf};

use maplit::hashset;
use serde::{Deserialize, Serialize};
use strum::IntoEnumIterator;
use strum_macros::EnumIter;

use super::host::HostConfiguration;

/// HostConfigurationSource is the source of the Host Configuration.
/// Used internally by Trident.
#[derive(Debug)]
pub enum HostConfigurationSource {
    File(PathBuf),
    RawString(String),
    Embedded(Box<HostConfiguration>),
}

impl std::fmt::Display for HostConfigurationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostConfigurationSource::File(path) => write!(f, "file: {}", path.display()),
            HostConfigurationSource::RawString(_) => write!(f, "raw-string"),
            HostConfigurationSource::Embedded(_) => write!(f, "embedded"),
        }
    }
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
    pub fn all() -> Self {
        Self(hashset![Operation::Stage, Operation::Finalize])
    }

    pub fn empty() -> Self {
        Self(HashSet::new())
    }

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

impl From<Operation> for Operations {
    fn from(op: Operation) -> Self {
        let mut set = HashSet::new();
        set.insert(op);
        Operations(set)
    }
}
