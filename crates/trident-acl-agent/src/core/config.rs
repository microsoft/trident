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

use std::{
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use anyhow::{anyhow, Context, Error};
use const_format::formatcp;
use log::warn;
use openssl::{nid::Nid, x509::X509};
use serde::{de::Error as _, Deserialize, Deserializer};
use trident_proto::TRIDENT_DEFAULT_SOCKET_URI;
use url::Url;

use crate::{DEFAULT_NEBRASKA_APP_ID, DEFAULT_NEBRASKA_TRACK};

const ENV_PREFIX_NEBRASKA: &str = "TRIDENT_ACL_AGENT_NEBRASKA_";
const ENV_PREFIX_KUBERNETES: &str = "TRIDENT_ACL_AGENT_KUBERNETES_";
const ENV_PREFIX_TRIDENT: &str = "TRIDENT_ACL_AGENT_TRIDENT_";
const ENV_PREFIX_ORCHESTRATION: &str = "TRIDENT_ACL_AGENT_ORCHESTRATION_";

const DEFAULT_KUBERNETES_POLL_INTERVAL: Duration = Duration::from_secs(2);
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
/// Default for `node_gone_max_wait`: unset, i.e. wait for the Node to
/// reappear indefinitely. See that field's docs for why this is the
/// safer default and when an operator would want to bound it instead.
const DEFAULT_NODE_GONE_MAX_WAIT: Option<Duration> = None;
/// File name for the persisted agent state (see `annotations::state`).
pub const STATE_FILE_NAME: &str = "state.json";
pub const DEFAULT_STATE_PATH: &str = formatcp!("/var/lib/trident-acl-agent/{STATE_FILE_NAME}");
pub const DEFAULT_KUBELET_KUBECONFIG: &str = "/var/lib/kubelet/kubeconfig";
/// Default path to kubelet's own client certificate, used to derive
/// `node_name` when `TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE` is
/// `kubelet-cert`. Not independently overridable (yet) - only the source
/// selection is.
const DEFAULT_KUBELET_CLIENT_CERT: &str = "/var/lib/kubelet/pki/kubelet-client-current.pem";
/// Prefix kubelet's client cert Subject CN always carries
/// (`system:node:<node-name>`), per the Kubernetes TLS bootstrapping spec.
const KUBELET_CERT_CN_NODE_PREFIX: &str = "system:node:";
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
            kubernetes: {
                let node_name_source = kubernetes.node_name_source.unwrap_or_default();
                KubernetesConfig {
                    api_server: kubernetes.api_server,
                    kubeconfig: kubernetes
                        .kubeconfig
                        .unwrap_or_else(|| PathBuf::from(DEFAULT_KUBELET_KUBECONFIG)),
                    node_name: kubernetes
                        .node_name
                        .unwrap_or_else(|| default_node_name(node_name_source)),
                    node_name_source,
                    watch_poll_interval: DEFAULT_KUBERNETES_POLL_INTERVAL,
                    annotation_prefix: kubernetes
                        .annotation_prefix
                        .unwrap_or_else(|| DEFAULT_ANNOTATION_PREFIX.to_string()),
                }
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
                node_gone_max_wait: orchestration
                    .node_gone_max_wait
                    .or(DEFAULT_NODE_GONE_MAX_WAIT),
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
    #[serde(deserialize_with = "empty_node_name_source_as_none")]
    node_name_source: Option<NodeNameSource>,
    #[serde(deserialize_with = "empty_string_as_none")]
    annotation_prefix: Option<String>,
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
    #[serde(deserialize_with = "empty_duration_as_none")]
    node_gone_max_wait: Option<Duration>,
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

fn empty_node_name_source_as_none<'de, D>(
    deserializer: D,
) -> Result<Option<NodeNameSource>, D::Error>
where
    D: Deserializer<'de>,
{
    empty_as_none(String::deserialize(deserializer)?)
        .map(|value| value.parse::<NodeNameSource>().map_err(D::Error::custom))
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
    /// Which source `node_name` was (or would be) derived from when no
    /// explicit `TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME` override is set.
    /// See [`NodeNameSource`] for the two options and why hostname remains
    /// the default despite not being AKS-guaranteed.
    pub node_name_source: NodeNameSource,
    pub watch_poll_interval: Duration,
    /// Annotation-key prefix used for the request/status/commit-status
    /// annotations (e.g. `acl.microsoft.com` in
    /// `acl.microsoft.com/update-request`). Defaults to
    /// [`DEFAULT_ANNOTATION_PREFIX`], overridable via
    /// `TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX` so a deployment can
    /// pick its own namespace instead.
    pub annotation_prefix: String,
}

impl Default for KubernetesConfig {
    fn default() -> Self {
        let node_name_source = NodeNameSource::default();
        Self {
            api_server: None,
            kubeconfig: PathBuf::from(DEFAULT_KUBELET_KUBECONFIG),
            node_name: default_node_name(node_name_source),
            node_name_source,
            watch_poll_interval: DEFAULT_KUBERNETES_POLL_INTERVAL,
            annotation_prefix: DEFAULT_ANNOTATION_PREFIX.to_string(),
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NodeNameSource {
    /// Lowercased `hostname()`. This matches cloud-provider-azure's current
    /// AKS convention (the Node object is registered under the lowercased
    /// hostname; see AgentBaker's `cse_config.sh`), but per the AKS team
    /// that convention is an implementation detail and is **not**
    /// guaranteed to hold. Kept as the default anyway, for backward
    /// compatibility with existing deployments - switch to `KubeletCert`
    /// explicitly where the guarantee matters.
    #[default]
    Hostname,
    /// Subject CN of kubelet's own client certificate
    /// (`/var/lib/kubelet/pki/kubelet-client-current.pem`), stripped of its
    /// `system:node:` prefix. This is the identity kubelet itself
    /// authenticates to the API server with, so it is guaranteed to match
    /// the real Node name regardless of hostname conventions. Opt in with
    /// `TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE=kubelet-cert`. If the
    /// cert can't be read or parsed, falls back to `Hostname` (logging a
    /// warning) rather than failing startup outright.
    KubeletCert,
}

impl FromStr for NodeNameSource {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "hostname" => Ok(Self::Hostname),
            "kubelet-cert" => Ok(Self::KubeletCert),
            other => Err(anyhow!(
                "unknown node_name_source {other:?} (expected \"hostname\" or \"kubelet-cert\")"
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
    /// Bounds how long `Orchestrator::await_node_recreation` will wait for
    /// the agent's own Node object to reappear after a 404 before giving up
    /// and returning an error (which propagates out of `run()` and exits the
    /// process - the pre-resilience behavior). Unset (`None`) by default:
    /// the agent waits indefinitely, since a 404 can be transient (e.g. a
    /// startup race before kubelet registers the Node) or self-healing (a
    /// delete+recreate during node replacement), and there is no reliable
    /// way to distinguish those from a truly permanent deletion from this
    /// side. Set `TRIDENT_ACL_AGENT_ORCHESTRATION_NODE_GONE_MAX_WAIT` (e.g.
    /// `30m`) to restore a bounded, fail-fast exit for deployments that
    /// prefer an external supervisor (systemd, a DaemonSet controller) to
    /// own re-creation/restart decisions instead.
    pub node_gone_max_wait: Option<Duration>,
}

impl Default for OrchestrationConfig {
    fn default() -> Self {
        Self {
            mode: Mode::Annotations,
            state_path: PathBuf::from(DEFAULT_STATE_PATH),
            stage_timeout: DEFAULT_STAGE_TIMEOUT,
            finalize_timeout: DEFAULT_FINALIZE_TIMEOUT,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            node_gone_max_wait: DEFAULT_NODE_GONE_MAX_WAIT,
        }
    }
}

fn default_nebraska_endpoint() -> Url {
    Url::parse(DEFAULT_NEBRASKA_ENDPOINT)
        .expect("invariant: DEFAULT_NEBRASKA_ENDPOINT is a compile-time-valid URL")
}

fn default_node_name(source: NodeNameSource) -> String {
    match source {
        NodeNameSource::Hostname => hostname_node_name(),
        NodeNameSource::KubeletCert => node_name_from_kubelet_cert(Path::new(
            DEFAULT_KUBELET_CLIENT_CERT,
        ))
        .unwrap_or_else(|err| {
            warn!(
                "failed to derive node_name from kubelet client cert at \
                         {DEFAULT_KUBELET_CLIENT_CERT} ({err:#}); falling back to hostname"
            );
            hostname_node_name()
        }),
    }
}

fn hostname_node_name() -> String {
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

/// Derives `node_name` from kubelet's own client cert's Subject CN
/// (`system:node:<node-name>`), stripping the `system:node:` prefix.
fn node_name_from_kubelet_cert(cert_path: &Path) -> Result<String, Error> {
    let pem = std::fs::read(cert_path).with_context(|| format!("failed to read {cert_path:?}"))?;
    let cert = X509::from_pem(&pem)
        .with_context(|| format!("failed to parse X.509 certificate from {cert_path:?}"))?;
    let cn = cert
        .subject_name()
        .entries_by_nid(Nid::COMMONNAME)
        .next()
        .and_then(|entry| entry.data().as_utf8().ok())
        .map(|cn| cn.to_string())
        .ok_or_else(|| anyhow!("certificate at {cert_path:?} has no Subject CN"))?;
    cn.strip_prefix(KUBELET_CERT_CN_NODE_PREFIX)
        .map(ToString::to_string)
        .ok_or_else(|| {
            anyhow!(
                "certificate at {cert_path:?} has Subject CN {cn:?}, \
                 which doesn't start with {KUBELET_CERT_CN_NODE_PREFIX:?}"
            )
        })
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
            config.orchestration.node_gone_max_wait, None,
            "node_gone_max_wait should default to unset so the agent waits indefinitely for the node to reappear"
        );
        assert_eq!(
            config.kubernetes.annotation_prefix,
            DEFAULT_ANNOTATION_PREFIX
        );
        assert_eq!(
            config.kubernetes.node_name_source,
            NodeNameSource::Hostname,
            "hostname must remain the default node_name_source for backward compatibility"
        );
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
                "TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE",
                "kubelet-cert",
            ),
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
            ("TRIDENT_ACL_AGENT_ORCHESTRATION_NODE_GONE_MAX_WAIT", "30m"),
            (
                "TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX",
                "contoso.example.com",
            ),
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
        assert_eq!(
            config.kubernetes.node_name_source,
            NodeNameSource::KubeletCert
        );
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
        assert_eq!(
            config.orchestration.node_gone_max_wait,
            Some(Duration::from_secs(30 * 60))
        );
        assert_eq!(config.kubernetes.annotation_prefix, "contoso.example.com");
    }

    #[test]
    fn empty_value_falls_back_to_default() {
        let config =
            AgentConfig::from_vars(vars(&[("TRIDENT_ACL_AGENT_NEBRASKA_APP_ID", "")])).unwrap();
        assert_eq!(config.nebraska.app_id, DEFAULT_NEBRASKA_APP_ID);
    }

    #[test]
    fn node_gone_max_wait_unset_by_empty_value_waits_indefinitely() {
        let config = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_ORCHESTRATION_NODE_GONE_MAX_WAIT",
            "",
        )]))
        .unwrap();
        assert_eq!(config.orchestration.node_gone_max_wait, None);
    }

    #[test]
    fn malformed_node_gone_max_wait_is_a_parse_error() {
        let err = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_ORCHESTRATION_NODE_GONE_MAX_WAIT",
            "not a duration",
        )]))
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a duration"), "{err:#}");
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
    fn malformed_duration_is_a_parse_error() {
        let err = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_ORCHESTRATION_STAGE_TIMEOUT",
            "not a duration",
        )]))
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a duration"), "{err:#}");
    }

    #[test]
    fn malformed_node_name_source_is_a_parse_error() {
        let err = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE",
            "bogus",
        )]))
        .unwrap_err();
        assert!(format!("{err:#}").contains("bogus"), "{err:#}");
    }

    #[test]
    fn empty_node_name_source_falls_back_to_hostname_default() {
        let config = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME_SOURCE",
            "",
        )]))
        .unwrap();
        assert_eq!(config.kubernetes.node_name_source, NodeNameSource::Hostname);
    }

    // CN "system:node:aks-nodepool1-36435499-vmss000000", O "system:nodes" -
    // the shape a real AKS kubelet client cert has.
    const TEST_CERT_NODE_CN: &str = "-----BEGIN CERTIFICATE-----
