use std::path::{Path, PathBuf};

use log::debug;

use trident_api::{
    constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT},
    error::TridentError,
};

/// Whether Trident should attempt to send tracing data to Application
/// Insights (best-effort, and only when a connection string was compiled
/// into the binary -- see [`crate::AZURE_MONITOR_CONNECTION_STRING`]).
///
/// Defaults to [`TelemetryPreference::OptOut`]: telemetry is disabled unless
/// a user has explicitly opted in via the Agent Configuration file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TelemetryPreference {
    /// Telemetry is disabled. Trident will not send any tracing data off
    /// the host.
    #[default]
    OptOut,
    /// Telemetry is enabled, best-effort, provided a connection string was
    /// compiled into this Trident binary.
    OptIn,
}

pub struct AgentConfig {
    datastore: PathBuf,
    telemetry: TelemetryPreference,
}

impl AgentConfig {
    /// Load the AgentConfig from the default configuration file.
    pub fn load() -> Result<Self, TridentError> {
        Self::load_from_path(AGENT_CONFIG_PATH)
    }

    /// Load the AgentConfig from an arbitrary path. Split out from [`load`]
    /// so the parsing logic can be unit tested without touching
    /// [`AGENT_CONFIG_PATH`].
    fn load_from_path(path: &str) -> Result<Self, TridentError> {
        let mut config = Self {
            datastore: TRIDENT_DATASTORE_PATH_DEFAULT.into(),
            telemetry: TelemetryPreference::default(),
        };

        if let Ok(contents) = std::fs::read_to_string(path) {
            for line in contents.lines() {
                if let Some(value) = line.strip_prefix("DatastorePath=") {
                    config.datastore = value.trim().into();
                } else if let Some(value) = line.strip_prefix("Telemetry=") {
                    config.telemetry = match value.trim().to_ascii_lowercase().as_str() {
                        "optin" => TelemetryPreference::OptIn,
                        "optout" => TelemetryPreference::OptOut,
                        other => {
                            debug!(
                                "Unrecognized Telemetry setting '{other}' in agent \
                                 configuration file, defaulting to OptOut"
                            );
                            TelemetryPreference::OptOut
                        }
                    };
                }
            }
        } else {
            // If the config file does not exist, we proceed with defaults.
            // Only log this at debug level to avoid alarming users unnecessarily.
            debug!("Agent configuration file not found at {path}, using defaults");
        }

        Ok(config)
    }

    /// Get the datastore path from the AgentConfig.
    pub fn datastore_path(&self) -> &Path {
        &self.datastore
    }

    /// Whether telemetry (best-effort tracing to Application Insights) is
    /// enabled per the agent configuration file. Defaults to `false`
    /// (opt-out) when unset or unrecognized.
    pub fn telemetry_enabled(&self) -> bool {
        matches!(self.telemetry, TelemetryPreference::OptIn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults_when_file_missing() {
        let config = AgentConfig::load_from_path("/nonexistent/path/for/trident-tests.conf")
            .expect("load_from_path should not fail even if the file is missing");
        assert_eq!(
            config.datastore_path(),
            Path::new(TRIDENT_DATASTORE_PATH_DEFAULT)
        );
        assert!(
            !config.telemetry_enabled(),
            "telemetry must default to OptOut"
        );
    }

    #[test]
    fn test_telemetry_optin() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trident.conf");
        std::fs::write(&path, "Telemetry=OptIn\n").unwrap();

        let config = AgentConfig::load_from_path(path.to_str().unwrap()).unwrap();
        assert!(config.telemetry_enabled());
    }

    #[test]
    fn test_telemetry_optout_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trident.conf");
        std::fs::write(&path, "Telemetry=OptOut\n").unwrap();

        let config = AgentConfig::load_from_path(path.to_str().unwrap()).unwrap();
        assert!(!config.telemetry_enabled());
    }

    #[test]
    fn test_telemetry_is_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trident.conf");
        std::fs::write(&path, "Telemetry=OPTIN\n").unwrap();

        let config = AgentConfig::load_from_path(path.to_str().unwrap()).unwrap();
        assert!(config.telemetry_enabled());
    }

    #[test]
    fn test_telemetry_unrecognized_value_defaults_optout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trident.conf");
        std::fs::write(&path, "Telemetry=maybe\n").unwrap();

        let config = AgentConfig::load_from_path(path.to_str().unwrap()).unwrap();
        assert!(!config.telemetry_enabled());
    }

    #[test]
    fn test_datastore_and_telemetry_together() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trident.conf");
        std::fs::write(
            &path,
            "DatastorePath=/custom/path.sqlite\nTelemetry=OptIn\n",
        )
        .unwrap();

        let config = AgentConfig::load_from_path(path.to_str().unwrap()).unwrap();
        assert!(config.telemetry_enabled());
        assert_eq!(config.datastore_path(), Path::new("/custom/path.sqlite"));
    }
}
