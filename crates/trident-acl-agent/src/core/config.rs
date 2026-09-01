//! Env-var-based config loading for trident-acl-agent.
//!
//! There is no config file. Every setting is an environment variable
//! prefixed `TRIDENT_ACL_AGENT_<SECTION>_<FIELD>` (e.g.
//! `TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT`), systemd-style: set it directly in
//! the unit's own `Environment=` lines, via a drop-in override (`systemctl
//! edit trident-acl-agent.service`, which creates
//! `/etc/systemd/system/trident-acl-agent.service.d/override.conf`), or by
//! any other means that ultimately sets the process's environment before it
//! starts. All are equivalent from the agent's point of view.
//!
//! Loading goes through [`envy`], which deserializes a prefixed subset of
//! the environment into small `Raw*` structs below - one per section, with a
//! field per setting - via [`envy::prefixed`]. Every field is optional, so a
//! merely-absent variable is never an error; it just falls back to that
//! setting's default (applied by [`AgentConfig::from_vars`]). A
//! present-and-malformed value (bad URL, bad duration, unknown
//! `mode`, etc.) is. `envy::prefixed(..).from_iter(..)` also means
//! this module's own unit tests can build config from a plain iterator of
//! `(name, value)` pairs instead of mutating real (process-global, `unsafe`)
//! environment variables.
//!
//! Annotation mode is the default; `omaha-only` (the historical one-shot
//! behavior) remains available as an explicit opt-out via
//! `TRIDENT_ACL_AGENT_ORCHESTRATION_MODE=omaha-only`.

use std::{path::PathBuf, str::FromStr, time::Duration};

use anyhow::{anyhow, Context, Error};
use const_format::formatcp;
use serde::{de::Error as _, Deserialize, Deserializer};
use trident_proto::TRIDENT_DEFAULT_SOCKET_URI;
use url::Url;

use crate::{core::retry::MaxTries, DEFAULT_NEBRASKA_APP_ID, DEFAULT_NEBRASKA_TRACK};

const ENV_PREFIX_NEBRASKA: &str = "TRIDENT_ACL_AGENT_NEBRASKA_";
const ENV_PREFIX_KUBERNETES: &str = "TRIDENT_ACL_AGENT_KUBERNETES_";
const ENV_PREFIX_TRIDENT: &str = "TRIDENT_ACL_AGENT_TRIDENT_";
const ENV_PREFIX_ORCHESTRATION: &str = "TRIDENT_ACL_AGENT_ORCHESTRATION_";

const DEFAULT_KUBERNETES_POLL_INTERVAL: Duration = Duration::from_secs(2);
/// Total attempts (first attempt included) to connect to the Kubernetes API
/// server before giving up: covers both the startup/recovery Node read
/// (`Orchestrator::get_node_with_retry`) and the watch loop's tolerance for
/// consecutive transient stream errors (`Orchestrator::run`). Matches the
/// total attempts the previous hardcoded `RECOVERY_NODE_READ_RETRIES`
/// constant made, so a deployment that never sets the override sees no
/// behavior change. Override via
/// `TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_MAX_TRIES` - `0`, `"infinite"`, or
/// `"forever"` retries forever.
const DEFAULT_CONNECT_MAX_TRIES: MaxTries = MaxTries::Limited(3);
/// Delay between connect attempts. Override via
/// `TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_BACKOFF`.
const DEFAULT_CONNECT_BACKOFF: Duration = Duration::from_secs(2);
// TODO: placeholder until the real production Nebraska/Omaha endpoint is
// known, for omaha-only mode. `.invalid` is reserved by RFC 2606 and is
// guaranteed to never resolve, so a deployment that forgets to set
// TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT fails loudly at the network layer
// instead of silently querying a real-looking but wrong host. Annotation
// mode does not use this default at all: stage/finalize requests must
// carry their own `server` field, with no fallback to this config (see
// Orchestrator::resolve_nebraska_endpoint).
pub const DEFAULT_NEBRASKA_ENDPOINT: &str = "https://nebraska.example.invalid/v1/update";
const DEFAULT_NODE_NAME: &str = "localhost";
const DEFAULT_STAGE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
const DEFAULT_FINALIZE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60);
/// File name for the persisted agent state (see `annotations::state`).
pub const STATE_FILE_NAME: &str = "state.json";
pub const DEFAULT_STATE_PATH: &str = formatcp!("/var/lib/trident-acl-agent/{STATE_FILE_NAME}");
pub const DEFAULT_KUBELET_KUBECONFIG: &str = "/var/lib/kubelet/kubeconfig";
/// Default annotation-key prefix.
/// Override with `TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX`.
pub const DEFAULT_ANNOTATION_PREFIX: &str = "acl.microsoft.com";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentConfig {
    pub nebraska: NebraskaConfig,
    pub kubernetes: KubernetesConfig,
    pub trident: TridentConfig,
    pub orchestration: OrchestrationConfig,
}