MIIDfzCCAmegAwIBAgIUd01WEzdpjKsn3oATTeMT8zBL5cIwDQYJKoZIhvcNAQEL
BQAwTzEVMBMGA1UECgwMc3lzdGVtOm5vZGVzMTYwNAYDVQQDDC1zeXN0ZW06bm9k
ZTpha3Mtbm9kZXBvb2wxLTM2NDM1NDk5LXZtc3MwMDAwMDAwHhcNMjYxMDA5MTYw
MTUwWhcNMzYxMDA2MTYwMTUwWjBPMRUwEwYDVQQKDAxzeXN0ZW06bm9kZXMxNjA0
BgNVBAMMLXN5c3RlbTpub2RlOmFrcy1ub2RlcG9vbDEtMzY0MzU0OTktdm1zczAw
MDAwMDCCASIwDQYJKoZIhvcNAQEBBQADggEPADCCAQoCggEBAL6Zd1AjlXLtYCFb
JTomb9M94zkzT49It6TqBKOiETnnGIhWS3F917ueaUJhhcSzMwKdGIaUU3FM5NPV
Tsd2zBnKs8/YkrVbP+Fyn6K4u5Q/8erilf3A3I4Dh/soxauK0H4HNh3UzEnwdvks
HasXEcVbb8488ld8p0eUmx8ycHXTnTVA00oiyp6wIbRClwOfIgox3a+0Fl0u49Wv
TIhGbUPj1Hkpm6BrTk5zoVKFOKtjmWBJ/tK/Oi1OcGF3cOW/tnJ+yhKYbk+a7Pok
EHqpFwhpJ28xgi9IVxIPWR9bskGdbX39r5EwXRhM3eguI7ReiupwAjR2UHbuHjx0
5KIp8ucCAwEAAaNTMFEwHQYDVR0OBBYEFPrWvCYXfgGQ+8AskqoFpLVwxFsTMB8G
A1UdIwQYMBaAFPrWvCYXfgGQ+8AskqoFpLVwxFsTMA8GA1UdEwEB/wQFMAMBAf8w
DQYJKoZIhvcNAQELBQADggEBAIn8Qv+hKXd1mCw+9QY1r4t07rSY5pcrEJh2H56g
67c72HvW+6xhizPyctkpFqH8bFkr5Ob+jASvlJenV1FRENYEdw9esMzxNWd0RQdu
9EYHZInD1XBF9n3R1yavtNT8NkCCReB33R3qPySqeFFZ0ATjRThoc4676V/34/Vt
hLURihiYkJJiIPByX4RxoB1dq1Etk6pGtlGquMjmNN5eo+YKqhejRkjdk5YQmuws
G3/d/F+SqLfGBUcGRBU1bw7Wu4KmkBMBgPOCUyWcspQsB4l0fhCOs4VyYB2DJjPb
wN+7zVpbIrXaPJX/yoPlLodQ6TbM/08QfJ6jRBTZ3iLjm+o=
-----END CERTIFICATE-----
";

    // CN "not-a-node-identity" - a well-formed cert that isn't a kubelet
    // client cert, to exercise the "no system:node: prefix" error path.
    const TEST_CERT_BAD_CN: &str = "-----BEGIN CERTIFICATE-----
