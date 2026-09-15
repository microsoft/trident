use std::{io, path::Path, pin::Pin};

use bollard::{
    API_DEFAULT_VERSION, Docker,
    container::{
        AttachContainerOptions, Config as DockerConfig, CreateContainerOptions,
        RemoveContainerOptions, WaitContainerOptions,
    },
    errors::Error as BollardError,
    image::CreateImageOptions,
    models::HostConfig,
};
use futures_util::{StreamExt, TryStreamExt};
use tokio::{select, task::JoinHandle};
use tokio_util::{
    io::{StreamReader, SyncIoBridge},
    sync::CancellationToken,
};
use tracing::{debug, warn};

use tailor_config::Engine;
use tailor_core::{
    ContainerConfig, ContainerResult, ContainerRuntime, DaemonInfo, ExecError, LocalImage,
    LogSource,
};

use crate::ic_log::{self, IcCapture};

use super::connection::{self, ConnectionPlan, Endpoint, EngineMatch, PreflightError, Resolution};

const ATTACH_LOGS: bool = true;
const WAIT_CONDITION_NOT_RUNNING: &str = "not-running";
/// Connection timeout (seconds) for explicit unix/http endpoints.
const CONNECT_TIMEOUT_SECS: u64 = 120;
const EXPORT_NAME_PREFIX: &str = "tailor-tools-dir";

#[derive(Debug, Clone)]
pub struct BollardRuntime {
    docker: Docker,
}

impl BollardRuntime {
    /// Connect to the resolved engine and **fail fast** if it is missing or unreachable, then
    /// reconcile the declared engine against the live daemon (`meta/docs/2026-06-29-container-runtimes.md`
    /// §4-§5). Run once before any build/clean.
    pub async fn establish(plan: &ConnectionPlan) -> Result<Self, ExecError> {
        match &plan.resolution {
            Resolution::Endpoint(endpoint) => {
                let label = endpoint.label();
                let runtime = Self::build_client(endpoint, plan.declared)?;
                runtime.verify(plan.declared, &label).await?;
                Ok(runtime)
            }
            Resolution::Probe(endpoints) => Self::probe(endpoints).await,
        }
    }

    /// Build a `bollard` client for one endpoint by scheme (§4). For a unix socket this also checks
    /// the socket file exists (bollard returns `SocketNotFoundError`), the first fail-fast gate.
    fn build_client(endpoint: &Endpoint, declared: Engine) -> Result<Self, ExecError> {
        let connected = match endpoint {
            Endpoint::LocalDefault => Docker::connect_with_local_defaults(),
            Endpoint::Unix(path) => {
                Docker::connect_with_unix(path, CONNECT_TIMEOUT_SECS, API_DEFAULT_VERSION)
            }
            Endpoint::Http(url) => {
                Docker::connect_with_http(url, CONNECT_TIMEOUT_SECS, API_DEFAULT_VERSION)
            }
            Endpoint::Https(url) => {
                return Err(ExecError::Runtime(format!(
                    "TLS remote engines are not supported yet ({url}); use a local socket or a plain tcp:// endpoint"
                )));
            }
            Endpoint::Ssh(url) => {
                return Err(ExecError::Runtime(format!(
                    "ssh remote engines are not supported yet ({url}); use a local socket or a tcp:// endpoint"
                )));
            }
        };
        connected.map(|docker| Self { docker }).map_err(|err| {
            ExecError::Runtime(connection::preflight_message(
                classify(&err),
                declared,
                &endpoint.label(),
            ))
        })
    }

    /// `engine: auto` — try each candidate in order, the first to answer a `version` handshake wins
    /// (§4). If none answer, fail fast listing every endpoint tried.
    async fn probe(endpoints: &[Endpoint]) -> Result<Self, ExecError> {
        let mut tried = Vec::new();
        for endpoint in endpoints {
            let label = endpoint.label();
            let Ok(runtime) = Self::build_client(endpoint, Engine::Auto) else {
                tried.push(label);
                continue;
            };
            if runtime.verify(Engine::Auto, &label).await.is_ok() {
                debug!(endpoint = %label, "engine auto-probe selected");
                return Ok(runtime);
            }
            tried.push(label);
        }
        Err(ExecError::Runtime(format!(
            "no container engine reachable — tried {}. Set runtime.host / --host or start an engine.",
            tried.join(", ")
        )))
    }