impl AgentConfig {
    /// Loads the effective config purely from `TRIDENT_ACL_AGENT_*`
    /// environment variables (see the module doc).
    pub fn from_env() -> Result<Self, Error> {
        Self::from_vars(std::env::vars().collect())
    }

    /// Same as [`Self::from_env`], but reads from a plain `Vec` of `(name,
    /// value)` pairs instead of the real process environment.
    fn from_vars(vars: Vec<(String, String)>) -> Result<Self, Error> {
        let nebraska: RawNebraskaConfig = envy::prefixed(ENV_PREFIX_NEBRASKA)
            .from_iter(vars.iter().cloned())
            .with_context(|| format!("invalid {ENV_PREFIX_NEBRASKA}* environment variable"))?;
        let kubernetes: RawKubernetesConfig = envy::prefixed(ENV_PREFIX_KUBERNETES)
            .from_iter(vars.iter().cloned())
            .with_context(|| format!("invalid {ENV_PREFIX_KUBERNETES}* environment variable"))?;
        let trident: RawTridentConfig = envy::prefixed(ENV_PREFIX_TRIDENT)
            .from_iter(vars.iter().cloned())
            .with_context(|| format!("invalid {ENV_PREFIX_TRIDENT}* environment variable"))?;
        let orchestration: RawOrchestrationConfig = envy::prefixed(ENV_PREFIX_ORCHESTRATION)
            .from_iter(vars.iter().cloned())
            .with_context(|| format!("invalid {ENV_PREFIX_ORCHESTRATION}* environment variable"))?;

        Ok(Self {
            nebraska: NebraskaConfig {
                endpoint: Some(nebraska.endpoint.unwrap_or_else(default_nebraska_endpoint)),
                app_id: nebraska
                    .app_id
                    .unwrap_or_else(|| DEFAULT_NEBRASKA_APP_ID.to_string()),
                track: nebraska
                    .track
                    .unwrap_or_else(|| DEFAULT_NEBRASKA_TRACK.to_string()),
            },
            kubernetes: KubernetesConfig {
                api_server: kubernetes.api_server,
                kubeconfig: kubernetes
                    .kubeconfig
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_KUBELET_KUBECONFIG)),
                node_name: kubernetes.node_name.unwrap_or_else(default_node_name),
                watch_poll_interval: DEFAULT_KUBERNETES_POLL_INTERVAL,
                annotation_prefix: kubernetes
                    .annotation_prefix
                    .unwrap_or_else(|| DEFAULT_ANNOTATION_PREFIX.to_string()),
                connect_max_tries: kubernetes
                    .connect_max_tries
                    .unwrap_or(DEFAULT_CONNECT_MAX_TRIES),
                connect_backoff: kubernetes
                    .connect_backoff
                    .unwrap_or(DEFAULT_CONNECT_BACKOFF),
            },
            trident: TridentConfig {
                socket: trident
                    .socket
                    .unwrap_or_else(|| TRIDENT_DEFAULT_SOCKET_URI.to_string()),
            },
            orchestration: OrchestrationConfig {
                mode: orchestration.mode.unwrap_or_default(),
                state_path: orchestration
                    .state_path
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_PATH)),
                stage_timeout: orchestration.stage_timeout.unwrap_or(DEFAULT_STAGE_TIMEOUT),
                finalize_timeout: orchestration
                    .finalize_timeout
                    .unwrap_or(DEFAULT_FINALIZE_TIMEOUT),
                heartbeat_interval: orchestration
                    .heartbeat_interval
                    .unwrap_or(DEFAULT_HEARTBEAT_INTERVAL),
            },
        })
    }
}

