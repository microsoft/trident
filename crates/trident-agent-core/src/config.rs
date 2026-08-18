//! Config primitives shared by both agent binaries: env-var parsing helpers
//! and the `NebraskaConfig`/`TridentConfig` sections that both
//! `trident-acl-agent` and `trident-aks-agent` load. Each binary defines its
//! own top-level `AgentConfig` (different env-var prefix, different set of
//! sections) in its own `config` module and reuses these building blocks.

use std::{env, str::FromStr, time::Duration};

use url::Url;

// TODO: placeholder until the real production Nebraska/Omaha endpoint is
// known. `.invalid` is reserved by RFC 2606 and is guaranteed to never
// resolve, so a deployment that forgets to configure a real endpoint (or
// override it per-request, where supported) fails loudly at the network
// layer instead of silently querying a real-looking but wrong host.
pub const DEFAULT_NEBRASKA_ENDPOINT: &str = "https://nebraska.example.invalid/v1/update";

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
            app_id: crate::DEFAULT_NEBRASKA_APP_ID.to_string(),
            track: crate::DEFAULT_NEBRASKA_TRACK.to_string(),
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

/// Reads `name`, treating both "unset" and "set to the empty string" as
/// absent - a drop-in override that clears a variable to `""` should fall
/// back to the default, not try to parse an empty value.
pub fn env_raw(name: &str) -> Option<String> {
    env::var(name).ok().filter(|v| !v.is_empty())
}

pub fn env_string(name: &str) -> Option<String> {
    env_raw(name)
}

pub fn env_url(name: &str) -> Result<Option<Url>, anyhow::Error> {
    env_raw(name)
        .map(|v| Url::parse(&v).map_err(|err| anyhow::anyhow!("invalid URL for {name}: {err}")))
        .transpose()
}

pub fn env_duration(name: &str, default: Duration) -> Result<Duration, anyhow::Error> {
    env_raw(name)
        .map(|v| {
            humantime::parse_duration(&v)
                .map_err(|err| anyhow::anyhow!("invalid duration for {name}: {err}"))
        })
        .transpose()
        .map(|parsed| parsed.unwrap_or(default))
}

pub fn env_parse<T>(name: &str) -> Result<Option<T>, anyhow::Error>
where
    T: FromStr<Err = anyhow::Error>,
{
    env_raw(name).map(|v| v.parse::<T>()).transpose()
}