    /// Ping the daemon (`version`) to confirm reachability — the fail-fast round-trip — and warn if
    /// the declared engine disagrees with the one the endpoint actually reports (§5).
    async fn verify(&self, declared: Engine, label: &str) -> Result<(), ExecError> {
        let version = self.docker.version().await.map_err(|err| {
            ExecError::Runtime(connection::preflight_message(
                classify(&err),
                declared,
                label,
            ))
        })?;
        let names: Vec<&str> = version
            .components
            .iter()
            .flatten()
            .map(|component| component.name.as_str())
            .collect();
        let platform = version
            .platform
            .as_ref()
            .map(|platform| platform.name.as_str());
        let outcome = connection::reconcile(
            declared,
            connection::detect_engine(names.iter().copied(), platform),
        );
        if let EngineMatch::Mismatch { declared, actual } = outcome {
            warn!("{}", connection::mismatch_warning(declared, actual, label));
        }
        debug!(engine = %outcome.effective(), endpoint = label, "engine preflight ok");
        Ok(())
    }
}

impl ContainerRuntime for BollardRuntime {
    async fn pull_image(&self, reference: &str) -> Result<(), ExecError> {
        self.docker
            .create_image(
                Some(CreateImageOptions {
                    from_image: reference,
                    ..Default::default()
                }),
                None,
                None,
            )
            .try_collect::<Vec<_>>()
            .await
            .map(|_| ())
            .map_err(|err| map_runtime_error(&err))
    }

    async fn inspect_image(&self, reference: &str) -> Result<Option<LocalImage>, ExecError> {
        match self.docker.inspect_image(reference).await {
            Ok(image) => Ok(Some(LocalImage {
                id: image.id.unwrap_or_default(),
                repo_digests: image.repo_digests.unwrap_or_default(),
                architecture: image.architecture.unwrap_or_default(),
                os: image.os.unwrap_or_default(),
            })),
            Err(BollardError::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(err) => Err(map_runtime_error(&err)),
        }
    }

    async fn create_and_run(
        &self,
        config: ContainerConfig,
        cancel: CancellationToken,
    ) -> Result<ContainerResult, ExecError> {
        let name = config.name.clone();
        let cell_slug = config.cell_slug.clone();
        let log_source = config.log_source;
        let log_file = config.log_file.clone();
        let docker_config = DockerConfig {
            image: Some(config.image_ref),
            cmd: Some(config.args),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            host_config: Some(HostConfig {
                binds: Some(config.binds),
                privileged: Some(config.privileged),
                ..Default::default()
            }),
            ..Default::default()
        };
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: name.clone(),
                    platform: Some(config.platform),
                }),
                docker_config,
            )
            .await
            .map_err(|err| map_runtime_error(&err))?;

        let attach = self
            .docker
            .attach_container(
                &name,
                Some(AttachContainerOptions::<String> {
                    stdout: Some(ATTACH_LOGS),
                    stderr: Some(ATTACH_LOGS),
                    stream: Some(ATTACH_LOGS),
                    logs: Some(ATTACH_LOGS),
                    ..Default::default()
                }),
            )
            .await
            .map_err(|err| map_runtime_error(&err))?;
        let log_task = stream_logs(attach.output, cell_slug, log_source);

        self.docker
            .start_container::<String>(&name, None)
            .await
            .map_err(|err| map_runtime_error(&err))?;

        let wait = wait_for_container(self.docker.clone(), name.clone());
        let exit_code = select! {
            result = wait => result?,
            () = cancel.cancelled() => {
                remove_container(&self.docker, &name).await?;
                return Err(ExecError::Cancelled);
            }
        };
        remove_container(&self.docker, &name).await?;
        let capture = log_task
            .await
            .map_err(|err| ExecError::Runtime(err.to_string()))?;
        let failure_dump =
            (exit_code != 0).then(|| capture.failure_dump(exit_code, log_file.as_deref()));
        Ok(ContainerResult {
            exit_code,
            logs: capture.joined(),
            failure_dump,
        })
    }

    async fn export_container(
        &self,
        image_ref: &str,
        platform: &str,
        pull: bool,
        dest_dir: &Path,
        cancel: CancellationToken,
    ) -> Result<(), ExecError> {
        let name = format!("{EXPORT_NAME_PREFIX}-{}", std::process::id());
        if pull {
            self.pull_image(image_ref).await?;
        }
        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: name.clone(),
                    platform: Some(platform.to_owned()),
                }),
                DockerConfig {
                    image: Some(image_ref.to_owned()),
                    ..Default::default()
                },
            )
            .await
            .map_err(|err| map_runtime_error(&err))?;

        let stream = self
            .docker
            .export_container(&name)
            .map_err(|err| io::Error::other(err.to_string()));
        let reader = StreamReader::new(stream);
        let dest = dest_dir.to_path_buf();
        let unpack = tokio::task::spawn_blocking(move || {
            let mut bridge = SyncIoBridge::new(reader);
            let mut archive = tar::Archive::new(&mut bridge);
            archive.set_preserve_ownerships(false);
            archive.unpack(dest)
        });
        let result = select! {
            result = unpack => result.map_err(|err| ExecError::Runtime(err.to_string())),
            () = cancel.cancelled() => {
                remove_container(&self.docker, &name).await?;
                return Err(ExecError::Cancelled);
            }
        };
        remove_container(&self.docker, &name).await?;
        result?.map_err(|source| ExecError::Io {
            context: format!(
                "failed to export tools-dir container `{image_ref}` to `{}`",
                dest_dir.display()
            ),
            source,
        })
    }

    async fn daemon_info(&self) -> Result<DaemonInfo, ExecError> {
        let info = self
            .docker
            .info()
            .await
            .map_err(|err| map_runtime_error(&err))?;
        Ok(parse_daemon_info(
            &info.security_options.unwrap_or_default(),
        ))
    }
}