/// Mirrors [`NebraskaConfig`], with every field optional: [`envy`] leaves a
/// field `None` when its environment variable is unset, so
/// [`AgentConfig::from_vars`] can apply this section's defaults itself.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawNebraskaConfig {
    #[serde(deserialize_with = "empty_url_as_none")]
    endpoint: Option<Url>,
    #[serde(deserialize_with = "empty_string_as_none")]
    app_id: Option<String>,
    #[serde(deserialize_with = "empty_string_as_none")]
    track: Option<String>,
}

/// Mirrors [`KubernetesConfig`] (see [`RawNebraskaConfig`]).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawKubernetesConfig {
    #[serde(deserialize_with = "empty_url_as_none")]
    api_server: Option<Url>,
    #[serde(deserialize_with = "empty_path_as_none")]
    kubeconfig: Option<PathBuf>,
    #[serde(deserialize_with = "empty_string_as_none")]
    node_name: Option<String>,
    #[serde(deserialize_with = "empty_string_as_none")]
    annotation_prefix: Option<String>,
    #[serde(deserialize_with = "empty_max_tries_as_none")]
    connect_max_tries: Option<MaxTries>,
    #[serde(deserialize_with = "empty_duration_as_none")]
    connect_backoff: Option<Duration>,
}

/// Mirrors [`TridentConfig`] (see [`RawNebraskaConfig`]).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTridentConfig {
    #[serde(deserialize_with = "empty_string_as_none")]
    socket: Option<String>,
}

/// Mirrors [`OrchestrationConfig`] (see [`RawNebraskaConfig`]).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawOrchestrationConfig {
    #[serde(deserialize_with = "empty_mode_as_none")]
    mode: Option<Mode>,
    #[serde(deserialize_with = "empty_path_as_none")]
    state_path: Option<PathBuf>,
    #[serde(deserialize_with = "empty_duration_as_none")]
    stage_timeout: Option<Duration>,
    #[serde(deserialize_with = "empty_duration_as_none")]
    finalize_timeout: Option<Duration>,
    #[serde(deserialize_with = "empty_duration_as_none")]
    heartbeat_interval: Option<Duration>,
}

/// Treats "set to the empty string" the same as "unset": a drop-in override
/// that clears a variable to `""` should fall back to the default, not try
/// to parse an empty value.
fn empty_as_none(value: String) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

fn empty_string_as_none<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(empty_as_none(String::deserialize(deserializer)?))
}

fn empty_url_as_none<'de, D>(deserializer: D) -> Result<Option<Url>, D::Error>
where
    D: Deserializer<'de>,
{
    empty_as_none(String::deserialize(deserializer)?)
        .map(|value| {
            Url::parse(&value)
                .map_err(|err| D::Error::custom(format!("invalid URL {value:?}: {err}")))
        })
        .transpose()
}

fn empty_path_as_none<'de, D>(deserializer: D) -> Result<Option<PathBuf>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(empty_as_none(String::deserialize(deserializer)?).map(PathBuf::from))
}

fn empty_duration_as_none<'de, D>(deserializer: D) -> Result<Option<Duration>, D::Error>
where
    D: Deserializer<'de>,
{
    empty_as_none(String::deserialize(deserializer)?)
        .map(|value| {
            humantime::parse_duration(&value)
                .map_err(|err| D::Error::custom(format!("invalid duration {value:?}: {err}")))
        })
        .transpose()
}

fn empty_mode_as_none<'de, D>(deserializer: D) -> Result<Option<Mode>, D::Error>
where
    D: Deserializer<'de>,
{
    empty_as_none(String::deserialize(deserializer)?)
        .map(|value| value.parse::<Mode>().map_err(D::Error::custom))
        .transpose()
}

fn empty_max_tries_as_none<'de, D>(deserializer: D) -> Result<Option<MaxTries>, D::Error>
where
    D: Deserializer<'de>,
{
    empty_as_none(String::deserialize(deserializer)?)
        .map(|value| value.parse::<MaxTries>().map_err(D::Error::custom))
        .transpose()
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
            endpoint: Some(default_nebraska_endpoint()),
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
    pub kubeconfig: PathBuf,
    pub node_name: String,
    pub watch_poll_interval: Duration,
    /// Annotation-key prefix used for the request/status/commit-status
    /// annotations (e.g. `acl.microsoft.com` in
    /// `acl.microsoft.com/update-request`). Defaults to
    /// [`DEFAULT_ANNOTATION_PREFIX`], overridable via
    /// `TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX` so a deployment can
    /// pick its own namespace instead.
    pub annotation_prefix: String,
    /// Total attempts (first attempt included) to connect to the
    /// Kubernetes API server - see [`DEFAULT_CONNECT_MAX_TRIES`]. Override
    /// via `TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_MAX_TRIES`.
    pub connect_max_tries: MaxTries,
    /// Delay between connect attempts. Override via
    /// `TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_BACKOFF`.
    pub connect_backoff: Duration,
}

