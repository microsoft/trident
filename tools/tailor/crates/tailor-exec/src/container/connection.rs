//! Engine selection and connection resolution (`meta/docs/2026-06-29-container-runtimes.md` §3-§5).
//!
//! Everything here is **pure**: it turns the raw inputs the composition root gathers (CLI flags,
//! environment, manifest) into a [`ConnectionPlan`] — *which* engine was declared and *where* to
//! connect — without touching the process environment or `bollard`. The impure connect/preflight
//! step lives in [`super::runtime`].
//!
//! Two orthogonal axes (§3): `engine` is a **selector** (default socket + `auto` probe order +
//! error label); `host` is the **endpoint**. The engine that actually governs runtime behavior is
//! whatever the endpoint reports on connect — see [`detect_engine`] / [`reconcile`].

use tailor_config::Engine;

/// The default rootful Docker socket, used when `engine: docker` has no explicit host.
pub(crate) const DOCKER_DEFAULT_SOCKET: &str = "/var/run/docker.sock";
/// The default rootful Podman socket (rootless is `$XDG_RUNTIME_DIR/podman/podman.sock`).
pub(crate) const PODMAN_ROOTFUL_SOCKET: &str = "/run/podman/podman.sock";

/// One concrete place `bollard` can connect to, classified by URL scheme (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// Nothing was specified for `engine: docker` — keep today's `connect_with_local_defaults()`
    /// path (itself honors `DOCKER_HOST`), so existing Docker users see zero change.
    LocalDefault,
    /// A unix-domain socket path (from `unix://…` or a bare `/path`).
    Unix(String),
    /// A plain `tcp://` / `http://` endpoint.
    Http(String),
    /// An `https://` (TLS) endpoint — recognized but unsupported in v1, pending the rustls remote
    /// milestone (a clear error at connect).
    Https(String),
    /// An `ssh://` remote engine — recognized but unsupported in v1 (a clear error at connect).
    Ssh(String),
}

impl Endpoint {
    /// A human-readable label for error and warning messages (the socket path or URL).
    pub fn label(&self) -> String {
        match self {
            Endpoint::LocalDefault => "the default Docker socket".to_owned(),
            Endpoint::Unix(path) => path.clone(),
            Endpoint::Http(url) | Endpoint::Https(url) | Endpoint::Ssh(url) => url.clone(),
        }
    }
}

/// How to obtain a connection: a single endpoint, or — for `engine: auto` with no pinned host — an
/// ordered probe list whose first reachable member wins (§4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Connect to exactly this endpoint.
    Endpoint(Endpoint),
    /// Try each endpoint in order; the first to answer a handshake wins.
    Probe(Vec<Endpoint>),
}

/// The outcome of [`resolve`]: the declared engine *selector* plus how to connect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionPlan {
    /// The declared engine (`docker` | `podman` | `auto`) — a selector, reconciled against the live
    /// daemon after connecting (see [`reconcile`]).
    pub declared: Engine,
    /// Where/how to connect.
    pub resolution: Resolution,
}

/// Raw inputs to [`resolve`], gathered by the composition root (flags > env > manifest > default).
///
/// Kept as plain options so resolution is a pure, table-testable function — the impure reads
/// (`std::env`, the manifest) happen in the caller.
#[derive(Debug, Clone, Default)]
pub struct ResolveInputs {
    /// `--engine` (highest precedence on the engine axis).
    pub flag_engine: Option<Engine>,
    /// `--host` (highest precedence on the endpoint axis).
    pub flag_host: Option<String>,
    /// `DOCKER_HOST` — applies only when the resolved engine is `docker`.
    pub env_docker_host: Option<String>,
    /// `CONTAINER_HOST` — applies only when the resolved engine is `podman`.
    pub env_container_host: Option<String>,
    /// `runtime.engine` from `tailor.yaml`.
    pub manifest_engine: Option<Engine>,
    /// `runtime.host` from `tailor.yaml`.
    pub manifest_host: Option<String>,
    /// `$XDG_RUNTIME_DIR`, for the rootless Podman default socket.
    pub xdg_runtime_dir: Option<String>,
}

