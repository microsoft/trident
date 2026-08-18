//! Env-var-based config loading for `trident-aks-agent`.
//!
//! There is no config file. Every setting is an environment variable
//! prefixed `TRIDENT_AKS_AGENT_` (one constant per setting, e.g.
//! [`ENV_NEBRASKA_ENDPOINT`]), systemd-style: set it directly in the unit's
//! own `Environment=` lines, via a drop-in override (`systemctl edit
//! trident-aks-agent.service`, which creates
//! `/etc/systemd/system/trident-aks-agent.service.d/override.conf`), or by
//! any other means that ultimately sets the process's environment before it
//! starts. All are equivalent from the agent's point of view - it just reads
//! `std::env::var`.

use std::{path::PathBuf, time::Duration};

use trident_agent_core::config::{
    env_duration, env_string, env_url, NebraskaConfig, TridentConfig,
};

/// The environment variables this module reads, one constant per setting.
const ENV_NEBRASKA_ENDPOINT: &str = "TRIDENT_AKS_AGENT_NEBRASKA_ENDPOINT";
const ENV_NEBRASKA_APP_ID: &str = "TRIDENT_AKS_AGENT_NEBRASKA_APP_ID";
const ENV_NEBRASKA_TRACK: &str = "TRIDENT_AKS_AGENT_NEBRASKA_TRACK";
const ENV_KUBERNETES_API_SERVER: &str = "TRIDENT_AKS_AGENT_KUBERNETES_API_SERVER";
const ENV_KUBERNETES_KUBECONFIG: &str = "TRIDENT_AKS_AGENT_KUBERNETES_KUBECONFIG";
const ENV_KUBERNETES_NODE_NAME: &str = "TRIDENT_AKS_AGENT_KUBERNETES_NODE_NAME";
const ENV_TRIDENT_SOCKET: &str = "TRIDENT_AKS_AGENT_TRIDENT_SOCKET";
const ENV_ORCHESTRATION_STATE_PATH: &str = "TRIDENT_AKS_AGENT_ORCHESTRATION_STATE_PATH";
const ENV_ORCHESTRATION_STAGE_TIMEOUT: &str = "TRIDENT_AKS_AGENT_ORCHESTRATION_STAGE_TIMEOUT";
const ENV_ORCHESTRATION_FINALIZE_TIMEOUT: &str = "TRIDENT_AKS_AGENT_ORCHESTRATION_FINALIZE_TIMEOUT";
const ENV_ORCHESTRATION_HEARTBEAT_INTERVAL: &str =
    "TRIDENT_AKS_AGENT_ORCHESTRATION_HEARTBEAT_INTERVAL";

const DEFAULT_KUBERNETES_POLL_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_STAGE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const DEFAULT_FINALIZE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
pub const DEFAULT_STATE_PATH: &str = "/var/lib/trident-aks-agent/state.json";
pub const DEFAULT_KUBELET_KUBECONFIG: &str = "/var/lib/kubelet/kubeconfig";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentConfig {
    pub nebraska: NebraskaConfig,
    pub kubernetes: KubernetesConfig,
    pub trident: TridentConfig,
    pub orchestration: OrchestrationConfig,
}

impl AgentConfig {
    /// Loads the effective config purely from `TRIDENT_AKS_AGENT_*`
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
            kubernetes: KubernetesConfig {
                api_server: env_url(ENV_KUBERNETES_API_SERVER)?,
                kubeconfig: env_string(ENV_KUBERNETES_KUBECONFIG)
                    .unwrap_or_else(|| DEFAULT_KUBELET_KUBECONFIG.to_string()),
                node_name: env_string(ENV_KUBERNETES_NODE_NAME).unwrap_or_else(default_node_name),
                watch_poll_interval: DEFAULT_KUBERNETES_POLL_INTERVAL,
            },
            trident: TridentConfig {
                socket: env_string(ENV_TRIDENT_SOCKET)
                    .unwrap_or_else(|| trident_proto::TRIDENT_DEFAULT_SOCKET_URI.to_string()),
            },
            orchestration: OrchestrationConfig {
                state_path: env_string(ENV_ORCHESTRATION_STATE_PATH)
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_PATH)),
                stage_timeout: env_duration(
                    ENV_ORCHESTRATION_STAGE_TIMEOUT,
                    DEFAULT_STAGE_TIMEOUT,
                )?,
                finalize_timeout: env_duration(
                    ENV_ORCHESTRATION_FINALIZE_TIMEOUT,
                    DEFAULT_FINALIZE_TIMEOUT,
                )?,
                heartbeat_interval: env_duration(
                    ENV_ORCHESTRATION_HEARTBEAT_INTERVAL,
                    DEFAULT_HEARTBEAT_INTERVAL,
                )?,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KubernetesConfig {
    /// Explicit override for the Kubernetes API server URL. When unset, the
    /// server embedded in `kubeconfig` is used as-is (e.g. the real cluster
    /// FQDN a node's own `/var/lib/kubelet/kubeconfig` already points at).
    /// Only needed when the kubeconfig's own server is wrong for this
    /// deployment - e.g. a pod deployment wanting the in-cluster
    /// `https://kubernetes.default.svc` name, which a plain node-level
    /// kubeconfig has no reason to contain.
    pub api_server: Option<url::Url>,
    pub kubeconfig: String,
    pub node_name: String,
    pub watch_poll_interval: Duration,
}

