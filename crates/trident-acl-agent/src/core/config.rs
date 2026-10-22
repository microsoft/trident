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

use std::{fmt, path::PathBuf, str::FromStr, time::Duration};

use anyhow::{anyhow, Context, Error};
use const_format::formatcp;
use serde::{de::Error as _, Deserialize, Deserializer};
use trident_proto::TRIDENT_DEFAULT_SOCKET_URI;
use url::Url;

use crate::{cosi_sha384::CosiSha384Source, DEFAULT_NEBRASKA_APP_ID, DEFAULT_NEBRASKA_TRACK};

const ENV_PREFIX_NEBRASKA: &str = "TRIDENT_ACL_AGENT_NEBRASKA_";
const ENV_PREFIX_COSI_SHA384: &str = "TRIDENT_ACL_AGENT_COSI_SHA384_";
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
/// File name for the persisted Nebraska affinity cookie (see
/// `core::nebraska::CookieStore`). Deliberately a separate file from
/// `STATE_FILE_NAME`: the cookie is a host/connection-level attribute with
/// no relationship to any update operation, unlike everything else in
/// `state.json`.
pub const NEBRASKA_COOKIE_FILE_NAME: &str = "nebraska-cookie";
pub const DEFAULT_NEBRASKA_COOKIE_PATH: &str =
    formatcp!("/var/lib/trident-acl-agent/{NEBRASKA_COOKIE_FILE_NAME}");
pub const DEFAULT_KUBELET_KUBECONFIG: &str = "/var/lib/kubelet/kubeconfig";
/// Default annotation-key prefix.
/// Override with `TRIDENT_ACL_AGENT_KUBERNETES_ANNOTATION_PREFIX`.
pub const DEFAULT_ANNOTATION_PREFIX: &str = "acl.microsoft.com";
/// Cookies are opt-in.
/// Override with `TRIDENT_ACL_AGENT_ORCHESTRATION_COOKIE_ENABLED`.
const DEFAULT_COOKIE_ENABLED: bool = false;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentConfig {
    pub nebraska: NebraskaConfig,
    pub cosi_sha384: CosiSha384Config,
    pub kubernetes: KubernetesConfig,
    pub trident: TridentConfig,
    pub orchestration: OrchestrationConfig,
}

impl AgentConfig {
    /// A logging view of the effective configuration. Network URL credentials,
    /// paths, queries, and fragments are omitted; kubeconfig contents are never read.
    pub fn redacted_for_logging(&self) -> impl fmt::Debug + '_ {
        RedactedAgentConfig(self)
    }

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
        let cosi_sha384: RawCosiSha384Config = envy::prefixed(ENV_PREFIX_COSI_SHA384)
            .from_iter(vars.iter().cloned())
            .with_context(|| format!("invalid {ENV_PREFIX_COSI_SHA384}* environment variable"))?;
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
            cosi_sha384: CosiSha384Config {
                source: cosi_sha384.source.unwrap_or_default(),
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
                cookie_path: orchestration
                    .cookie_path
                    .unwrap_or_else(|| PathBuf::from(DEFAULT_NEBRASKA_COOKIE_PATH)),
                cookie_enabled: orchestration
                    .cookie_enabled
                    .unwrap_or(DEFAULT_COOKIE_ENABLED),
                cookie_lifetime: orchestration.cookie_lifetime.unwrap_or_default(),
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

struct RedactedAgentConfig<'a>(&'a AgentConfig);