/// A [`ContainerRuntime`] that never connects to an engine — the stand-in for `build --dry-run`,
/// which renders container invocations without contacting any daemon (`meta/docs/2026-06-29-container-runtimes.md`
/// §4). Dry-run execution returns before touching the runtime, so these methods are never reached; if
/// one ever is, it fails loudly instead of silently connecting.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopRuntime;

impl ContainerRuntime for NoopRuntime {
    async fn pull_image(&self, _reference: &str) -> Result<(), ExecError> {
        Err(no_engine())
    }

    async fn inspect_image(&self, _reference: &str) -> Result<Option<LocalImage>, ExecError> {
        Err(no_engine())
    }

    async fn create_and_run(
        &self,
        _config: ContainerConfig,
        _cancel: CancellationToken,
    ) -> Result<ContainerResult, ExecError> {
        Err(no_engine())
    }

    async fn daemon_info(&self) -> Result<DaemonInfo, ExecError> {
        Err(no_engine())
    }

    async fn export_container(
        &self,
        _image_ref: &str,
        _platform: &str,
        _pull: bool,
        _dest_dir: &Path,
        _cancel: CancellationToken,
    ) -> Result<(), ExecError> {
        Err(no_engine())
    }
}

fn no_engine() -> ExecError {
    ExecError::Runtime("no container engine connected (dry-run runtime)".to_owned())
}

/// Parse the daemon `info.security_options` into the ownership-relevant flags (§6).
///
/// Docker advertises `name=rootless` / `name=userns`; Podman's compat `info` likewise reports
/// `rootless` here, so the substring match covers both. (Podman rootless detection is flagged as an
/// open question in `meta/docs/2026-06-29-container-runtimes.md` §7.1 pending end-to-end validation.)
fn parse_daemon_info(security_options: &[String]) -> DaemonInfo {
    DaemonInfo {
        rootless: security_options
            .iter()
            .any(|item| item.contains("rootless")),
        userns_remap: security_options.iter().any(|item| item.contains("userns")),
    }
}

/// Classify a `bollard` connect/handshake failure into a fail-fast [`PreflightError`] (§5).
fn classify(err: &BollardError) -> PreflightError {
    match err {
        BollardError::SocketNotFoundError(_) => PreflightError::SocketMissing,
        BollardError::IOError { err } => match err.kind() {
            io::ErrorKind::NotFound => PreflightError::SocketMissing,
            io::ErrorKind::ConnectionRefused => PreflightError::ConnectionRefused,
            io::ErrorKind::PermissionDenied => PreflightError::PermissionDenied,
            _ => PreflightError::Unreachable,
        },
        BollardError::DockerResponseServerError { status_code, .. }
            if *status_code == 400 || *status_code == 426 =>
        {
            PreflightError::Incompatible
        }
        _ => PreflightError::Unreachable,
    }
}

async fn wait_for_container(docker: Docker, name: String) -> Result<i64, ExecError> {
    let mut stream = docker.wait_container(
        &name,
        Some(WaitContainerOptions {
            condition: WAIT_CONDITION_NOT_RUNNING,
        }),
    );
    match stream.next().await {
        Some(Ok(response)) => Ok(response.status_code),
        Some(Err(BollardError::DockerContainerWaitError { code, .. })) => Ok(code),
        Some(Err(err)) => Err(map_runtime_error(&err)),
        None => Err(ExecError::Runtime(
            "container wait stream ended without a status".to_owned(),
        )),
    }
}

