use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::status::ServicingType;

use super::error::HostConfigurationStaticValidationError;

/// Scripts that can be run on the host during Trident stages.
/// These scripts are run in the order they are defined.
/// Ensure that the scripts are idempotent as they may be run multiple times.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Scripts {
    /// Scripts to be run after step 'Provision' in Trident is complete.
    ///
    /// These scripts are run with the root filesystem of the target OS mounted at *$TARGET_ROOT*
    /// and other partitions specified for the target OS mounted relative to that. The *$EXEC_ROOT*
    /// variable wil be set to '/' for consistency with post configure scripts.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub post_provision: Vec<Script>,

    /// Scripts to be run after step 'Configure' in Trident is complete.
    ///
    /// These scripts are run from within a chroot of the target OS. The *$TARGET_ROOT* variable
    /// will be set to '/'. The *$EXEC_ROOT* variable will be set to the root of the filesystem
    /// Trident is being run from (or more specifically a directory within /tmp that is bind mounted
    /// to the root).
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

    /// List of servicing types that the script should run on.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub run_on: Vec<ServicingTypeSelection>,

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
    pub fn should_run(&self, servicing_type: ServicingType) -> bool {
        if self.run_on.contains(&ServicingTypeSelection::All) {
            return true;
        }
        match servicing_type {
            ServicingType::CleanInstall => {
                self.run_on.contains(&ServicingTypeSelection::CleanInstall)
            }
            ServicingType::NormalUpdate => {
                self.run_on.contains(&ServicingTypeSelection::NormalUpdate)
            }
            ServicingType::AbUpdate => self.run_on.contains(&ServicingTypeSelection::AbUpdate),
            ServicingType::UpdateAndReboot => self
                .run_on
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
    pub(crate) fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        self.post_provision
            .iter()
            .chain(self.post_configure.iter())
            .try_for_each(|script| script.validate())?;
        Ok(())
    }
}

impl Script {
    pub(crate) fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        match (&self.content, &self.path) {
            (Some(_), Some(_)) => Err(
                HostConfigurationStaticValidationError::ScriptHasBothContentAndPath(
                    self.name.clone(),
                ),
            ),
            (None, None) => Err(
                HostConfigurationStaticValidationError::ScriptHasNoContentOrPath(self.name.clone()),
            ),
            (None, Some(path)) => {
                if !path.is_absolute() {
                    return Err(HostConfigurationStaticValidationError::PathNotAbsolute {
                        path: path.clone().to_string_lossy().to_string(),
                    });
                }
                Ok(())
            }
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
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: Some("echo test".into()),
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: None,
        };
        assert!(script.should_run(ServicingType::CleanInstall));
    }

    #[test]
    fn test_should_run_false() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: Some("echo test".into()),
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: None,
        };
        assert!(!script.should_run(ServicingType::NormalUpdate));
    }

    #[test]
    fn test_should_run_all() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::All],
            interpreter: Some("/bin/bash".into()),
            content: Some("echo test".into()),
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: None,
        };
        assert!(script.should_run(ServicingType::AbUpdate));
    }

    #[test]
    fn test_invalid_script_with_no_content_or_path() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: None,
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: None,
        };
        assert_eq!(
            script.validate().unwrap_err(),
            HostConfigurationStaticValidationError::ScriptHasNoContentOrPath(script.name)
        );
    }

    #[test]
    fn test_invalid_script_with_both_content_and_path() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: Some("echo test".into()),
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: Some("/path/to/script".into()),
        };
        assert_eq!(
            script.validate().unwrap_err(),
            HostConfigurationStaticValidationError::ScriptHasBothContentAndPath(script.name)
        );
    }

    #[test]
    fn test_valid_script_with_absolute_path() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: None,
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: Some("/path/to/script".into()),
        };
        assert!(script.validate().is_ok());
    }

    #[test]
    fn test_invalid_script_with_relative_path() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            content: None,
            environment_variables: HashMap::new(),
            log_file_path: None,
            path: Some("path/to/script".into()),
        };
        assert_eq!(
            script.validate().unwrap_err(),
            HostConfigurationStaticValidationError::PathNotAbsolute {
                path: "path/to/script".into()
            }
        );
    }
}