impl fmt::Debug for RedactedAgentConfig<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let config = self.0;
        f.debug_struct("AgentConfig")
            .field(
                "nebraska.endpoint",
                &config
                    .nebraska
                    .endpoint
                    .as_ref()
                    .map(super::nebraska::redacted),
            )
            .field("nebraska.app_id", &config.nebraska.app_id)
            .field("nebraska.track", &config.nebraska.track)
            .field("cosi_sha384", &config.cosi_sha384)
            .field(
                "kubernetes.api_server",
                &config
                    .kubernetes
                    .api_server
                    .as_ref()
                    .map(super::nebraska::redacted),
            )
            .field("kubernetes.kubeconfig", &config.kubernetes.kubeconfig)
            .field("kubernetes.node_name", &config.kubernetes.node_name)
            .field(
                "kubernetes.watch_poll_interval",
                &config.kubernetes.watch_poll_interval,
            )
            .field(
                "kubernetes.annotation_prefix",
                &config.kubernetes.annotation_prefix,
            )
            .field(
                "trident.socket",
                &super::trident::redacted_socket(&config.trident.socket),
            )
            .field("orchestration", &config.orchestration)
            .finish()
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

/// Mirrors [`CosiSha384Config`] (see [`RawNebraskaConfig`]).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawCosiSha384Config {
    #[serde(deserialize_with = "empty_cosi_sha384_source_as_none")]
    source: Option<CosiSha384Source>,
}

fn empty_cosi_sha384_source_as_none<'de, D>(
    deserializer: D,
) -> Result<Option<CosiSha384Source>, D::Error>
where
    D: Deserializer<'de>,
{
    empty_as_none(String::deserialize(deserializer)?)
        .map(|value| value.parse::<CosiSha384Source>().map_err(D::Error::custom))
        .transpose()
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
    #[serde(deserialize_with = "empty_path_as_none")]
    cookie_path: Option<PathBuf>,
    #[serde(deserialize_with = "empty_bool_as_none")]
    cookie_enabled: Option<bool>,
    #[serde(deserialize_with = "empty_cookie_lifetime_as_none")]
    cookie_lifetime: Option<CookieLifetime>,
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

/// Parses `"true"`/`"false"` (Rust's `bool::from_str`, case-sensitive
/// lowercase only), treating the empty string as unset like every other
/// `empty_*_as_none` helper here.
fn empty_bool_as_none<'de, D>(deserializer: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    empty_as_none(String::deserialize(deserializer)?)
        .map(|value| {
            value
                .parse::<bool>()
                .map_err(|err| D::Error::custom(format!("invalid boolean {value:?}: {err}")))
        })
        .transpose()
}

fn empty_cookie_lifetime_as_none<'de, D>(
    deserializer: D,
) -> Result<Option<CookieLifetime>, D::Error>
where
    D: Deserializer<'de>,
{
    empty_as_none(String::deserialize(deserializer)?)
        .map(|value| value.parse::<CookieLifetime>().map_err(D::Error::custom))
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

/// Which source resolves a package's COSI metadata SHA-384 (see
/// `crate::cosi_sha384`). A separate section from [`NebraskaConfig`]: this
/// setting governs how agent-side COSI logic interprets Nebraska's response,
/// not the Nebraska protocol itself.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CosiSha384Config {
    pub source: CosiSha384Source,
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
}

