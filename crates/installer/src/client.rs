use std::borrow::Cow;

use anyhow::{anyhow, bail, Error};
use log::Level;
use tokio::{sync::mpsc::Sender, task};

use trident_client::{
    Client, ClientError, Completion as ClientCompletion, ExecutionState, RebootDisposition,
    RebootPolicy, RemoteError, RemoteErrorCategory, ServicingEvent, ServicingKind,
    ServicingOperation, ServicingState, Sha384Hash,
};

use crate::{
    config::{Config, SerialVerbosity, DEFAULT_HOST_CONFIGURATION},
    source::{self, Plan, Request},
};

#[derive(Debug)]
pub(super) enum Event {
    InstallerLog {
        level: Level,
        message: String,
    },
    Preparing(String),
    Prepared {
        description: String,
        reboot: bool,
        serial_verbosity: SerialVerbosity,
    },
    Response(ServicingEvent),
    IdleConfirmed,
    Error {
        details: String,
        uncertain: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Completion {
    Success {
        reboot_required: bool,
        performed: bool,
    },
    Failure(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AttemptOutcome {
    Completed,
    Aborted,
}

/// Interprets a domain completion, enforcing the installer's caller-managed reboot contract.
pub(super) fn completion(
    outcome: Result<ClientCompletion, RemoteError>,
) -> Result<Completion, Error> {
    match outcome {
        Ok(ClientCompletion {
            reboot,
            servicing_kind,
            ..
        }) => {
            let reboot_required = match reboot {
                RebootDisposition::NotRequired => false,
                RebootDisposition::Required => true,
                RebootDisposition::Started => {
                    bail!("Daemon did not honour caller-managed reboot: {reboot:?}")
                }
                _ => bail!("Unsupported daemon reboot disposition: {reboot:?}"),
            };
            let performed = match servicing_kind {
                None => true,
                Some(ServicingKind::NoneRequired) => false,
                Some(
                    ServicingKind::RuntimeUpdate
                    | ServicingKind::AbUpdate
                    | ServicingKind::CleanInstall
                    | ServicingKind::ManualRollbackAb
                    | ServicingKind::ManualRollbackRuntime,
                ) => true,
                Some(other) => bail!("Unsupported daemon servicing kind: {other:?}"),
            };
            Ok(Completion::Success {
                reboot_required,
                performed,
            })
        }
        Err(error) => Ok(Completion::Failure(format_error(&error))),
    }
}

/// Classifies a daemon completion using the same rules that drive the result screen.
fn attempt_outcome(completion_result: &Result<ClientCompletion, RemoteError>) -> AttemptOutcome {
    match completion(completion_result.clone()) {
        Ok(Completion::Success { .. }) => AttemptOutcome::Completed,
        Ok(Completion::Failure(_)) | Err(_) => AttemptOutcome::Aborted,
    }
}

/// Renders a structured Trident error as readable multi-line diagnostics.
fn format_error(error: &RemoteError) -> String {
    let category = match error.category {
        RemoteErrorCategory::ExecutionEnvironmentMisconfiguration => {
            Cow::Borrowed("Execution environment")
        }
        RemoteErrorCategory::HealthChecks => Cow::Borrowed("Health checks"),
        RemoteErrorCategory::Initialization => Cow::Borrowed("Initialization"),
        RemoteErrorCategory::Internal => Cow::Borrowed("Internal error"),
        RemoteErrorCategory::InvalidInput => Cow::Borrowed("Invalid input"),
        RemoteErrorCategory::Servicing => Cow::Borrowed("Servicing"),
        RemoteErrorCategory::UnsupportedConfiguration => Cow::Borrowed("Unsupported configuration"),
        RemoteErrorCategory::Unspecified => Cow::Borrowed("Unspecified"),
        RemoteErrorCategory::Unknown(value) => Cow::Owned(format!("Unknown ({value})")),
        other => Cow::Owned(format!("{other:?}")),
    };
    let mut details = format!(
        "{}\n\nCategory: {category}\nCode: {}",
        error.message.trim(),
        error.subkind
    );
    if !error.error_message.is_empty() && !error.message.contains(&error.error_message) {
        details.push_str(&format!("\nCause: {}", error.error_message));
    }
    if let Some(location) = &error.location {
        details.push_str(&format!("\nLocation: {}:{}", location.path, location.line));
    }
    details
}

/// Keeps invalid completions and unacknowledged writes uncertain, but not known remote failures.
fn operation_uncertain(error: &ClientError) -> bool {
    error.remote_error().is_none() && error.execution_state() != ExecutionState::NotStarted
}

/// Rejects unrecognized states before retrying an operation with an unknown prior outcome.
fn recognized_servicing_state(state: ServicingState) -> bool {
    matches!(
        state,
        ServicingState::NotProvisioned
            | ServicingState::CleanInstallStaged
            | ServicingState::AbUpdateStaged
            | ServicingState::ManualRollbackAbStaged
            | ServicingState::ManualRollbackRuntimeStaged
            | ServicingState::RuntimeUpdateStaged
            | ServicingState::CleanInstallFinalized
            | ServicingState::AbUpdateFinalized
            | ServicingState::ManualRollbackAbFinalized
            | ServicingState::Provisioned
            | ServicingState::AbUpdateHealthCheckFailed
    )
}

/// Describes the requested operation without exposing credentials in source URLs.
fn request_description(settings: &Config, request: &Request) -> String {
    match request {
        Request::Autorun => format!(
            "Autorun: HC '{}' or first media COSI in '{}'",
            source::display_source(
                settings
                    .autorun
                    .host_configuration
                    .as_deref()
                    .unwrap_or(DEFAULT_HOST_CONFIGURATION)
            ),
            settings.media.cosi_directory.display()
        ),
        Request::HostConfiguration(url) => format!("Install from {}", source::display_source(url)),
        Request::Stream(url) => format!("StreamDisk from {}", source::display_url(url)),
    }
}

/// Prepares a source, checks stream safety, and forwards Trident's domain events to the UI.
///
/// After an uncertain previous operation, `require_idle` prevents a new write until the daemon
/// answers a status request. Ambiguous RPC failures are reported as uncertain instead of retrying.
pub(super) async fn execute(
    settings: Config,
    request: Request,
    require_idle: bool,
    tx: Sender<Event>,
    mut terminal: impl FnMut(AttemptOutcome),
) -> Result<AttemptOutcome, Error> {
    let description = request_description(&settings, &request);
    send(
        &tx,
        Event::Prepared {
            description,
            reboot: settings.autorun.reboot,
            serial_verbosity: settings.serial_verbosity,
        },
    )
    .await?;
    let progress = tx.clone();
    let prepared = task::spawn_blocking(move || {
        source::prepare_with_progress(&settings, &request, |stage| {
            progress
                .blocking_send(Event::Preparing(stage.to_owned()))
                .map_err(|_| anyhow!("Installer event receiver closed"))
        })
    })
    .await?;
    let (settings, plan) = match prepared {
        Ok(prepared) => prepared,
        Err(error) => {
            return abort(
                &tx,
                Event::Error {
                    details: format!("{error:#}"),
                    uncertain: require_idle,
                },
                &mut terminal,
            )
            .await
        }
    };
    send(
        &tx,
        Event::Prepared {
            description: plan.description(),
            reboot: settings.autorun.reboot,
            serial_verbosity: settings.serial_verbosity,
        },
    )
    .await?;
    let stream_hash: Option<Sha384Hash> = if let Plan::Stream { image } = &plan {
        send(
            &tx,
            Event::Preparing("Checking image metadata and available disks".into()),
        )
        .await?;
        let image = image.clone();
        match task::spawn_blocking(move || source::stream_preflight(&image)).await? {
            Ok(hash) => Some(hash),
            Err(error) => {
                return abort(
                    &tx,
                    Event::Error {
                        details: format!("{error:#}"),
                        uncertain: false,
                    },
                    &mut terminal,
                )
                .await
            }
        }
    } else {
        None
    };
    send(&tx, Event::Preparing("Connecting to Trident daemon".into())).await?;
    let client = match Client::connect_default().await {
        Ok(client) => client,
        Err(error) => {
            return abort(
                &tx,
                Event::Error {
                    details: format!("{error:#}"),
                    uncertain: require_idle,
                },
                &mut terminal,
            )
            .await
        }
    };
    if require_idle {
        match client.servicing_state().await {
            Ok(state) if recognized_servicing_state(state) => {
                send(&tx, Event::IdleConfirmed).await?;
            }
            Ok(state) => {
                return abort(
                    &tx,
                    Event::Error {
                        details: format!("The previous operation's outcome is unknown. Trident returned an unsupported servicing state {state:?}; refusing another write."),
                        uncertain: true,
                    },
                    &mut terminal,
                )
                .await;
            }
            Err(error) => {
                return abort(
                    &tx,
                    Event::Error {
                        details: format!("The previous operation's outcome is unknown. Cannot establish that Trident is idle; refusing another write.\n{error}"),
                        uncertain: true,
                    },
                    &mut terminal,
                )
                .await;
            }
        }
    }
    let response = match (plan, stream_hash) {
        (Plan::Stream { image }, Some(hash)) => {
            client
                .stream_disk(&image, Some(&hash), RebootPolicy::Caller)
                .await
        }
        (Plan::Install { config, .. }, None) => client.install(&config, RebootPolicy::Caller).await,
        _ => {
            return Err(anyhow!(
                "Installer plan and preflight hash are inconsistent"
            ))
        }
    };
    let response = match response {
        Ok(response) => response,
        Err(error) => {
            return abort(
                &tx,
                Event::Error {
                    details: format!("gRPC request failed: {error}"),
                    uncertain: operation_uncertain(&error),
                },
                &mut terminal,
            )
            .await;
        }
    };
    receive(response, &tx, &mut terminal).await
}

/// Forwards domain events until a completed result; a broken stream leaves the outcome unknown.
async fn receive(
    mut operation: ServicingOperation,
    tx: &Sender<Event>,
    terminal: &mut impl FnMut(AttemptOutcome),
) -> Result<AttemptOutcome, Error> {
    loop {
        let event = match operation.next_event().await {
            Ok(Some(event)) => event,
            Ok(None) => {
                return abort(
                    tx,
                    Event::Error {
                        details: "The gRPC stream ended without a final Completed response. Installation outcome is unknown; it may still be running.".into(),
                        uncertain: true,
                    },
                    terminal,
                )
                .await;
            }
            Err(error) => {
                return abort(
                    tx,
                    Event::Error {
                        details: format!(
                            "The gRPC stream failed; installation outcome is unknown.\n{error}"
                        ),
                        uncertain: operation_uncertain(&error),
                    },
                    terminal,
                )
                .await;
            }
        };
        let completed = match &event {
            ServicingEvent::Completed(result) => Some(attempt_outcome(result)),
            _ => None,
        };
        if let Some(outcome) = completed {
            terminal(outcome);
        }
        send(tx, Event::Response(event)).await?;
        if let Some(outcome) = completed {
            return Ok(outcome);
        }
    }
}

/// Sends a handled installation error and classifies the attempt as aborted.
async fn abort(
    tx: &Sender<Event>,
    event: Event,
    terminal: &mut impl FnMut(AttemptOutcome),
) -> Result<AttemptOutcome, Error> {
    terminal(AttemptOutcome::Aborted);
    send(tx, event).await?;
    Ok(AttemptOutcome::Aborted)
}

/// Sends an installer event, returning an error if the UI receiver has closed.
pub(super) async fn send(tx: &Sender<Event>, event: Event) -> Result<(), Error> {
    tx.send(event)
        .await
        .map_err(|_| anyhow!("Installer event receiver closed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use url::Url;

    use trident_client::SourceLocation;

    #[test]
    /// Autorun and recovery descriptions must not disclose signed source URLs.
    fn request_descriptions_redact_secrets() {
        let url = "https://user:password@example.com/image.cosi?token=secret#fragment";
        let mut settings = Config::default();
        settings.autorun.host_configuration = Some(url.into());
        for request in [
            Request::Autorun,
            Request::HostConfiguration(url.into()),
            Request::Stream(Url::parse(url).unwrap()),
        ] {
            let description = request_description(&settings, &request);
            assert!(description.contains("https://example.com/image.cosi"));
            for secret in ["user", "password", "token=secret", "fragment"] {
                assert!(!description.contains(secret), "{description}");
            }
        }
    }

    /// Builds a valid domain completion for installer-specific result checks.
    fn successful() -> ClientCompletion {
        ClientCompletion {
            reboot: RebootDisposition::Required,
            servicing_kind: Some(ServicingKind::CleanInstall),
            image_hash: None,
        }
    }

    #[test]
    /// Ensures caller-managed reboot and no-op completions retain their installer semantics.
    fn final_response_is_strict() {
        assert_eq!(
            completion(Ok(successful())).unwrap(),
            Completion::Success {
                reboot_required: true,
                performed: true,
            }
        );
        completion(Ok(ClientCompletion {
            reboot: RebootDisposition::Started,
            ..successful()
        }))
        .unwrap_err();
        assert_eq!(
            completion(Ok(ClientCompletion {
                servicing_kind: Some(ServicingKind::NoneRequired),
                ..successful()
            }))
            .unwrap(),
            Completion::Success {
                reboot_required: true,
                performed: false,
            }
        );
        completion(Ok(ClientCompletion {
            servicing_kind: Some(ServicingKind::Unknown(42)),
            ..successful()
        }))
        .unwrap_err();
    }

    #[test]
    fn daemon_completions_map_to_attempt_outcomes() {
        assert_eq!(
            attempt_outcome(&Ok(successful())),
            AttemptOutcome::Completed
        );
        assert_eq!(
            attempt_outcome(&Ok(ClientCompletion {
                servicing_kind: Some(ServicingKind::NoneRequired),
                ..successful()
            })),
            AttemptOutcome::Completed
        );
        assert_eq!(
            attempt_outcome(&Err(remote_failure())),
            AttemptOutcome::Aborted
        );
        assert_eq!(
            attempt_outcome(&Ok(ClientCompletion {
                servicing_kind: Some(ServicingKind::Unknown(42)),
                ..successful()
            })),
            AttemptOutcome::Aborted
        );
    }

    /// Builds a structured remote failure for formatting and execution-state tests.
    fn remote_failure() -> RemoteError {
        RemoteError {
            category: RemoteErrorCategory::Servicing,
            subkind: "download-image".into(),
            message: "Installation failed\n  Failed to download the image".into(),
            error_message: "Connection timed out".into(),
            location: Some(SourceLocation {
                path: "source.rs".into(),
                line: 42,
            }),
        }
    }

    #[test]
    /// Ensures domain errors remain readable without protobuf debug output.
    fn errors_are_readable_without_debug_dumps() {
        let formatted = format_error(&remote_failure());
        assert!(formatted.contains("Installation failed\n  Failed to download the image"));
        assert!(formatted.contains("Category: Servicing"));
        assert!(formatted.contains("Code: download-image"));
        assert!(formatted.contains("Cause: Connection timed out"));
        assert!(formatted.contains("Location: source.rs:42"));
        assert!(!formatted.contains("TridentError {"));
        assert!(!formatted.contains("\\n"));
        assert_eq!(
            completion(Err(remote_failure())).unwrap(),
            Completion::Failure(formatted)
        );
        let category = RemoteError {
            category: RemoteErrorCategory::Unknown(42),
            ..remote_failure()
        };
        assert!(format_error(&category).contains("Category: Unknown (42)"));
    }

    /// Connection errors cannot be mistaken for an accepted servicing request.
    #[tokio::test]
    async fn connection_errors_do_not_mark_a_write_uncertain() {
        let error = Client::connect("not an endpoint").await.unwrap_err();
        assert_eq!(error.execution_state(), ExecutionState::NotStarted);
        assert!(!operation_uncertain(&error));
    }

    #[test]
    /// Unknown servicing states cannot clear the uncertainty of a previous write.
    fn unknown_servicing_state_is_not_accepted_for_retry() {
        assert!(recognized_servicing_state(ServicingState::NotProvisioned));
        assert!(recognized_servicing_state(
            ServicingState::CleanInstallFinalized
        ));
        assert!(!recognized_servicing_state(ServicingState::Unspecified));
        assert!(!recognized_servicing_state(ServicingState::Unknown(42)));
    }
}
