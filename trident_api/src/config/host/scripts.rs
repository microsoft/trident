use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::status::ServicingType;

use super::error::InvalidHostConfigurationError;

/// Scripts that can be run on the host during Trident stages.
/// These scripts are run in the order they are defined.
/// Ensure that the scripts are idempotent as they may be run multiple times.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Scripts {
    /// Scripts to be run after Trident provision stage is complete.
    ///
    /// These scripts are run with the root filesystem of the target OS mounted at */mnt/newroot*
    /// and other partitions specified for the target OS mounted relative to that.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub post_provision: Vec<Script>,

    /// Scripts to be run after Trident configuration stage is complete.
    ///
    /// These scripts are run from within a chroot of the target OS.
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

    /// Selection of servicing types to run the script with.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub servicing_type_selection: Vec<ServicingTypeSelection>,

    /// Binary to run the script with. The default is `/bin/sh`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<PathBuf>,

    /// The contents of the script. Conflicts with path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,

    /// Path to the script file. Conflicts with content.
    ///
    /// The file must be located in the host's filesystem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,

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
    /// Returns true if servicing type is enabled for this script.
    pub fn should_run(&self, servicing_type: &ServicingType) -> bool {
        if self
            .servicing_type_selection
            .contains(&ServicingTypeSelection::All)
        {
            return true;
        }
        match servicing_type {
            ServicingType::CleanInstall => self
                .servicing_type_selection
                .contains(&ServicingTypeSelection::CleanInstall),
            ServicingType::NormalUpdate => self
                .servicing_type_selection
                .contains(&ServicingTypeSelection::NormalUpdate),
            ServicingType::AbUpdate => self
                .servicing_type_selection
                .contains(&ServicingTypeSelection::AbUpdate),
            ServicingType::UpdateAndReboot => self
                .servicing_type_selection
                .contains(&ServicingTypeSelection::UpdateAndReboot),
            _ => false,
        }
    }
}

/// The selection of servicing types performed by Trident that can be used for any user-facing API.
/// Currently, it is used to allow the user to select when to run a custom Script.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum ServicingTypeSelection {
    /// # Clean Install
    ///
    /// This selection only includes CleanInstall, a clean install of the runtime OS image when the
    /// host is booted from the provisioning OS.
    CleanInstall,

    /// # Normal Update
    ///
    /// This selection only includes NormalUpdate, an update that requires pausing the workload.
    NormalUpdate,

    /// # A/B Update
    ///
    /// This selection only includes AbUpdate, an update that requires switching to a different
    /// root partition and rebooting.
    AbUpdate,

    /// # Update and Reboot
    ///
    /// This selection only includes UpdateAndReboot, an update that requires rebooting the host.
    UpdateAndReboot,

    /// # All
    ///
    /// This selection includes all servicing types.
    All,
}

impl Scripts {
    pub(crate) fn validate(&self) -> Result<(), InvalidHostConfigurationError> {
        self.post_provision
            .iter()
            .chain(self.post_configure.iter())
            .try_for_each(|script| script.validate())?;
        Ok(())
    }
}

impl Script {
    pub(crate) fn validate(&self) -> Result<(), InvalidHostConfigurationError> {
        match (&self.content, &self.path) {
            (Some(_), Some(_)) => Err(InvalidHostConfigurationError::ScriptHasBothContentAndPath(
                self.name.clone(),
            )),
            (None, None) => Err(InvalidHostConfigurationError::ScriptHasNoContentOrPath(
                self.name.clone(),
            )),
            _ => Ok(()),
        }
    }
}

/// Unit Test for should_run
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_run_true() {
        let script = Script {
            name: "test-script".into(),
            servicing_type_selection: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: Some("echo test".into()),
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: None,
        };
        assert!(script.should_run(&ServicingType::CleanInstall));
    }

    #[test]
    fn test_should_run_false() {
        let script = Script {
            name: "test-script".into(),
            servicing_type_selection: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: Some("echo test".into()),
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: None,
        };
        assert!(!script.should_run(&ServicingType::NormalUpdate));
    }

    #[test]
    fn test_should_run_all() {
        let script = Script {
            name: "test-script".into(),
            servicing_type_selection: vec![ServicingTypeSelection::All],
            interpreter: Some("/bin/bash".into()),
            content: Some("echo test".into()),
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: None,
        };
        assert!(script.should_run(&ServicingType::AbUpdate));
    }
}