/// Resolve `(engine, host, env)` into a [`ConnectionPlan`] (§3-§4).
///
/// The two axes resolve independently. **engine** = `--engine` → `runtime.engine` → `docker`.
/// **endpoint** = `--host` → the engine's env var → `runtime.host` → (engine `auto` ⇒ probe;
/// otherwise the engine's default socket). `--host` overrides only the endpoint, never the engine.
pub fn resolve(inputs: &ResolveInputs) -> ConnectionPlan {
    let declared = inputs
        .flag_engine
        .or(inputs.manifest_engine)
        .unwrap_or_default();

    // The engine env var only applies to its own engine; for `auto` it folds into the probe list.
    let env_host = match declared {
        Engine::Docker => inputs.env_docker_host.clone(),
        Engine::Podman => inputs.env_container_host.clone(),
        Engine::Auto => None,
    };
    let explicit_host = inputs
        .flag_host
        .clone()
        .or(env_host)
        .or_else(|| inputs.manifest_host.clone());

    let resolution = match (declared, explicit_host) {
        (_, Some(host)) => Resolution::Endpoint(parse_host(&host)),
        (Engine::Docker, None) => Resolution::Endpoint(Endpoint::LocalDefault),
        (Engine::Podman, None) => Resolution::Endpoint(Endpoint::Unix(podman_default_socket(
            inputs.xdg_runtime_dir.as_deref(),
        ))),
        // `auto` with no pinned host probes. An explicit host (`--host` or `runtime.host`) is matched
        // above and pins the endpoint (skipping the probe); under `auto` the engine env vars are not
        // a pin — they seed the probe list instead (see `probe_list`).
        (Engine::Auto, None) => Resolution::Probe(probe_list(inputs)),
    };

    ConnectionPlan {
        declared,
        resolution,
    }
}

/// Classify a host string into an [`Endpoint`] by URL scheme (§4). A bare value (no `://`) is
/// treated as a unix socket path.
pub(crate) fn parse_host(raw: &str) -> Endpoint {
    let host = raw.trim();
    if let Some(rest) = host.strip_prefix("unix://") {
        Endpoint::Unix(rest.to_owned())
    } else if host.starts_with("ssh://") {
        Endpoint::Ssh(host.to_owned())
    } else if host.starts_with("https://") {
        Endpoint::Https(host.to_owned())
    } else if host.starts_with("tcp://") || host.starts_with("http://") {
        Endpoint::Http(host.to_owned())
    } else if host.contains("://") {
        // Unknown scheme — hand the raw value to the HTTP transport as a best effort.
        Endpoint::Http(host.to_owned())
    } else {
        // No scheme: a bare socket path.
        Endpoint::Unix(host.to_owned())
    }
}

/// The default Podman socket: rootless under `$XDG_RUNTIME_DIR` when set, else the rootful socket.
pub(crate) fn podman_default_socket(xdg_runtime_dir: Option<&str>) -> String {
    match xdg_runtime_dir {
        Some(dir) if !dir.is_empty() => format!("{}/podman/podman.sock", dir.trim_end_matches('/')),
        _ => PODMAN_ROOTFUL_SOCKET.to_owned(),
    }
}

/// The ordered `engine: auto` probe list: the Docker socket (or `DOCKER_HOST`), then the rootless
/// and rootful Podman sockets. Duplicates are removed so a redundant env var doesn't probe twice.
pub(crate) fn probe_list(inputs: &ResolveInputs) -> Vec<Endpoint> {
    let mut endpoints = Vec::new();
    endpoints.push(
        inputs
            .env_docker_host
            .as_deref()
            .map_or(Endpoint::Unix(DOCKER_DEFAULT_SOCKET.to_owned()), parse_host),
    );
    if let Some(container_host) = inputs.env_container_host.as_deref() {
        endpoints.push(parse_host(container_host));
    }
    if let Some(dir) = inputs
        .xdg_runtime_dir
        .as_deref()
        .filter(|dir| !dir.is_empty())
    {
        endpoints.push(Endpoint::Unix(podman_default_socket(Some(dir))));
    }
    endpoints.push(Endpoint::Unix(PODMAN_ROOTFUL_SOCKET.to_owned()));

    let mut deduped: Vec<Endpoint> = Vec::with_capacity(endpoints.len());
    for endpoint in endpoints {
        if !deduped.contains(&endpoint) {
            deduped.push(endpoint);
        }
    }
    deduped
}

/// Identify the engine a live endpoint actually is, from its `version` handshake (§5). Podman names
/// itself in the version components (and platform name); everything else is treated as Docker. Never
/// returns [`Engine::Auto`].
pub(crate) fn detect_engine<'a>(
    component_names: impl IntoIterator<Item = &'a str>,
    platform_name: Option<&str>,
) -> Engine {
    let names_say_podman = component_names
        .into_iter()
        .any(|name| name.to_ascii_lowercase().contains("podman"));
    let platform_says_podman =
        platform_name.is_some_and(|name| name.to_ascii_lowercase().contains("podman"));
    if names_say_podman || platform_says_podman {
        Engine::Podman
    } else {
        Engine::Docker
    }
}