impl Default for KubernetesConfig {
    fn default() -> Self {
        Self {
            api_server: None,
            kubeconfig: DEFAULT_KUBELET_KUBECONFIG.to_string(),
            node_name: default_node_name(),
            watch_poll_interval: DEFAULT_KUBERNETES_POLL_INTERVAL,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationConfig {
    pub state_path: PathBuf,
    /// Placeholder default pending real data from storm aclagent scenario runs.
    pub stage_timeout: Duration,
    /// Placeholder default pending real data from storm aclagent scenario runs.
    pub finalize_timeout: Duration,
    /// Refresh cadence for in-flight InProgress heartbeats. Default is well
    /// below the ~10 minute watchdog staleness target proposed in
    /// accepted-design-v3.md.
    pub heartbeat_interval: Duration,
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            state_path: PathBuf::from(DEFAULT_STATE_PATH),
            stage_timeout: DEFAULT_STAGE_TIMEOUT,
            finalize_timeout: DEFAULT_FINALIZE_TIMEOUT,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
        }
    }
}

fn default_node_name() -> String {
    // Kubernetes Node names must be valid RFC 1123 DNS labels, which are
    // lowercase-only; kubelet itself lowercases the hostname when it
    // registers the Node object. Match that behavior here so a mixed-case
    // hostname doesn't produce a node_name that can never match the actual
    // Node the agent is supposed to reconcile against.
    osutils::hostname::read()
        .unwrap_or_else(|_| "localhost".to_string())
        .to_lowercase()
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
        // in this crate reads or writes these TRIDENT_AKS_AGENT_* variables.
        unsafe {
            std::env::remove_var(ENV_NEBRASKA_ENDPOINT);
            std::env::remove_var(ENV_NEBRASKA_APP_ID);
            std::env::remove_var(ENV_NEBRASKA_TRACK);
            std::env::remove_var(ENV_KUBERNETES_API_SERVER);
            std::env::remove_var(ENV_KUBERNETES_KUBECONFIG);
            std::env::remove_var(ENV_KUBERNETES_NODE_NAME);
            std::env::remove_var(ENV_TRIDENT_SOCKET);
            std::env::remove_var(ENV_ORCHESTRATION_STATE_PATH);
            std::env::remove_var(ENV_ORCHESTRATION_STAGE_TIMEOUT);
            std::env::remove_var(ENV_ORCHESTRATION_FINALIZE_TIMEOUT);
            std::env::remove_var(ENV_ORCHESTRATION_HEARTBEAT_INTERVAL);
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
            config.kubernetes.api_server, None,
            "api_server should default to unset so the kubeconfig's own server is used as-is"
        );
        assert_eq!(
            config.kubernetes.kubeconfig,
            DEFAULT_KUBELET_KUBECONFIG.to_string()
        );
        assert_eq!(
            config.trident.socket,
            trident_proto::TRIDENT_DEFAULT_SOCKET_URI
        );
        assert_eq!(
            config.orchestration.state_path,
            PathBuf::from(DEFAULT_STATE_PATH)
        );
        assert_eq!(config.orchestration.stage_timeout, DEFAULT_STAGE_TIMEOUT);
        assert_eq!(
            config.orchestration.finalize_timeout,
            DEFAULT_FINALIZE_TIMEOUT
        );
        assert_eq!(
            config.orchestration.heartbeat_interval,
            DEFAULT_HEARTBEAT_INTERVAL
        );

        // SAFETY: see clear_env's doc comment.
        unsafe {
            std::env::set_var(
                ENV_NEBRASKA_ENDPOINT,
                "https://custom-nebraska.example.invalid/v1/update",
            );
            std::env::set_var(ENV_NEBRASKA_APP_ID, "custom-app");
            std::env::set_var(ENV_NEBRASKA_TRACK, "custom-track");
            std::env::set_var(ENV_KUBERNETES_API_SERVER, "https://cluster.example.invalid");
            std::env::set_var(
                ENV_KUBERNETES_KUBECONFIG,
                "/etc/trident-aks-agent/kubeconfig",
            );
            std::env::set_var(ENV_KUBERNETES_NODE_NAME, "node-42");
            std::env::set_var(ENV_TRIDENT_SOCKET, "unix:///custom/trident.sock");
            std::env::set_var(
                ENV_ORCHESTRATION_STATE_PATH,
                "/var/lib/trident-aks-agent/custom-state.json",
            );
            std::env::set_var(ENV_ORCHESTRATION_STAGE_TIMEOUT, "21m");
            std::env::set_var(ENV_ORCHESTRATION_FINALIZE_TIMEOUT, "11m");
            std::env::set_var(ENV_ORCHESTRATION_HEARTBEAT_INTERVAL, "45s");
        }

        let config = AgentConfig::from_env().unwrap();
        clear_env();

        assert_eq!(
            config.nebraska.endpoint.unwrap().as_str(),
            "https://custom-nebraska.example.invalid/v1/update"
        );
        assert_eq!(config.nebraska.app_id, "custom-app");
        assert_eq!(config.nebraska.track, "custom-track");
        assert_eq!(
            config.kubernetes.api_server.unwrap().as_str(),
            "https://cluster.example.invalid/"
        );
        assert_eq!(
            config.kubernetes.kubeconfig.as_str(),
            "/etc/trident-aks-agent/kubeconfig"
        );
        assert_eq!(config.kubernetes.node_name, "node-42");
        assert_eq!(config.trident.socket, "unix:///custom/trident.sock");
        assert_eq!(
            config.orchestration.state_path,
            PathBuf::from("/var/lib/trident-aks-agent/custom-state.json")
        );
        assert_eq!(
            config.orchestration.stage_timeout,
            Duration::from_secs(21 * 60)
        );
        assert_eq!(
            config.orchestration.finalize_timeout,
            Duration::from_secs(11 * 60)
        );
        assert_eq!(
            config.orchestration.heartbeat_interval,
            Duration::from_secs(45)
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
