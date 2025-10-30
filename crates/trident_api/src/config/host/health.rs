use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::config::host::scripts::{Script, ServicingTypeSelection};
use crate::status::ServicingType;

/// Configuration for the host OS health.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Health {
    /// Checks to be run before Trident commits a serviced target OS as 'provisioned'. If any of
    /// the checks fail, the commit will not be completed and, for A/B update, a rollback will be
    /// triggered.
    ///
    /// These checks run for installs and A/B updates. If `runOn` is specified for anything other
    /// than 'clean-install' or 'ab-update' type, the check will be ignored. If 'all' is
    /// specified, the check will run for both 'clean-install' and 'ab-update'.
    ///
    /// These checks are run in the target OS. The `$TARGET_ROOT` variable
    /// will be set to '/' for consistency with postProvision scripts.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum Check {
    /// Script that will be run. The success or failure of the script will define
    /// the health of the target OS.
    /// Valid script servicing types are CleanInstall and AbUpdate, or All (which
    /// will execute for both).
    Script(Script),

    /// Define systemd service(s) that need to be in a successful state, defined
    /// by `systemctl status` returning success. The success or failure of this
    /// check will define the health of the target OS.
    /// Valid script servicing types are CleanInstall and AbUpdate, or All (which
    /// will execute for both).
    SystemdCheck(SystemdCheck),
}

impl Check {
    /// Returns true if servicing type is enabled for this script.
    pub fn should_run(&self, servicing_type: ServicingType) -> bool {
        match servicing_type {
            ServicingType::CleanInstall | ServicingType::AbUpdate => { /* valid */ }
            _ => return false,
        }
        match self {
            Check::Script(script) => script.should_run(servicing_type),
            Check::SystemdCheck(systemd_check) => systemd_check.should_run(servicing_type),
        }
    }
}

/// Custom serialization and deserialization for UpdateCheck enum.
/// This is needed to avoid using YAML tags (i.e. !Script and !SystemdCheck) in
/// the serialized output.
impl<'de> serde::Deserialize<'de> for Check {
    fn deserialize<D>(deserializer: D) -> Result<Check, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = serde_yaml::Value::deserialize(deserializer)?;
        if let Some(mapping) = value.as_mapping() {
            if mapping.contains_key(serde_yaml::Value::String("systemdServices".to_string())) {
                // Deserialize as SystemdCheck
                let systemd_check: SystemdCheck =
                    serde_yaml::from_value(value).map_err(serde::de::Error::custom)?;
                return Ok(Check::SystemdCheck(systemd_check));
            } else {
                // Deserialize as Script
                let script: Script =
                    serde_yaml::from_value(value).map_err(serde::de::Error::custom)?;
                return Ok(Check::Script(script));
            }
        }
        Err(serde::de::Error::custom(
            "invalid health check, expected a mapping",
        ))
    }
}
impl serde::Serialize for Check {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Check::Script(script) => script.serialize(serializer),
            Check::SystemdCheck(systemd_check) => systemd_check.serialize(serializer),
        }
    }
}

/// A script that can be run on the host during Trident stages.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct SystemdCheck {
    /// Name of the check.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,

    /// List of systemd services that need to be in successful state.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub systemd_services: Vec<String>,

    /// Timeout for the systemd check.
    pub timeout_seconds: usize,

    /// List of servicing types that the check should run on.
    /// Valid servicing types are CleanInstall and AbUpdate, if
    /// All is specified, the check will run for both CleanInstall
    /// and AbUpdate.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub run_on: Vec<ServicingTypeSelection>,
}

impl SystemdCheck {
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

/// Unit Test for should_run
#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::host::scripts::{ScriptSource, ServicingTypeSelection};

    fn create_test_health_checks(run_on_servicing_type: ServicingTypeSelection) -> Health {
        Health {
            checks: vec![
                Check::Script(Script {
                    name: "test-script".into(),
                    run_on: vec![run_on_servicing_type.clone()],
                    interpreter: Some("/bin/bash".into()),
                    source: ScriptSource::Content("echo hi".into()),
                    ..Default::default()
                }),
                Check::SystemdCheck(SystemdCheck {
                    name: "test-systemd-check".into(),
                    systemd_services: vec!["test-service".into()],
                    timeout_seconds: 60,
                    run_on: vec![run_on_servicing_type.clone()],
                }),
            ],
        }
    }

    #[test]
    fn test_health_checks_should_run() {
        create_test_health_checks(ServicingTypeSelection::AbUpdate)
            .checks
            .iter()
            .for_each(|check| {
                assert!(check.should_run(ServicingType::AbUpdate));
                assert!(!check.should_run(ServicingType::CleanInstall));
            });
        create_test_health_checks(ServicingTypeSelection::CleanInstall)
            .checks
            .iter()
            .for_each(|check| {
                assert!(!check.should_run(ServicingType::AbUpdate));
                assert!(check.should_run(ServicingType::CleanInstall));
            });
        create_test_health_checks(ServicingTypeSelection::All)
            .checks
            .iter()
            .for_each(|check| {
                assert!(check.should_run(ServicingType::AbUpdate));
                assert!(check.should_run(ServicingType::CleanInstall));
            });
        create_test_health_checks(ServicingTypeSelection::NormalUpdate)
            .checks
            .iter()
            .for_each(|check| {
                assert!(!check.should_run(ServicingType::AbUpdate));
                assert!(!check.should_run(ServicingType::CleanInstall));
            });
    }

    #[test]
    fn test_health_checks_serde() {
        let health = create_test_health_checks(ServicingTypeSelection::AbUpdate);
        let serialized = serde_yaml::to_string(&health.checks).unwrap();
        println!("Serialized health check: {}", &serialized);
        assert!(
            !serialized.contains("!Script") && !serialized.contains("!SystemdCheck"),
            "Serialized health check should not use yaml tags to differentiate enum variants"
        );
        let deserialized: Vec<Check> = serde_yaml::from_str(&serialized).unwrap();
        assert_eq!(health.checks, deserialized);
    }
}