impl Default for KubernetesConfig {
    fn default() -> Self {
        Self {
            api_server: None,
            kubeconfig: PathBuf::from(DEFAULT_KUBELET_KUBECONFIG),
            node_name: default_node_name(),
            watch_poll_interval: DEFAULT_KUBERNETES_POLL_INTERVAL,
            annotation_prefix: DEFAULT_ANNOTATION_PREFIX.to_string(),
            connect_max_tries: DEFAULT_CONNECT_MAX_TRIES,
            connect_backoff: DEFAULT_CONNECT_BACKOFF,
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
            socket: TRIDENT_DEFAULT_SOCKET_URI.to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Mode {
    /// Historical one-shot behavior: query Nebraska/Omaha once, and if an
    /// update is offered, call tridentd's combined `update()` RPC once and
    /// exit. No Kubernetes involvement at all - no annotations, no watch,
    /// no Node access. Not fully designed and not a supported deployment
    /// option - kept only as an internal escape hatch, and deliberately
    /// left out of user-facing docs. `Annotations` is the only documented,
    /// supported mode.
    #[doc(hidden)]
    OmahaOnly,
    /// The annotation-driven reconcile loop: watches the Node's
    /// `<annotation-prefix>/update-request` annotation and drives Trident's
    /// stage/finalize/rollback/commit operations against tridentd
    /// accordingly, writing progress back to
    /// `<annotation-prefix>/update-status` and
    /// `<annotation-prefix>/update-commit-status`. `<annotation-prefix>`
    /// defaults to
    /// [`DEFAULT_ANNOTATION_PREFIX`] (`acl.microsoft.com`), overridable via
    /// `TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX`. This is the only
    /// supported mode.
    #[default]
    Annotations,
}

impl FromStr for Mode {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "omaha-only" => Ok(Mode::OmahaOnly),
            "annotations" => Ok(Mode::Annotations),
            other => Err(anyhow!(
                "unknown mode {other:?} (expected \"annotations\" or \"omaha-only\")"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationConfig {
    pub mode: Mode,
    pub state_path: PathBuf,
    /// Placeholder default pending real data from storm aclagent scenario runs.
    pub stage_timeout: Duration,
    /// Placeholder default pending real data from storm aclagent scenario runs.
    pub finalize_timeout: Duration,
    /// Refresh cadence for in-flight InProgress heartbeats. Default is well
    /// below the ~10 minute watchdog staleness target.
    pub heartbeat_interval: Duration,
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Annotations,
            state_path: PathBuf::from(DEFAULT_STATE_PATH),
            stage_timeout: DEFAULT_STAGE_TIMEOUT,
            finalize_timeout: DEFAULT_FINALIZE_TIMEOUT,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
        }
    }
}

fn default_nebraska_endpoint() -> Url {
    Url::parse(DEFAULT_NEBRASKA_ENDPOINT)
        .expect("invariant: DEFAULT_NEBRASKA_ENDPOINT is a compile-time-valid URL")
}

fn default_node_name() -> String {
    // Kubernetes Node names must be valid RFC 1123 DNS labels, which are
    // lowercase-only; kubelet itself lowercases the hostname when it
    // registers the Node object. Match that behavior here so a mixed-case
    // hostname doesn't produce a node_name that can never match the actual
    // Node the agent is supposed to reconcile against.
    hostname::get()
        .ok()
        .and_then(|name| name.into_string().ok())
        .unwrap_or_else(|| DEFAULT_NODE_NAME.to_string())
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn defaults_when_all_vars_unset() {
        let config = AgentConfig::from_vars(vec![]).unwrap();

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
            PathBuf::from(DEFAULT_KUBELET_KUBECONFIG)
        );
        assert_eq!(config.trident.socket, TRIDENT_DEFAULT_SOCKET_URI);
        assert_eq!(config.orchestration.mode, Mode::Annotations);
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
        assert_eq!(
            config.kubernetes.annotation_prefix,
            DEFAULT_ANNOTATION_PREFIX
        );
        assert_eq!(
            config.kubernetes.connect_max_tries,
            DEFAULT_CONNECT_MAX_TRIES
        );
        assert_eq!(config.kubernetes.connect_backoff, DEFAULT_CONNECT_BACKOFF);
    }

    #[test]
    fn overrides_apply_when_vars_set() {
        let config = AgentConfig::from_vars(vars(&[
            (
                "TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT",
                "https://custom-nebraska.example.invalid/v1/update",
            ),
            ("TRIDENT_ACL_AGENT_NEBRASKA_APP_ID", "custom-app"),
            ("TRIDENT_ACL_AGENT_NEBRASKA_TRACK", "custom-track"),
            (
                "TRIDENT_ACL_AGENT_KUBERNETES_API_SERVER",
                "https://cluster.example.invalid",
            ),
            (
                "TRIDENT_ACL_AGENT_KUBERNETES_KUBECONFIG",
                "/etc/trident-acl-agent/kubeconfig",
            ),
            ("TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME", "node-42"),
            (
                "TRIDENT_ACL_AGENT_TRIDENT_SOCKET",
                "unix:///custom/trident.sock",
            ),
            ("TRIDENT_ACL_AGENT_ORCHESTRATION_MODE", "omaha-only"),
            (
                "TRIDENT_ACL_AGENT_ORCHESTRATION_STATE_PATH",
                "/var/lib/trident-acl-agent/custom-state.json",
            ),
            ("TRIDENT_ACL_AGENT_ORCHESTRATION_STAGE_TIMEOUT", "21m"),
            ("TRIDENT_ACL_AGENT_ORCHESTRATION_FINALIZE_TIMEOUT", "11m"),
            ("TRIDENT_ACL_AGENT_ORCHESTRATION_HEARTBEAT_INTERVAL", "45s"),
            (
                "TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX",
                "contoso.example.com",
            ),
            ("TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_MAX_TRIES", "infinite"),
            ("TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_BACKOFF", "5s"),
        ]))
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
            config.kubernetes.kubeconfig,
            PathBuf::from("/etc/trident-acl-agent/kubeconfig")
        );
        assert_eq!(config.kubernetes.node_name, "node-42");
        assert_eq!(config.trident.socket, "unix:///custom/trident.sock");
        assert_eq!(config.orchestration.mode, Mode::OmahaOnly);
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
        assert_eq!(config.kubernetes.annotation_prefix, "contoso.example.com");
        assert_eq!(config.kubernetes.connect_max_tries, MaxTries::Infinite);
        assert_eq!(config.kubernetes.connect_backoff, Duration::from_secs(5));
    }

    #[test]
    fn connect_max_tries_accepts_zero_and_a_bounded_count() {
        let config = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_MAX_TRIES",
            "0",
        )]))
        .unwrap();
        assert_eq!(config.kubernetes.connect_max_tries, MaxTries::Infinite);

        let config = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_MAX_TRIES",
            "7",
        )]))
        .unwrap();
        assert_eq!(config.kubernetes.connect_max_tries, MaxTries::Limited(7));
    }

    #[test]
    fn empty_value_falls_back_to_default() {
        let config =
            AgentConfig::from_vars(vars(&[("TRIDENT_ACL_AGENT_NEBRASKA_APP_ID", "")])).unwrap();
        assert_eq!(config.nebraska.app_id, DEFAULT_NEBRASKA_APP_ID);
    }

    #[test]
    fn malformed_url_is_a_parse_error() {
        let err = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT",
            "not a url",
        )]))
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a url"), "{err:#}");
    }

    #[test]
    fn malformed_mode_is_a_parse_error() {
        let err =
            AgentConfig::from_vars(vars(&[("TRIDENT_ACL_AGENT_ORCHESTRATION_MODE", "bogus")]))
                .unwrap_err();
        assert!(format!("{err:#}").contains("bogus"), "{err:#}");
    }

    #[test]
    fn malformed_connect_max_tries_is_a_parse_error() {
        let err = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_KUBERNETES_CONNECT_MAX_TRIES",
            "bogus",
        )]))
        .unwrap_err();
        assert!(format!("{err:#}").contains("bogus"), "{err:#}");
    }

    #[test]
    fn malformed_duration_is_a_parse_error() {
        let err = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_ORCHESTRATION_STAGE_TIMEOUT",
            "not a duration",
        )]))
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a duration"), "{err:#}");
    }
}