MIIDHTCCAgWgAwIBAgIUUuLrFudHzqPoZiQlWpn0pwkQfekwDQYJKoZIhvcNAQEL
BQAwHjEcMBoGA1UEAwwTbm90LWEtbm9kZS1pZGVudGl0eTAeFw0yNjEwMDkxNjAx
NTdaFw0zNjEwMDYxNjAxNTdaMB4xHDAaBgNVBAMME25vdC1hLW5vZGUtaWRlbnRp
dHkwggEiMA0GCSqGSIb3DQEBAQUAA4IBDwAwggEKAoIBAQCh4AGuv6pTVUVSeoCd
hjhUNpuNOFzgdvOvFn3CdqjgBrV6emS0jf3eRBKKzyrhU0zSOHr0/ulCXGGE4+ah
bak666z/23F17ilSM9Z5iJdrDfSKmmTLpswv5/UyJwrdUQJjrqH4zweWOHSDj81P
xX6RjIfMaLOh3GWXrrSb4IJtQ+ms4zPiz73mXxoxDNG9Z8PV8Rk9YeyqANKs1ovi
zhOI+fBTMQbnYCyWIxGHjeJWTBtVjUtU1eiU/iXlFIWQgNGxYgL94qPITNGLNA/p
7TV8UfsGZukNKOImW4TgGyghGoSY6vda6eIsKHr/ezgRRFnN/AQ1fzYX/eIzYcZl
wRA3AgMBAAGjUzBRMB0GA1UdDgQWBBT1y6XPmHYq/vxuuM66ZzLhoPZDGjAfBgNV
HSMEGDAWgBT1y6XPmHYq/vxuuM66ZzLhoPZDGjAPBgNVHRMBAf8EBTADAQH/MA0G
CSqGSIb3DQEBCwUAA4IBAQB0/859B0zfEujTcUX8sS19minrLuXk/FhttIoH1Wl0
DOF4kbsN1mkiaBWXFqOivTbBFVkVVv+yzvPRyv0NKVzKyFOy05Q2Mz79ZnXTEDTN
naxxy4nls8EAcarecR9XKkLbJgE+pUZ7bOL+qUNyRBIF7RdDSIyYcNYLg5XoA982
xFeJ1m79RhbveFGkhVCsdvaf07z+8JZtEH+apEUjtBHbRU+eXt+ovD3yoxufw8RW
P1bXRwozJvvsUsxTGkU7z3y54wDQWGRf43vRNYcXOiY45sgqOX46m56S1R0Xumpd
gf0TbLqnNti3MeMftKcEa8VQ2rE6Np02EETxr94lsiNK
-----END CERTIFICATE-----
";

    #[test]
    fn node_name_from_kubelet_cert_strips_system_node_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("kubelet-client-current.pem");
        std::fs::write(&cert_path, TEST_CERT_NODE_CN).unwrap();

        let node_name = node_name_from_kubelet_cert(&cert_path).unwrap();
        assert_eq!(node_name, "aks-nodepool1-36435499-vmss000000");
    }

    #[test]
    fn node_name_from_kubelet_cert_errors_without_system_node_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let cert_path = dir.path().join("not-a-kubelet-cert.pem");
        std::fs::write(&cert_path, TEST_CERT_BAD_CN).unwrap();

        let err = node_name_from_kubelet_cert(&cert_path).unwrap_err();
        assert!(format!("{err:#}").contains("system:node:"), "{err:#}");
    }

    #[test]
    fn node_name_from_kubelet_cert_errors_on_missing_file() {
        let err = node_name_from_kubelet_cert(Path::new("/nonexistent/kubelet-client-current.pem"))
            .unwrap_err();
        assert!(format!("{err:#}").contains("failed to read"), "{err:#}");
    }

    #[test]
    fn default_node_name_falls_back_to_hostname_when_cert_source_requested_but_unreadable() {
        // DEFAULT_KUBELET_CLIENT_CERT won't exist on a dev/test machine, so
        // requesting KubeletCert must degrade to the same value Hostname
        // would produce, rather than erroring or returning an empty string.
        assert_eq!(
            default_node_name(NodeNameSource::KubeletCert),
            default_node_name(NodeNameSource::Hostname)
        );
    }
}
