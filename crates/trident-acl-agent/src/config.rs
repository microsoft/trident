//! Env-var-based config loading for `trident-acl-agent`.
//!
//! There is no config file. Every setting is an environment variable
//! prefixed `TRIDENT_ACL_AGENT_` (one constant per setting, e.g.
//! [`ENV_NEBRASKA_ENDPOINT`]), systemd-style: set it directly in whatever
//! launches this binary (a systemd drop-in on a unit *provided elsewhere*,
//! a VM extension, AgentBaker, a wrapper script, etc.) - this crate itself
//! ships no unit file. All are equivalent from the agent's point of view -
//! it just reads `std::env::var`.

use std::time::Duration;

use trident_agent_core::config::{
    env_duration, env_string, env_url, NebraskaConfig, TridentConfig,
};

/// The environment variables this module reads, one constant per setting.
const ENV_NEBRASKA_ENDPOINT: &str = "TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT";
const ENV_NEBRASKA_APP_ID: &str = "TRIDENT_ACL_AGENT_NEBRASKA_APP_ID";
const ENV_NEBRASKA_TRACK: &str = "TRIDENT_ACL_AGENT_NEBRASKA_TRACK";
const ENV_TRIDENT_SOCKET: &str = "TRIDENT_ACL_AGENT_TRIDENT_SOCKET";
const ENV_ORCHESTRATION_STAGE_TIMEOUT: &str = "TRIDENT_ACL_AGENT_ORCHESTRATION_STAGE_TIMEOUT";
const ENV_ORCHESTRATION_FINALIZE_TIMEOUT: &str = "TRIDENT_ACL_AGENT_ORCHESTRATION_FINALIZE_TIMEOUT";

const DEFAULT_STAGE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const DEFAULT_FINALIZE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentConfig {
    pub nebraska: NebraskaConfig,
    pub trident: TridentConfig,
    pub orchestration: OrchestrationConfig,
}