fn stream_logs(
    mut output: Pin<
        Box<
            dyn futures_util::Stream<Item = Result<bollard::container::LogOutput, BollardError>>
                + Send,
        >,
    >,
    cell_slug: String,
    log_source: LogSource,
) -> JoinHandle<IcCapture> {
    tokio::spawn(async move {
        let mut capture = IcCapture::default();
        // IC (logrus) emits one JSON object per newline-terminated line, but bollard hands back
        // arbitrary chunks: buffer bytes and split on '\n' so a parse always sees a whole line and a
        // line split across chunks is reassembled (`meta/docs/2026-06-29-logging.md` §5.3).
        let mut pending = String::new();
        while let Some(item) = output.next().await {
            match item {
                Ok(chunk) => {
                    pending.push_str(&chunk.to_string());
                    while let Some(newline) = pending.find('\n') {
                        let line: String = pending.drain(..=newline).collect();
                        ingest(&line, &cell_slug, log_source, &mut capture);
                    }
                }
                Err(err) => {
                    let line = format!("container log stream error: {err}");
                    ingest(&line, &cell_slug, log_source, &mut capture);
                }
            }
        }
        // Flush any trailing line without a final newline.
        if !pending.is_empty() {
            ingest(&pending, &cell_slug, log_source, &mut capture);
        }
        capture
    })
}

/// Parse one IC output line, re-emit it live as a `tracing` event, and keep it in the capture. Blank
/// lines are skipped so they neither clutter the live view nor pad the capture.
fn ingest(line: &str, cell_slug: &str, log_source: LogSource, capture: &mut IcCapture) {
    let trimmed = line.trim_end_matches(['\n', '\r']);
    if trimmed.is_empty() {
        return;
    }
    let parsed = ic_log::parse_ic_line(trimmed);
    ic_log::emit(&parsed, cell_slug, log_source);
    capture.push(parsed);
}

async fn remove_container(docker: &Docker, name: &str) -> Result<(), ExecError> {
    docker
        .remove_container(
            name,
            Some(RemoveContainerOptions {
                force: true,
                ..Default::default()
            }),
        )
        .await
        .map_err(|err| map_runtime_error(&err))
}

fn map_runtime_error(err: &BollardError) -> ExecError {
    ExecError::Runtime(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_daemon_info_detects_docker_rootless_and_userns() {
        let options = vec!["name=seccomp".to_owned(), "name=rootless".to_owned()];
        let info = parse_daemon_info(&options);
        assert!(info.rootless);
        assert!(!info.userns_remap);

        let remap = parse_daemon_info(&["name=userns".to_owned()]);
        assert!(remap.userns_remap);
        assert!(!remap.rootless);
    }

    #[test]
    fn parse_daemon_info_detects_podman_rootless_token() {
        // Podman's compat `info` surfaces a bare `rootless` token in security options.
        let info = parse_daemon_info(&["rootless".to_owned()]);
        assert!(info.rootless);
    }

    #[test]
    fn parse_daemon_info_empty_is_rootful() {
        let info = parse_daemon_info(&[]);
        assert!(!info.rootless);
        assert!(!info.userns_remap);
    }

    #[test]
    fn classify_maps_missing_socket_and_io_errors() {
        assert_eq!(
            classify(&BollardError::SocketNotFoundError("/x.sock".to_owned())),
            PreflightError::SocketMissing
        );
        assert_eq!(
            classify(&BollardError::IOError {
                err: io::Error::from(io::ErrorKind::ConnectionRefused),
            }),
            PreflightError::ConnectionRefused
        );
        assert_eq!(
            classify(&BollardError::IOError {
                err: io::Error::from(io::ErrorKind::PermissionDenied),
            }),
            PreflightError::PermissionDenied
        );
        assert_eq!(
            classify(&BollardError::IOError {
                err: io::Error::from(io::ErrorKind::NotFound),
            }),
            PreflightError::SocketMissing
        );
    }

    #[test]
    fn classify_maps_incompatible_api_response() {
        assert_eq!(
            classify(&BollardError::DockerResponseServerError {
                status_code: 426,
                message: "client too old".to_owned(),
            }),
            PreflightError::Incompatible
        );
    }
}