/// The reconciliation of a declared engine against the one the endpoint actually reports (§3, §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EngineMatch {
    /// The declaration agreed with — or, for `auto`, deferred to — the live daemon.
    Confirmed(Engine),
    /// The declared engine disagreed with the endpoint. The **endpoint** wins (it is ground truth);
    /// the caller emits a non-fatal warning.
    Mismatch { declared: Engine, actual: Engine },
}

impl EngineMatch {
    /// The engine that governs runtime behavior — always the one the endpoint reports.
    pub(crate) fn effective(self) -> Engine {
        match self {
            EngineMatch::Confirmed(engine) | EngineMatch::Mismatch { actual: engine, .. } => engine,
        }
    }
}

/// Reconcile the declared engine against the detected one (`detected` is never `auto`).
///
/// `auto` adopts whatever was detected. An explicit engine that disagrees is a [`EngineMatch::Mismatch`]:
/// the endpoint is ground truth (you can't impose Podman semantics on a real Docker daemon, or
/// vice-versa), so behavior follows `detected` and the caller warns.
pub(crate) fn reconcile(declared: Engine, detected: Engine) -> EngineMatch {
    match declared {
        Engine::Auto => EngineMatch::Confirmed(detected),
        engine if engine == detected => EngineMatch::Confirmed(detected),
        engine => EngineMatch::Mismatch {
            declared: engine,
            actual: detected,
        },
    }
}

/// The non-fatal warning text for an engine mismatch (§5).
pub(crate) fn mismatch_warning(declared: Engine, actual: Engine, endpoint: &str) -> String {
    format!("--engine {declared}, but {endpoint} reports {actual}; using {actual}")
}

/// Why an engine preflight failed, used to format a specific, actionable message (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreflightError {
    /// The socket file does not exist.
    SocketMissing,
    /// The daemon is not accepting connections (connection refused).
    ConnectionRefused,
    /// The socket exists but the user can't open it.
    PermissionDenied,
    /// Reached the daemon, but its API is too old / incompatible.
    Incompatible,
    /// Any other transport failure.
    Unreachable,
}

