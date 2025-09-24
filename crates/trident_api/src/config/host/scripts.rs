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
    /// Scripts to be run before Trident begins servicing the host.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub pre_servicing: Vec<Script>,

    /// Scripts to be run after step 'Provision' in Trident is complete.
    ///
    /// These scripts are run with the root filesystem of the target OS mounted at `$TARGET_ROOT`
    /// and other partitions specified for the target OS mounted relative to that.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub post_provision: Vec<Script>,

    /// Scripts to be run after step 'Configure' in Trident is complete.
    ///
    /// These scripts are run from within a chroot of the target OS. The `$TARGET_ROOT` variable
    /// will be set to '/' for consistency with postProvision scripts.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub post_configure: Vec<Script>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum ScriptSource {
    /// Raw contents of the script.
    Content(String),

    /// Path to a script in the execution OS.
    Path(PathBuf),
}

/// Impl default for ScriptSource to instantiat so that Script can derive
/// default and it is easier to make definitions for tests and samples.
impl Default for ScriptSource {
    fn default() -> Self {
        ScriptSource::Content(String::new())
    }
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

    /// The source of the script.
    #[serde(flatten)]
    pub source: ScriptSource,

    /// Arguments to pass to the script.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub arguments: Vec<String>,

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
    /// This selection only includes CleanInstall, a clean install of the target OS image when the
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
        match &self.source {
            ScriptSource::Path(path) => {
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
            source: ScriptSource::Content("echo test".into()),
            environment_variables: HashMap::new(),
            arguments: vec![],
        };
        assert!(script.should_run(ServicingType::CleanInstall));
    }

    #[test]
    fn test_should_run_false() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            source: ScriptSource::Content("echo test".into()),
            environment_variables: HashMap::new(),
            arguments: vec![],
        };
        assert!(!script.should_run(ServicingType::NormalUpdate));
    }

    #[test]
    fn test_should_run_all() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::All],
            interpreter: Some("/bin/bash".into()),
            source: ScriptSource::Content("echo test".into()),
            environment_variables: HashMap::new(),
            arguments: vec![],
        };
        assert!(script.should_run(ServicingType::AbUpdate));
    }

    #[test]
    fn test_valid_script_with_absolute_path() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            environment_variables: HashMap::new(),
            source: ScriptSource::Path("/path/to/script".into()),
            arguments: vec![],
        };
        script.validate().unwrap();
    }

    #[test]
    fn test_valid_script_with_arguments() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            environment_variables: HashMap::new(),
            source: ScriptSource::Content("echo".into()),
            arguments: vec!["test".into()],
        };
        script.validate().unwrap();
    }

    #[test]
    fn test_invalid_script_with_relative_path() {
        let script = Script {
            name: "test-script".into(),
            run_on: vec![ServicingTypeSelection::CleanInstall],
            interpreter: Some("/bin/bash".into()),
            environment_variables: HashMap::new(),
            source: ScriptSource::Path("path/to/script".into()),
            arguments: vec![],
        };
        assert_eq!(
            script.validate().unwrap_err(),
            HostConfigurationStaticValidationError::PathNotAbsolute {
                path: "path/to/script".into()
            }
        );
    }
}
