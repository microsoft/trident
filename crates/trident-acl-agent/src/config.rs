//! Env-var-based config loading for Harpoon.
//!
//! There is no config file. Every setting is an environment variable
//! prefixed `TRIDENT_ACL_AGENT_` (one constant per setting, e.g.
//! [`ENV_NEBRASKA_ENDPOINT`]), systemd-style: set it directly in the unit's
//! own `Environment=` lines, via a drop-in override (`systemctl edit
//! trident-acl-agent.service`, which creates
//! `/etc/systemd/system/trident-acl-agent.service.d/override.conf`), or by
//! any other means that ultimately sets the process's environment before it
//! starts. All are equivalent from the agent's point of view - it just reads
//! `std::env::var`.
//!
//! Annotation mode is the default; `omaha-only` (the historical one-shot
//! behavior) remains available as an explicit opt-out via
//! `TRIDENT_ACL_AGENT_ORCHESTRATION_GOAL_SOURCE=omaha-only`.

use std::{env, path::PathBuf, str::FromStr, time::Duration};

use url::Url;

use crate::{DEFAULT_NEBRASKA_APP_ID, DEFAULT_NEBRASKA_TRACK};

/// The environment variables this module reads, one constant per setting.
const ENV_NEBRASKA_ENDPOINT: &str = "TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT";
const ENV_NEBRASKA_APP_ID: &str = "TRIDENT_ACL_AGENT_NEBRASKA_APP_ID";
const ENV_NEBRASKA_TRACK: &str = "TRIDENT_ACL_AGENT_NEBRASKA_TRACK";
const ENV_KUBERNETES_API_SERVER: &str = "TRIDENT_ACL_AGENT_KUBERNETES_API_SERVER";
const ENV_KUBERNETES_KUBECONFIG: &str = "TRIDENT_ACL_AGENT_KUBERNETES_KUBECONFIG";
const ENV_KUBERNETES_NODE_NAME: &str = "TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME";
const ENV_TRIDENT_SOCKET: &str = "TRIDENT_ACL_AGENT_TRIDENT_SOCKET";
const ENV_ORCHESTRATION_GOAL_SOURCE: &str = "TRIDENT_ACL_AGENT_ORCHESTRATION_GOAL_SOURCE";
const ENV_ORCHESTRATION_STATE_PATH: &str = "TRIDENT_ACL_AGENT_ORCHESTRATION_STATE_PATH";
const ENV_ORCHESTRATION_STAGE_TIMEOUT: &str = "TRIDENT_ACL_AGENT_ORCHESTRATION_STAGE_TIMEOUT";
const ENV_ORCHESTRATION_FINALIZE_TIMEOUT: &str = "TRIDENT_ACL_AGENT_ORCHESTRATION_FINALIZE_TIMEOUT";
const ENV_ORCHESTRATION_HEARTBEAT_INTERVAL: &str =
    "TRIDENT_ACL_AGENT_ORCHESTRATION_HEARTBEAT_INTERVAL";

const DEFAULT_KUBERNETES_POLL_INTERVAL: Duration = Duration::from_secs(2);
// TODO: placeholder until the real production Nebraska/Omaha endpoint is
// known. `.invalid` is reserved by RFC 2606 and is guaranteed to never
// resolve, so a deployment that forgets to set
// TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT (or override it per-request via the
// update-request annotation's `server` field) fails loudly at the network
// layer instead of silently querying a real-looking but wrong host.
pub const DEFAULT_NEBRASKA_ENDPOINT: &str = "https://nebraska.example.invalid/v1/update";
const DEFAULT_STAGE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const DEFAULT_FINALIZE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
pub const DEFAULT_STATE_PATH: &str = "/var/lib/trident-acl-agent/state.json";
pub const DEFAULT_KUBELET_KUBECONFIG: &str = "/var/lib/kubelet/kubeconfig";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentConfig {
    pub nebraska: NebraskaConfig,
    pub kubernetes: KubernetesConfig,
    pub trident: TridentConfig,
    pub orchestration: OrchestrationConfig,
}

