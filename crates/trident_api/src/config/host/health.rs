use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::config::host::scripts::{Script, ServicingTypeSelection};
use crate::status::ServicingType;

/// Scripts that can be run on the host during Trident stages.
/// These scripts are run in the order they are defined.
/// Ensure that the scripts are idempotent as they may be run multiple times.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Health {
    /// Scripts to be run before Trident commits an updated target OS as 'provisioned'.  If any of
    /// the scripts fail, commit will not be completed and rollback will be triggered.
    ///
    /// These scripts only run for updates, not installs. If runOn is specified for anything other
    /// than an update type, the script will be ignored.
    ///
    /// These scripts are run in the target OS. The `$TARGET_ROOT` variable
    /// will be set to '/' for consistency with postProvision scripts.
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<Check>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum Check {
    /// Raw contents of the script.
    Script(Script),

    /// Path to a script in the execution OS.
    SystemdCheck(SystemdCheck),
}

impl Check {
    /// Returns true if servicing type is enabled for this script.
    pub fn should_run(&self, servicing_type: ServicingType) -> bool {
        servicing_type == ServicingType::AbUpdate
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
            "invalid update check, expected a mapping",
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
    /// Name of the script.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,

    /// List of systemd services that need to be in successful state.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub systemd_services: Vec<String>,

    /// Timeout for the systemd check.
    pub timeout_seconds: usize,

    /// List of servicing types that the script should run on.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub run_on: Vec<ServicingTypeSelection>,
}

/// Unit Test for should_run
#[cfg(test)]
mod tests {
    use super::*;

    use crate::config::host::scripts::{ScriptSource, ServicingTypeSelection};

    fn create_test_health_checks() -> Health {
        Health {
            checks: vec![
                Check::Script(Script {
                    name: "test-script".into(),
                    run_on: vec![ServicingTypeSelection::AbUpdate],
                    interpreter: Some("/bin/bash".into()),
                    source: ScriptSource::Content("echo hi".into()),
                    ..Default::default()
                }),
                Check::SystemdCheck(SystemdCheck {
                    name: "test-systemd-check".into(),
                    systemd_services: vec!["test-service".into()],
                    timeout_seconds: 60,
                    run_on: vec![ServicingTypeSelection::AbUpdate],
                }),
            ],
        }
    }

    #[test]
    fn test_health_checks_should_run() {
        let health = create_test_health_checks();
        health.checks.iter().for_each(|check| {
            assert!(check.should_run(ServicingType::AbUpdate));
            assert!(!check.should_run(ServicingType::CleanInstall));
        });
    }

    #[test]
    fn test_health_checks_serde() {
        let health = create_test_health_checks();
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