impl Default for KubernetesConfig {
    fn default() -> Self {
        Self {
            api_server: None,
            kubeconfig: PathBuf::from(DEFAULT_KUBELET_KUBECONFIG),
            node_name: default_node_name(),
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

/// Controls whether a persisted Nebraska affinity cookie is reused across
/// staging attempts, or reset before each attempt's initial check. See
/// [`OrchestrationConfig::cookie_lifetime`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CookieLifetime {
    /// Reuse a persisted cookie across every update until Nebraska itself
    /// replaces it. This is the originally-shipped behavior.
    #[default]
    Forever,
    /// Start each staging attempt without a cookie: clear persisted
    /// affinity before `handle_stage`'s initial Nebraska check, then persist
    /// and reuse any cookie it establishes for progress, finalize, and
    /// post-reboot completion.
    ///
    /// Restarting an unfinished stage starts a new attempt, even when the
    /// target version, `nodeUpdateId`, and `operationId` are unchanged.
    /// Its initial check also starts cookie-less; affinity is not preserved
    /// across staging attempts. Resuming finalize/commit does not reset it.
    PerUpdate,
}

impl FromStr for CookieLifetime {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "forever" => Ok(CookieLifetime::Forever),
            "per-update" => Ok(CookieLifetime::PerUpdate),
            other => Err(anyhow!(
                "unknown cookie lifetime {other:?} (expected \"forever\" or \"per-update\")"
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestrationConfig {
    pub mode: Mode,
    pub state_path: PathBuf,
    /// Path to the persisted Nebraska affinity cookie (see
    /// `core::nebraska::CookieStore`). Deliberately a separate file/field
    /// from `state_path`: the cookie has no relationship to any particular
    /// update operation, unlike everything `state_path` persists.
    pub cookie_path: PathBuf,
    /// Blanket kill switch for the whole cookie mechanism. When `false`, no
    /// `Cookie:` header is ever sent and no `Set-Cookie` is ever adopted -
    /// the persisted file (if one exists from before the toggle was
    /// flipped) is never read *or* written, so toggling this back to `true`
    /// later just picks the stale file back up as-is. Override with
    /// `TRIDENT_ACL_AGENT_ORCHESTRATION_COOKIE_ENABLED`.
    pub cookie_enabled: bool,
    /// Whether a persisted cookie is reused forever, or deliberately
    /// reset before every staging attempt. See [`CookieLifetime`].
    /// Override with `TRIDENT_ACL_AGENT_ORCHESTRATION_COOKIE_LIFETIME`
    /// (`"forever"` or `"per-update"`).
    pub cookie_lifetime: CookieLifetime,
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
            cookie_path: PathBuf::from(DEFAULT_NEBRASKA_COOKIE_PATH),
            cookie_enabled: DEFAULT_COOKIE_ENABLED,
            cookie_lifetime: CookieLifetime::Forever,
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
    fn startup_summary_includes_effective_defaults() {
        let config = AgentConfig::from_vars(vec![]).unwrap();
        let summary = format!("{:?}", config.redacted_for_logging());
        for expected in [
            "AgentConfig",
            "nebraska.endpoint",
            "https://nebraska.example.invalid/<redacted>",
            "nebraska.app_id",
            "nebraska.track",
            "source: Labels",
            "kubernetes.api_server: None",
            DEFAULT_KUBELET_KUBECONFIG,
            "kubernetes.node_name",
            "watch_poll_interval",
            DEFAULT_ANNOTATION_PREFIX,
            TRIDENT_DEFAULT_SOCKET_URI,
            "mode: Annotations",
            DEFAULT_STATE_PATH,
            "stage_timeout",
            "finalize_timeout",
            "heartbeat_interval",
            "node_gone_max_wait",
        ] {
            assert!(summary.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn startup_summary_uses_env_overrides_without_exposing_network_secrets() {
        let config = AgentConfig::from_vars(vars(&[
            ("TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT", "https://nebraska-user:nebraska-password@nebraska.example:8443/secret-path?token=secret-query#secret-fragment"),
            ("TRIDENT_ACL_AGENT_NEBRASKA_APP_ID", "custom-app"),
            ("TRIDENT_ACL_AGENT_NEBRASKA_TRACK", "custom-track"),
            ("TRIDENT_ACL_AGENT_COSI_SHA384_SOURCE", "hash"),
            ("TRIDENT_ACL_AGENT_KUBERNETES_API_SERVER", "https://kube-user:kube-password@cluster.example/kube-secret-path?token=kube-secret-query#kube-secret-fragment"),
            ("TRIDENT_ACL_AGENT_KUBERNETES_NODE_NAME", "node-42"),
            ("TRIDENT_ACL_AGENT_TRIDENT_SOCKET", "https://socket-user:socket-password@trident.example/socket-secret-path?token=socket-secret-query#socket-secret-fragment"),
            ("TRIDENT_ACL_AGENT_ORCHESTRATION_STAGE_TIMEOUT", "21m"),
        ])).unwrap();
        let original = config.clone();
        let summary = format!("{:?}", config.redacted_for_logging());
        for expected in [
            "https://nebraska.example:8443/<redacted>",
            "https://cluster.example/<redacted>",
            "https://trident.example/<redacted>",
            "custom-app",
            "custom-track",
            "source: Hash",
            "node-42",
            "stage_timeout: 1260s",
        ] {
            assert!(summary.contains(expected), "missing {expected}");
        }
        for secret in [
            "nebraska-user",
            "nebraska-password",
            "secret-path",
            "secret-query",
            "secret-fragment",
            "kube-user",
            "kube-password",
            "socket-user",
            "socket-password",
        ] {
            assert!(
                !summary.contains(secret),
                "startup summary contains a sensitive URL component"
            );
        }
        assert!(
            config == original,
            "formatting the startup summary changed the configuration"
        );
    }

    #[test]
    fn startup_summary_redacts_unix_socket_query_and_invalid_socket() {
        let mut config = AgentConfig::default();
        config.trident.socket =
            "unix:///run/trident/custom.sock?secret=socket-token#fragment".to_string();
        let summary = format!("{:?}", config.redacted_for_logging());
        assert!(summary.contains("unix:///run/trident/custom.sock"));
        assert!(!summary.contains("socket-token"));
        assert!(!summary.contains("fragment"));
        config.trident.socket = "invalid-socket-secret".to_string();
        let summary = format!("{:?}", config.redacted_for_logging());
        assert!(summary.contains("<invalid URI>"));
        assert!(!summary.contains("invalid-socket-secret"));
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
        assert_eq!(config.cosi_sha384.source, CosiSha384Source::Labels);
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
        assert_eq!(
            config.orchestration.cookie_path,
            PathBuf::from(DEFAULT_NEBRASKA_COOKIE_PATH)
        );
        assert!(
            !config.orchestration.cookie_enabled,
            "cookie_enabled should default to off"
        );
        assert_eq!(
            config.orchestration.cookie_lifetime,
            CookieLifetime::Forever
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
            ("TRIDENT_ACL_AGENT_COSI_SHA384_SOURCE", "hash"),
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
            (
                "TRIDENT_ACL_AGENT_ORCHESTRATION_COOKIE_PATH",
                "/var/lib/trident-acl-agent/custom-nebraska-cookie",
            ),
            ("TRIDENT_ACL_AGENT_ORCHESTRATION_COOKIE_ENABLED", "true"),
            (
                "TRIDENT_ACL_AGENT_ORCHESTRATION_COOKIE_LIFETIME",
                "per-update",
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
        assert_eq!(config.cosi_sha384.source, CosiSha384Source::Hash);
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
            config.orchestration.cookie_path,
            PathBuf::from("/var/lib/trident-acl-agent/custom-nebraska-cookie")
        );
        assert!(config.orchestration.cookie_enabled);
        assert_eq!(
            config.orchestration.cookie_lifetime,
            CookieLifetime::PerUpdate
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
    fn cookie_enabled_is_opt_in() {
        assert!(!AgentConfig::default().orchestration.cookie_enabled);
        for (value, expected) in [("", false), ("false", false), ("true", true)] {
            let config = AgentConfig::from_vars(vars(&[(
                "TRIDENT_ACL_AGENT_ORCHESTRATION_COOKIE_ENABLED",
                value,
            )]))
            .unwrap();
            assert_eq!(
                config.orchestration.cookie_enabled, expected,
                "cookie_enabled with value {value:?}"
            );
        }
    }

    #[test]
    fn malformed_cookie_enabled_is_a_parse_error() {
        let err = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_ORCHESTRATION_COOKIE_ENABLED",
            "yes please",
        )]))
        .unwrap_err();
        assert!(format!("{err:#}").contains("yes please"), "{err:#}");
    }

    #[test]
    fn malformed_cookie_lifetime_is_a_parse_error() {
        let err = AgentConfig::from_vars(vars(&[(
            "TRIDENT_ACL_AGENT_ORCHESTRATION_COOKIE_LIFETIME",
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
