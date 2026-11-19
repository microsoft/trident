use std::{
    io::ErrorKind,
    path::{Path, PathBuf},
};

use log::debug;

use trident_api::{
    constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT},
    error::{InvalidInputError, ReportError, TridentError},
};

pub struct AgentConfig {
    datastore: PathBuf,
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
            if let Some(path) = line.strip_prefix("DatastorePath=") {
                config.datastore = path.trim().into();
            }
        }

        Ok(config)
    }

    /// Get the datastore path from the AgentConfig.
    pub fn datastore_path(&self) -> &Path {
        &self.datastore
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
}
