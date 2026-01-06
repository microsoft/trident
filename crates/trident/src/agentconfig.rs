use std::path::{Path, PathBuf};

use trident_api::{
    constants::{AGENT_CONFIG_PATH, TRIDENT_DATASTORE_PATH_DEFAULT},
    error::TridentError,
};

pub struct AgentConfig {
    datastore: PathBuf,
}

impl AgentConfig {
    /// Load the AgentConfig from the default configuration file.
    pub fn load() -> Result<Self, TridentError> {
        let mut config = Self {
            datastore: TRIDENT_DATASTORE_PATH_DEFAULT.into(),
        };

        if let Ok(contents) = std::fs::read_to_string(AGENT_CONFIG_PATH) {
            for line in contents.lines() {
                if let Some(path) = line.strip_prefix("DatastorePath=") {
                    config.datastore = path.trim().into();
                }
            }
        } else {
            // If the config file does not exist, we proceed with defaults.
            // Only log this at debug level to avoid alarming users unnecessarily.
            log::info!(
                "Agent configuration file not found at {}, using defaults",
                AGENT_CONFIG_PATH
            );
        }

        Ok(config)
    }

    /// Get the datastore path from the AgentConfig.
    pub fn datastore_path(&self) -> &Path {
        &self.datastore
    }
}