impl AgentConfig {
    /// Loads the effective config purely from `TRIDENT_ACL_AGENT_*`
    /// environment variables (see the module doc). A merely-absent variable
    /// is never an error - it just falls back to that setting's default -
    /// but a present-and-malformed value (bad URL, bad duration, etc.) is.
    pub fn from_env() -> Result<Self, anyhow::Error> {
        Ok(Self {
            nebraska: NebraskaConfig {
                endpoint: env_url(ENV_NEBRASKA_ENDPOINT)?.or_else(|| {
                    Some(
                        url::Url::parse(trident_agent_core::config::DEFAULT_NEBRASKA_ENDPOINT)
                            .expect("static url"),
                    )
                }),
                app_id: env_string(ENV_NEBRASKA_APP_ID)
                    .unwrap_or_else(|| trident_agent_core::DEFAULT_NEBRASKA_APP_ID.to_string()),
                track: env_string(ENV_NEBRASKA_TRACK)
                    .unwrap_or_else(|| trident_agent_core::DEFAULT_NEBRASKA_TRACK.to_string()),
            },
            trident: TridentConfig {
                socket: env_string(ENV_TRIDENT_SOCKET)
                    .unwrap_or_else(|| trident_proto::TRIDENT_DEFAULT_SOCKET_URI.to_string()),
            },
            orchestration: OrchestrationConfig {
                stage_timeout: env_duration(
                    ENV_ORCHESTRATION_STAGE_TIMEOUT,
                    DEFAULT_STAGE_TIMEOUT,
                )?,
                finalize_timeout: env_duration(
                    ENV_ORCHESTRATION_FINALIZE_TIMEOUT,
                    DEFAULT_FINALIZE_TIMEOUT,
                )?,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationConfig {
    /// Placeholder default pending real data from storm aclagent scenario runs.
    pub stage_timeout: Duration,
    /// Placeholder default pending real data from storm aclagent scenario runs.
    pub finalize_timeout: Duration,
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            stage_timeout: DEFAULT_STAGE_TIMEOUT,
            finalize_timeout: DEFAULT_FINALIZE_TIMEOUT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Clears every var this module reads. Environment mutation is process-
    /// global and `std::env::remove_var`/`set_var` are `unsafe` (not
    /// thread-safe against concurrent reads elsewhere in the process), so
    /// all of the defaults/overrides/empty-value/malformed-value cases below
    /// are intentionally folded into one sequential `#[test]` rather than
    /// several separate ones that `cargo test` could run in parallel against
    /// the same variables.
    fn clear_env() {
        // SAFETY: single-threaded within this test function; no other test
        // in this crate reads or writes these TRIDENT_ACL_AGENT_* variables.
        unsafe {
            std::env::remove_var(ENV_NEBRASKA_ENDPOINT);
            std::env::remove_var(ENV_NEBRASKA_APP_ID);
            std::env::remove_var(ENV_NEBRASKA_TRACK);
            std::env::remove_var(ENV_TRIDENT_SOCKET);
            std::env::remove_var(ENV_ORCHESTRATION_STAGE_TIMEOUT);
            std::env::remove_var(ENV_ORCHESTRATION_FINALIZE_TIMEOUT);
        }
    }

    #[test]
    fn env_config_defaults_then_overrides() {
        clear_env();

        let config = AgentConfig::from_env().unwrap();
        assert_eq!(
            config.nebraska.endpoint.unwrap().as_str(),
            trident_agent_core::config::DEFAULT_NEBRASKA_ENDPOINT
        );
        assert_eq!(
            config.nebraska.app_id,
            trident_agent_core::DEFAULT_NEBRASKA_APP_ID
        );
        assert_eq!(
            config.nebraska.track,
            trident_agent_core::DEFAULT_NEBRASKA_TRACK
        );
        assert_eq!(
            config.trident.socket,
            trident_proto::TRIDENT_DEFAULT_SOCKET_URI
        );
        assert_eq!(config.orchestration.stage_timeout, DEFAULT_STAGE_TIMEOUT);
        assert_eq!(
            config.orchestration.finalize_timeout,
            DEFAULT_FINALIZE_TIMEOUT
        );

        // SAFETY: see clear_env's doc comment.
        unsafe {
            std::env::set_var(
                ENV_NEBRASKA_ENDPOINT,
                "https://custom-nebraska.example.invalid/v1/update",
            );
            std::env::set_var(ENV_NEBRASKA_APP_ID, "custom-app");
            std::env::set_var(ENV_NEBRASKA_TRACK, "custom-track");
            std::env::set_var(ENV_TRIDENT_SOCKET, "unix:///custom/trident.sock");
            std::env::set_var(ENV_ORCHESTRATION_STAGE_TIMEOUT, "21m");
            std::env::set_var(ENV_ORCHESTRATION_FINALIZE_TIMEOUT, "11m");
        }

        let config = AgentConfig::from_env().unwrap();
        clear_env();

        assert_eq!(
            config.nebraska.endpoint.unwrap().as_str(),
            "https://custom-nebraska.example.invalid/v1/update"
        );
        assert_eq!(config.nebraska.app_id, "custom-app");
        assert_eq!(config.nebraska.track, "custom-track");
        assert_eq!(config.trident.socket, "unix:///custom/trident.sock");
        assert_eq!(
            config.orchestration.stage_timeout,
            Duration::from_secs(21 * 60)
        );
        assert_eq!(
            config.orchestration.finalize_timeout,
            Duration::from_secs(11 * 60)
        );

        // --- empty value falls back to default, same as unset -------------
        clear_env();
        // SAFETY: see clear_env's doc comment.
        unsafe {
            std::env::set_var(ENV_NEBRASKA_APP_ID, "");
        }
        let config = AgentConfig::from_env().unwrap();
        assert_eq!(
            config.nebraska.app_id,
            trident_agent_core::DEFAULT_NEBRASKA_APP_ID
        );

        // --- a present-but-malformed URL is a parse error ------------------
        clear_env();
        // SAFETY: see clear_env's doc comment.
        unsafe {
            std::env::set_var(ENV_NEBRASKA_ENDPOINT, "not a url");
        }
        let err = AgentConfig::from_env().unwrap_err();
        assert!(err.to_string().contains(ENV_NEBRASKA_ENDPOINT), "{err}");

        clear_env();
    }
}