/// Format a fail-fast preflight error into a message that names the engine, the endpoint, and a fix
/// (§5). `engine` is the *declared* engine (the detected one is unknown when the handshake fails).
pub(crate) fn preflight_message(kind: PreflightError, engine: Engine, endpoint: &str) -> String {
    match kind {
        PreflightError::SocketMissing => match engine {
            Engine::Podman => format!(
                "podman engine not found: no socket at {endpoint}. Start it with \
                 `systemctl --user start podman.socket` (rootless) or `podman system service`."
            ),
            _ => format!(
                "{engine} engine not found: no socket at {endpoint}. Is the engine installed and started?"
            ),
        },
        PreflightError::ConnectionRefused => format!(
            "cannot reach the {engine} engine at {endpoint} (connection refused). Is the daemon running?"
        ),
        PreflightError::PermissionDenied => format!(
            "permission denied opening {endpoint}. Add your user to the `docker` group, or use rootless Podman."
        ),
        PreflightError::Incompatible => format!(
            "the {engine} engine at {endpoint} responded, but its API is incompatible with tailor."
        ),
        PreflightError::Unreachable => {
            format!("cannot reach the {engine} engine at {endpoint}.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> ResolveInputs {
        ResolveInputs::default()
    }

    // ----- engine axis -----

    #[test]
    fn engine_defaults_to_docker_local() {
        let plan = resolve(&inputs());
        assert_eq!(plan.declared, Engine::Docker);
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::LocalDefault)
        );
    }

    #[test]
    fn flag_engine_beats_manifest_engine() {
        let plan = resolve(&ResolveInputs {
            flag_engine: Some(Engine::Podman),
            manifest_engine: Some(Engine::Docker),
            ..inputs()
        });
        assert_eq!(plan.declared, Engine::Podman);
    }

    #[test]
    fn manifest_engine_used_when_no_flag() {
        let plan = resolve(&ResolveInputs {
            manifest_engine: Some(Engine::Podman),
            ..inputs()
        });
        assert_eq!(plan.declared, Engine::Podman);
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::Unix(PODMAN_ROOTFUL_SOCKET.to_owned()))
        );
    }

    // ----- endpoint axis & precedence -----

    #[test]
    fn flag_host_wins_over_env_and_manifest() {
        let plan = resolve(&ResolveInputs {
            flag_host: Some("unix:///flag.sock".to_owned()),
            env_docker_host: Some("unix:///env.sock".to_owned()),
            manifest_host: Some("unix:///manifest.sock".to_owned()),
            ..inputs()
        });
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::Unix("/flag.sock".to_owned()))
        );
    }

    #[test]
    fn docker_host_env_used_when_no_flag_host() {
        let plan = resolve(&ResolveInputs {
            env_docker_host: Some("tcp://10.0.0.1:2375".to_owned()),
            manifest_host: Some("unix:///manifest.sock".to_owned()),
            ..inputs()
        });
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::Http("tcp://10.0.0.1:2375".to_owned()))
        );
    }

    #[test]
    fn manifest_host_used_when_no_flag_or_env() {
        let plan = resolve(&ResolveInputs {
            manifest_host: Some("/manifest.sock".to_owned()),
            ..inputs()
        });
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::Unix("/manifest.sock".to_owned()))
        );
    }

    #[test]
    fn docker_host_ignored_when_engine_is_podman() {
        // A stray DOCKER_HOST must not hijack `--engine podman`.
        let plan = resolve(&ResolveInputs {
            flag_engine: Some(Engine::Podman),
            env_docker_host: Some("tcp://10.0.0.1:2375".to_owned()),
            xdg_runtime_dir: Some("/run/user/1000".to_owned()),
            ..inputs()
        });
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::Unix(
                "/run/user/1000/podman/podman.sock".to_owned()
            ))
        );
    }

    #[test]
    fn container_host_env_applies_to_podman() {
        let plan = resolve(&ResolveInputs {
            flag_engine: Some(Engine::Podman),
            env_container_host: Some("unix:///run/podman/custom.sock".to_owned()),
            ..inputs()
        });
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::Unix("/run/podman/custom.sock".to_owned()))
        );
    }

    #[test]
    fn podman_rootless_default_uses_xdg_runtime_dir() {
        let plan = resolve(&ResolveInputs {
            flag_engine: Some(Engine::Podman),
            xdg_runtime_dir: Some("/run/user/1000/".to_owned()),
            ..inputs()
        });
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::Unix(
                "/run/user/1000/podman/podman.sock".to_owned()
            ))
        );
    }

    #[test]
    fn podman_rootful_default_without_xdg() {
        assert_eq!(podman_default_socket(None), PODMAN_ROOTFUL_SOCKET);
        assert_eq!(podman_default_socket(Some("")), PODMAN_ROOTFUL_SOCKET);
    }

    // ----- auto / probing -----

    #[test]
    fn auto_with_explicit_host_skips_probing() {
        let plan = resolve(&ResolveInputs {
            flag_engine: Some(Engine::Auto),
            flag_host: Some("unix:///run/podman/podman.sock".to_owned()),
            ..inputs()
        });
        assert_eq!(plan.declared, Engine::Auto);
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::Unix("/run/podman/podman.sock".to_owned()))
        );
    }

    #[test]
    fn auto_with_manifest_host_pins_and_ignores_env() {
        // Under `auto`, an explicit manifest host pins the endpoint (skipping the probe); the engine
        // env vars only seed the probe when no host is pinned, so DOCKER_HOST is not consulted here.
        let plan = resolve(&ResolveInputs {
            manifest_engine: Some(Engine::Auto),
            manifest_host: Some("/run/podman/podman.sock".to_owned()),
            env_docker_host: Some("tcp://ignored:2375".to_owned()),
            ..inputs()
        });
        assert_eq!(
            plan.resolution,
            Resolution::Endpoint(Endpoint::Unix("/run/podman/podman.sock".to_owned()))
        );
    }

    #[test]
    fn auto_without_host_probes_docker_then_podman() {
        let plan = resolve(&ResolveInputs {
            flag_engine: Some(Engine::Auto),
            xdg_runtime_dir: Some("/run/user/1000".to_owned()),
            ..inputs()
        });
        assert_eq!(
            plan.resolution,
            Resolution::Probe(vec![
                Endpoint::Unix(DOCKER_DEFAULT_SOCKET.to_owned()),
                Endpoint::Unix("/run/user/1000/podman/podman.sock".to_owned()),
                Endpoint::Unix(PODMAN_ROOTFUL_SOCKET.to_owned()),
            ])
        );
    }

    #[test]
    fn auto_probe_honors_docker_host_and_dedupes() {
        let plan = resolve(&ResolveInputs {
            flag_engine: Some(Engine::Auto),
            env_docker_host: Some("tcp://dockerhost:2375".to_owned()),
            // No XDG ⇒ rootless podman entry equals the rootful one and must be deduped.
            ..inputs()
        });
        assert_eq!(
            plan.resolution,
            Resolution::Probe(vec![
                Endpoint::Http("tcp://dockerhost:2375".to_owned()),
                Endpoint::Unix(PODMAN_ROOTFUL_SOCKET.to_owned()),
            ])
        );
    }

    // ----- host parsing -----

    #[test]
    fn parse_host_classifies_schemes() {
        assert_eq!(
            parse_host("unix:///run/docker.sock"),
            Endpoint::Unix("/run/docker.sock".to_owned())
        );
        assert_eq!(
            parse_host("/run/docker.sock"),
            Endpoint::Unix("/run/docker.sock".to_owned())
        );
        assert_eq!(
            parse_host("tcp://host:2375"),
            Endpoint::Http("tcp://host:2375".to_owned())
        );
        assert_eq!(
            parse_host("http://host:2375"),
            Endpoint::Http("http://host:2375".to_owned())
        );
        assert_eq!(
            parse_host("https://host:2376"),
            Endpoint::Https("https://host:2376".to_owned())
        );
        assert_eq!(
            parse_host("ssh://user@host"),
            Endpoint::Ssh("ssh://user@host".to_owned())
        );
        assert_eq!(
            parse_host("  unix:///trimmed.sock  "),
            Endpoint::Unix("/trimmed.sock".to_owned())
        );
    }

    // ----- detection -----

    #[test]
    fn detect_engine_spots_podman_in_components() {
        assert_eq!(
            detect_engine(["Podman Engine", "Conmon"], None),
            Engine::Podman
        );
        assert_eq!(detect_engine(["podman"], None), Engine::Podman);
    }

    #[test]
    fn detect_engine_spots_podman_in_platform() {
        assert_eq!(
            detect_engine(std::iter::empty::<&str>(), Some("Podman Engine")),
            Engine::Podman
        );
    }

    #[test]
    fn detect_engine_defaults_to_docker() {
        assert_eq!(
            detect_engine(
                ["Engine", "containerd", "runc"],
                Some("Docker Engine - Community")
            ),
            Engine::Docker
        );
        assert_eq!(
            detect_engine(std::iter::empty::<&str>(), None),
            Engine::Docker
        );
    }

    // ----- reconciliation -----

    #[test]
    fn reconcile_auto_adopts_detected() {
        assert_eq!(
            reconcile(Engine::Auto, Engine::Podman),
            EngineMatch::Confirmed(Engine::Podman)
        );
        assert_eq!(
            reconcile(Engine::Auto, Engine::Docker).effective(),
            Engine::Docker
        );
    }

    #[test]
    fn reconcile_matching_explicit_is_confirmed() {
        assert_eq!(
            reconcile(Engine::Podman, Engine::Podman),
            EngineMatch::Confirmed(Engine::Podman)
        );
    }

    #[test]
    fn reconcile_mismatch_follows_endpoint() {
        // `--engine podman --host /docker.sock`: endpoint is really Docker → effective is Docker.
        let outcome = reconcile(Engine::Podman, Engine::Docker);
        assert_eq!(
            outcome,
            EngineMatch::Mismatch {
                declared: Engine::Podman,
                actual: Engine::Docker
            }
        );
        assert_eq!(outcome.effective(), Engine::Docker);
    }

    #[test]
    fn mismatch_warning_names_both_engines() {
        let warning = mismatch_warning(Engine::Podman, Engine::Docker, "/var/run/docker.sock");
        assert_eq!(
            warning,
            "--engine podman, but /var/run/docker.sock reports docker; using docker"
        );
    }

    // ----- preflight messages -----

    #[test]
    fn preflight_message_is_engine_specific() {
        let podman = preflight_message(
            PreflightError::SocketMissing,
            Engine::Podman,
            "/run/user/1000/podman/podman.sock",
        );
        assert!(podman.contains("podman engine not found"));
        assert!(podman.contains("podman system service"));

        let docker = preflight_message(PreflightError::ConnectionRefused, Engine::Docker, "/sock");
        assert!(docker.contains("connection refused"));
        assert!(docker.contains("docker"));

        let denied = preflight_message(PreflightError::PermissionDenied, Engine::Docker, "/sock");
        assert!(denied.contains("permission denied"));
    }
}
