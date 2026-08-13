//! Config loading for Harpoon.
//!
//! See the design doc's endpoint override section (§12). Annotation mode is
//! the default; `omaha-only` (the historical one-shot behavior) remains
//! available as an explicit opt-out via `goal_source = "omaha-only"`.

use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::Context;
use serde::Deserialize;
use url::Url;

use crate::{DEFAULT_NEBRASKA_APP_ID, DEFAULT_NEBRASKA_TRACK};

pub const DEFAULT_CONFIG_PATH: &str = "/etc/trident/trident-acl-agent.conf";
const DEFAULT_KUBERNETES_POLL_INTERVAL: Duration = Duration::from_secs(2);
// TODO: placeholder until the real production Nebraska/Omaha endpoint is
// known. `.invalid` is reserved by RFC 2606 and is guaranteed to never
// resolve, so a deployment that forgets to override this fails loudly at
// the network layer instead of silently querying a real-looking but wrong
// host.
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
    pub fn load(path: &Path, explicit: bool) -> Result<Option<Self>, anyhow::Error> {
        match fs::read_to_string(path) {
            Ok(contents) => Ok(Some(Self::from_toml(&contents)?)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound && !explicit => Ok(None),
            Err(err) => Err(anyhow::Error::new(err).context(format!(
                "failed to read Harpoon config at {}",
                path.display()
            ))),
        }
    }

    pub fn from_toml(contents: &str) -> Result<Self, anyhow::Error> {
        let raw: RawAgentConfig =
            toml::from_str(contents).context("failed to parse trident-acl-agent.conf")?;
        raw.into_effective()
    }

    pub fn with_cli_endpoint(mut self, cli_endpoint: Option<Url>) -> Self {
        if cli_endpoint.is_some() {
            self.nebraska.endpoint = cli_endpoint;
        }
        self
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
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

#[derive(Debug, Default, Deserialize)]
struct RawAgentConfig {
    #[serde(default)]
    nebraska: RawNebraskaConfig,
    #[serde(default)]
    kubernetes: RawKubernetesConfig,
    #[serde(default)]
    trident: RawTridentConfig,
    #[serde(default)]
    orchestration: RawOrchestrationConfig,
}

impl RawAgentConfig {
    fn into_effective(self) -> Result<AgentConfig, anyhow::Error> {
        Ok(AgentConfig {
            nebraska: NebraskaConfig {
                endpoint: self
                    .nebraska
                    .endpoint
                    .or_else(|| Some(Url::parse(DEFAULT_NEBRASKA_ENDPOINT).expect("static url"))),
                app_id: self
                    .nebraska
                    .app_id
                    .unwrap_or_else(|| DEFAULT_NEBRASKA_APP_ID.to_string()),
                track: self
                    .nebraska
                    .track
                    .unwrap_or_else(|| DEFAULT_NEBRASKA_TRACK.to_string()),
            },
            kubernetes: KubernetesConfig {
                api_server: self.kubernetes.api_server,
                kubeconfig: self
                    .kubernetes
                    .kubeconfig
                    .unwrap_or_else(|| DEFAULT_KUBELET_KUBECONFIG.to_string()),
                node_name: self
                    .kubernetes
                    .node_name
                    .map(expand_env_token)
                    .transpose()?
                    .unwrap_or_else(default_node_name),
                watch_poll_interval: DEFAULT_KUBERNETES_POLL_INTERVAL,
            },
            trident: TridentConfig {
                socket: self
                    .trident
                    .socket
                    .unwrap_or_else(|| trident_proto::TRIDENT_DEFAULT_SOCKET_URI.to_string()),
            },
            orchestration: OrchestrationConfig {
                goal_source: self.orchestration.goal_source.unwrap_or_default(),
                state_path: self
                    .orchestration
                    .state_path
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_PATH)),
                stage_timeout: parse_duration(
                    self.orchestration.stage_timeout.as_deref(),
                    DEFAULT_STAGE_TIMEOUT,
                    "orchestration.stage_timeout",
                )?,
                finalize_timeout: parse_duration(
                    self.orchestration.finalize_timeout.as_deref(),
                    DEFAULT_FINALIZE_TIMEOUT,
                    "orchestration.finalize_timeout",
                )?,
                heartbeat_interval: parse_duration(
                    self.orchestration.heartbeat_interval.as_deref(),
                    DEFAULT_HEARTBEAT_INTERVAL,
                    "orchestration.heartbeat_interval",
                )?,
            },
        })
    }
}

#[derive(Debug, Default, Deserialize)]
struct RawNebraskaConfig {
    endpoint: Option<Url>,
    app_id: Option<String>,
    track: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawKubernetesConfig {
    api_server: Option<Url>,
    kubeconfig: Option<String>,
    node_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawTridentConfig {
    socket: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct RawOrchestrationConfig {
    goal_source: Option<GoalSource>,
    state_path: Option<String>,
    stage_timeout: Option<String>,
    finalize_timeout: Option<String>,
    heartbeat_interval: Option<String>,
}

fn parse_duration(
    value: Option<&str>,
    default: Duration,
    field: &str,
) -> Result<Duration, anyhow::Error> {
    value
        .map(|value| {
            humantime::parse_duration(value)
                .map_err(|err| anyhow::anyhow!("invalid duration for {field}: {err}"))
        })
        .transpose()?
        .unwrap_or(default)
        .pipe(Ok)
}

fn expand_env_token(value: String) -> Result<String, anyhow::Error> {
    if let Some(name) = value.strip_prefix("${").and_then(|v| v.strip_suffix('}')) {
        return env::var(name).map_err(|_| {
            anyhow::anyhow!("environment variable {name} is not set for kubernetes.node_name")
        });
    }
    if let Some(name) = value.strip_prefix('$') {
        return env::var(name).map_err(|_| {
            anyhow::anyhow!("environment variable {name} is not set for kubernetes.node_name")
        });
    }
    Ok(value)
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

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_defaults() {
        let config = AgentConfig::from_toml("").unwrap();
        assert_eq!(
            config.nebraska.endpoint.unwrap().as_str(),
            DEFAULT_NEBRASKA_ENDPOINT
        );
        assert_eq!(config.nebraska.app_id, DEFAULT_NEBRASKA_APP_ID);
        assert_eq!(
            config.kubernetes.api_server, None,
            "api_server should default to unset so the kubeconfig's own server is used as-is"
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
    }

    #[test]
    fn parses_overrides() {
        let config = AgentConfig::from_toml(
            r#"
            [nebraska]
            endpoint = "https://custom-nebraska.example.invalid/v1/update"
            app_id = "custom-app"
            track = "custom-track"

            [kubernetes]
            api_server = "https://cluster.example.invalid"
            kubeconfig = "/etc/harpoon/kubeconfig"
            node_name = "node-42"

            [trident]
            socket = "unix:///custom/trident.sock"

            [orchestration]
            goal_source = "omaha-only"
            state_path = "/var/lib/trident-acl-agent/custom-state.json"
            stage_timeout = "21m"
            finalize_timeout = "11m"
            heartbeat_interval = "45s"
            "#,
        )
        .unwrap();

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
    }
}
