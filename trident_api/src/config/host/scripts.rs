use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::status::{ReconcileState, UpdateKind};

/// Scripts that can be run on the host during Trident stages.
/// These scripts are run in the order they are defined.
/// Ensure that the scripts are idempotent as they may be run multiple times.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Scripts {
    /// Scripts to be run after Trident provision stage is complete.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub post_provision: Vec<Script>,

    /// Scripts to be run after Trident configuration stage is complete.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub post_configure: Vec<Script>,
}

/// A script that can be run on the host during Trident stages.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Script {
    /// Name of the script.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,

    /// List of servicing_type to run the script with.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub servicing_type: Vec<ServicingType>,

    /// Binary to run the script with. The default is `/bin/sh`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<PathBuf>,

    /// The contents of the script.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,

    /// Path of a file to write the script's output to.
    ///
    /// This includes both stdout and stderr. The path and file
    /// will be created if they don't exist. If the file already
    /// exists, it will be truncated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_file_path: Option<PathBuf>,

    /// Environment variables that are needed by the script.
    /// These will be set before running the script.
    /// UPDATE_KIND and TARGET_ROOT values are set by default to use.
    #[serde(default)]
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub environment_variables: HashMap<String, String>,
}

impl Script {
    /// Returns true if reconcile state is enabled for this script.
    pub fn should_run(&self, reconcile_state: &ReconcileState) -> bool {
        if self.servicing_type.contains(&ServicingType::All) {
            return true;
        }
        match reconcile_state {
            ReconcileState::CleanInstall => {
                self.servicing_type.contains(&ServicingType::CleanInstall)
            }
            ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate) => {
                self.servicing_type.contains(&ServicingType::NormalUpdate)
            }
            ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
                self.servicing_type.contains(&ServicingType::AbUpdate)
            }
            ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot) => self
                .servicing_type
                .contains(&ServicingType::UpdateAndReboot),
            _ => false,
        }
    }
}

/// The type of servicing performed by Trident that a script should be run for.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum ServicingType {
    CleanInstall,
    NormalUpdate,
    AbUpdate,
    UpdateAndReboot,
    All,
}

/// Unit Test for should_run
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_run_true() {
        let script = Script {
            name: "test-script".into(),
            servicing_type: vec![ServicingType::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: "echo test".into(),
            environment_variables: HashMap::new(),
            log_file_path: None,
        };
        assert!(script.should_run(&ReconcileState::CleanInstall));
    }

    #[test]
    fn test_should_run_false() {
        let script = Script {
            name: "test-script".into(),
            servicing_type: vec![ServicingType::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: "echo test".into(),
            environment_variables: HashMap::new(),
            log_file_path: None,
        };
        assert!(!script.should_run(&ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate)));
    }

    #[test]
    fn test_should_run_all() {
        let script = Script {
            name: "test-script".into(),
            servicing_type: vec![ServicingType::All],
            interpreter: Some("/bin/bash".into()),
            content: "echo test".into(),
            environment_variables: HashMap::new(),
            log_file_path: None,
        };
        assert!(script.should_run(&ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate)));
    }
}