impl AgentConfig {
    /// Loads the effective config purely from `TRIDENT_ACL_AGENT_*`
    /// environment variables (see the module doc). A merely-absent variable
    /// is never an error - it just falls back to that setting's default -
    /// but a present-and-malformed value (bad URL, bad duration, unknown
    /// `goal_source`, etc.) is.
    pub fn from_env() -> Result<Self, anyhow::Error> {
        Ok(Self {
            nebraska: NebraskaConfig {
                endpoint: env_url(ENV_NEBRASKA_ENDPOINT)?
                    .or_else(|| Some(Url::parse(DEFAULT_NEBRASKA_ENDPOINT).expect("static url"))),
                app_id: env_string(ENV_NEBRASKA_APP_ID)
                    .unwrap_or_else(|| DEFAULT_NEBRASKA_APP_ID.to_string()),
                track: env_string(ENV_NEBRASKA_TRACK)
                    .unwrap_or_else(|| DEFAULT_NEBRASKA_TRACK.to_string()),
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
                goal_source: env_parse(ENV_ORCHESTRATION_GOAL_SOURCE)?.unwrap_or_default(),
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
pub struct NebraskaConfig {
    pub endpoint: Option<Url>,
    pub app_id: String,
    pub track: String,
}

impl Default for NebraskaConfig {
    fn default() -> Self {
        Self {
            endpoint: Some(Url::parse(DEFAULT_NEBRASKA_ENDPOINT).expect("static url")),
            app_id: DEFAULT_NEBRASKA_APP_ID.to_string(),
            track: DEFAULT_NEBRASKA_TRACK.to_string(),
        }
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
    pub api_server: Option<Url>,
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
pub struct TridentConfig {
    pub socket: String,
}

impl Default for TridentConfig {
    fn default() -> Self {
        Self {
            socket: trident_proto::TRIDENT_DEFAULT_SOCKET_URI.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GoalSource {
    /// Historical one-shot behavior: query Nebraska/Omaha once, and if an
    /// update is offered, call tridentd's combined `update()` RPC once and
    /// exit. No Kubernetes involvement at all - no annotations, no watch,
    /// no Node access. Kept as an explicit opt-out for nodes that don't
    /// participate in the AKS annotation-driven update protocol.
    OmahaOnly,
    /// The annotation-driven reconcile loop: watches the Node's
    /// `acl.azure.com/update-request` annotation and drives Trident's
    /// stage/finalize/rollback/commit operations against tridentd
    /// accordingly, writing progress back to `acl.azure.com/update-status`
    /// and `acl.azure.com/update-commit-status` (see accepted-design-v2.md).
    /// This is the default mode.
    #[default]
    Annotations,
}

impl FromStr for GoalSource {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "omaha-only" => Ok(GoalSource::OmahaOnly),
            "annotations" => Ok(GoalSource::Annotations),
            other => Err(anyhow::anyhow!(
                "unknown goal_source {other:?} (expected \"annotations\" or \"omaha-only\")"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationConfig {
    pub goal_source: GoalSource,
    pub state_path: PathBuf,
    /// Placeholder default pending real data from storm aclagent scenario runs.
    pub stage_timeout: Duration,
    /// Placeholder default pending real data from storm aclagent scenario runs.
    pub finalize_timeout: Duration,
    /// Refresh cadence for in-flight InProgress heartbeats. Default is well
    /// below the ~10 minute watchdog staleness target proposed in
    /// accepted-design-v2.md.
    pub heartbeat_interval: Duration,
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            goal_source: GoalSource::Annotations,
            state_path: PathBuf::from(DEFAULT_STATE_PATH),
            stage_timeout: DEFAULT_STAGE_TIMEOUT,
            finalize_timeout: DEFAULT_FINALIZE_TIMEOUT,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
        }
    }
}

/// Reads `name`, treating both "unset" and "set to the empty string" as
/// absent - a drop-in override that clears a variable to `""` should fall
/// back to the default, not try to parse an empty value.
fn env_raw(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

fn env_string(name: &str) -> Option<String> {
    env_raw(name)
}

fn env_url(name: &str) -> Result<Option<Url>, anyhow::Error> {
    env_raw(name)
        .map(|v| Url::parse(&v).map_err(|err| anyhow::anyhow!("invalid URL for {name}: {err}")))
        .transpose()
}

fn env_duration(name: &str, default: Duration) -> Result<Duration, anyhow::Error> {
    env_raw(name)
        .map(|v| {
            humantime::parse_duration(&v)
                .map_err(|err| anyhow::anyhow!("invalid duration for {name}: {err}"))
        })
        .transpose()
        .map(|parsed| parsed.unwrap_or(default))
}

fn env_parse<T>(name: &str) -> Result<Option<T>, anyhow::Error>
where
    T: FromStr<Err = anyhow::Error>,
{
    env_raw(name).map(|v| v.parse::<T>()).transpose()
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
        // in this crate reads or writes these TRIDENT_ACL_AGENT_* variables.
        unsafe {
            env::remove_var(ENV_NEBRASKA_ENDPOINT);
            env::remove_var(ENV_NEBRASKA_APP_ID);
            env::remove_var(ENV_NEBRASKA_TRACK);
            env::remove_var(ENV_KUBERNETES_API_SERVER);
            env::remove_var(ENV_KUBERNETES_KUBECONFIG);
            env::remove_var(ENV_KUBERNETES_NODE_NAME);
            env::remove_var(ENV_TRIDENT_SOCKET);
            env::remove_var(ENV_ORCHESTRATION_GOAL_SOURCE);
            env::remove_var(ENV_ORCHESTRATION_STATE_PATH);
            env::remove_var(ENV_ORCHESTRATION_STAGE_TIMEOUT);
            env::remove_var(ENV_ORCHESTRATION_FINALIZE_TIMEOUT);
            env::remove_var(ENV_ORCHESTRATION_HEARTBEAT_INTERVAL);
        }
    }

    #[test]
    fn env_config_defaults_then_overrides() {
        clear_env();

        let config = AgentConfig::from_env().unwrap();
        assert_eq!(
            config.nebraska.endpoint.unwrap().as_str(),
            DEFAULT_NEBRASKA_ENDPOINT
        );
        assert_eq!(config.nebraska.app_id, DEFAULT_NEBRASKA_APP_ID);
        assert_eq!(config.nebraska.track, DEFAULT_NEBRASKA_TRACK);
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
        assert_eq!(config.orchestration.goal_source, GoalSource::Annotations);
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
            env::set_var(
                ENV_NEBRASKA_ENDPOINT,
                "https://custom-nebraska.example.invalid/v1/update",
            );
            env::set_var(ENV_NEBRASKA_APP_ID, "custom-app");
            env::set_var(ENV_NEBRASKA_TRACK, "custom-track");
            env::set_var(ENV_KUBERNETES_API_SERVER, "https://cluster.example.invalid");
            env::set_var(ENV_KUBERNETES_KUBECONFIG, "/etc/harpoon/kubeconfig");
            env::set_var(ENV_KUBERNETES_NODE_NAME, "node-42");
            env::set_var(ENV_TRIDENT_SOCKET, "unix:///custom/trident.sock");
            env::set_var(ENV_ORCHESTRATION_GOAL_SOURCE, "omaha-only");
            env::set_var(
                ENV_ORCHESTRATION_STATE_PATH,
                "/var/lib/trident-acl-agent/custom-state.json",
            );
            env::set_var(ENV_ORCHESTRATION_STAGE_TIMEOUT, "21m");
            env::set_var(ENV_ORCHESTRATION_FINALIZE_TIMEOUT, "11m");
            env::set_var(ENV_ORCHESTRATION_HEARTBEAT_INTERVAL, "45s");
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
            "/etc/harpoon/kubeconfig"
        );
        assert_eq!(config.kubernetes.node_name, "node-42");
        assert_eq!(config.trident.socket, "unix:///custom/trident.sock");
        assert_eq!(config.orchestration.goal_source, GoalSource::OmahaOnly);
        assert_eq!(
            config.orchestration.state_path,
            PathBuf::from("/var/lib/trident-acl-agent/custom-state.json")
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
            env::set_var(ENV_NEBRASKA_APP_ID, "");
        }
        let config = AgentConfig::from_env().unwrap();
        assert_eq!(config.nebraska.app_id, DEFAULT_NEBRASKA_APP_ID);

        // --- a present-but-malformed URL is a parse error ------------------
        clear_env();
        // SAFETY: see clear_env's doc comment.
        unsafe {
            env::set_var(ENV_NEBRASKA_ENDPOINT, "not a url");
        }
        let err = AgentConfig::from_env().unwrap_err();
        assert!(err.to_string().contains(ENV_NEBRASKA_ENDPOINT), "{err}");

        // --- a present-but-unknown goal_source is a parse error ------------
        clear_env();
        // SAFETY: see clear_env's doc comment.
        unsafe {
            env::set_var(ENV_ORCHESTRATION_GOAL_SOURCE, "bogus");
        }
        let err = AgentConfig::from_env().unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");

        clear_env();
    }
}
