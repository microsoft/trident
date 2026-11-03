use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use log::debug;

use trident_api::{
    constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT},
    error::{InvalidInputError, ReportError, TridentError},
};

/// Whether Trident should attempt to send tracing data to Application
/// Insights (best-effort, and only when a connection string was compiled
/// into the binary -- see [`crate::AZURE_MONITOR_CONNECTION_STRING`]).
///
/// Defaults to [`TelemetryPreference::OptIn`]: users can disable telemetry
/// via the Agent Configuration file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TelemetryPreference {
    /// Sending telemetry to Application Insights is disabled.
    OptOut,
    /// Telemetry is enabled, best-effort, provided a connection string was
    /// compiled into this Trident binary.
    #[default]
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

    fn load_from_path(path: impl AsRef<Path>) -> Result<Self, TridentError> {
        let path = path.as_ref();
        let mut config = Self {
            datastore: TRIDENT_DATASTORE_PATH_DEFAULT.into(),
            telemetry: TelemetryPreference::default(),
        };

        let contents = match std::fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == ErrorKind::NotFound => {
                debug!(
                    "Agent configuration file not found at {}, using defaults",
                    path.display()
                );
                return Ok(config);
            }
            Err(error) => {
                return Err(error).structured(InvalidInputError::ReadInputFile {
                    path: path.display().to_string(),
                });
            }
        };

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

        Ok(config)
    }

    /// Get the datastore path from the AgentConfig.
    pub fn datastore_path(&self) -> &Path {
        &self.datastore
    }

    /// Whether telemetry (best-effort tracing to Application Insights) is
    /// enabled per the agent configuration file. Defaults to `true` when
    /// unset; unrecognized values disable telemetry defensively.
    pub fn telemetry_enabled(&self) -> bool {
        matches!(self.telemetry, TelemetryPreference::OptIn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use trident_api::error::ErrorKind as TridentErrorKind;

    #[test]
    fn test_missing_config_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = AgentConfig::load_from_path(dir.path().join("missing.conf")).unwrap();

        assert_eq!(
            config.datastore_path(),
            Path::new(TRIDENT_DATASTORE_PATH_DEFAULT)
        );
    }

    #[test]
    fn test_readable_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trident.conf");
        for (contents, expected) in [
            ("", TRIDENT_DATASTORE_PATH_DEFAULT),
            ("OtherSetting=value\n", TRIDENT_DATASTORE_PATH_DEFAULT),
            (
                "DatastorePath= /custom/state.sqlite \n",
                "/custom/state.sqlite",
            ),
        ] {
            std::fs::write(&path, contents).unwrap();
            let config = AgentConfig::load_from_path(&path).unwrap();

            assert_eq!(config.datastore_path(), Path::new(expected));
        }
    }

    #[test]
    fn test_invalid_utf8_returns_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trident.conf");
        std::fs::write(&path, b"DatastorePath=/custom/state.sqlite\n# \xff\n").unwrap();

        let error = AgentConfig::load_from_path(&path)
            .err()
            .expect("invalid UTF-8 must fail");
        assert!(
            matches!(
                error.kind(),
                TridentErrorKind::InvalidInput(InvalidInputError::ReadInputFile { path: error_path })
                    if error_path == &path.display().to_string()
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn test_directory_returns_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path();

        let error = AgentConfig::load_from_path(path)
            .err()
            .expect("reading a directory must fail");
        assert!(
            matches!(
                error.kind(),
                TridentErrorKind::InvalidInput(InvalidInputError::ReadInputFile { path: error_path })
                    if error_path == &path.display().to_string()
            ),
            "got {error:?}"
        );
    }

    #[test]
    fn test_defaults_when_file_missing() {
        let config = AgentConfig::load_from_path("/nonexistent/path/for/trident-tests.conf")
            .expect("load_from_path should not fail even if the file is missing");
        assert_eq!(
            config.datastore_path(),
            Path::new(TRIDENT_DATASTORE_PATH_DEFAULT)
        );
        assert!(
            config.telemetry_enabled(),
            "telemetry must default to OptIn"
        );
    }

    #[test]
    fn test_telemetry_defaults_when_unset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trident.conf");
        for contents in [
            "",
            "# Telemetry=OptOut\n",
            "DatastorePath=/custom/path.sqlite\n",
        ] {
            std::fs::write(&path, contents).unwrap();

            let config = AgentConfig::load_from_path(path.to_str().unwrap()).unwrap();
            assert!(config.telemetry_enabled(), "config: {contents:?}");
        }
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

    #[test]
    fn test_telemetry_optout_with_invalid_utf8_returns_read_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("trident.conf");
        std::fs::write(&path, b"Telemetry=OptOut\n# \xff\n").unwrap();

        let error = AgentConfig::load_from_path(&path)
            .err()
            .expect("an unreadable opt-out must not fall back to OptIn");
        assert!(
            matches!(
                error.kind(),
                TridentErrorKind::InvalidInput(InvalidInputError::ReadInputFile { path: error_path })
                    if error_path == &path.display().to_string()
            ),
            "got {error:?}"
        );
    }
}
