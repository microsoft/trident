use std::borrow::Cow;

use anyhow::{anyhow, bail, Error};
use log::Level;
use tokio::{sync::mpsc::Sender, task};
use url::Url;

use trident_client::{
    Client, ClientError, Completion as ClientCompletion, ExecutionState, RebootDisposition,
    RebootPolicy, RemoteError, RemoteErrorCategory, ServicingEvent, ServicingKind,
    ServicingOperation, ServicingState, Sha384Hash,
};

use crate::{
    config::{Config, SerialVerbosity, DEFAULT_HOST_CONFIGURATION},
    source::{self, Plan, Request},
};

/// Applies configured force only to autorun streams, while respecting explicit confirmation.
fn stream_force_requested(automatic: bool, configured: bool, confirmed: bool) -> bool {
    confirmed || (automatic && configured)
}

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
        stream_image: Option<Url>,
    },
    Response(ServicingEvent),
    Error {
        details: String,
        uncertain: bool,
    },
    AlreadyPresent(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Completion {
    Success {
        reboot_required: bool,
        performed: bool,
    },
    Failure(String),
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
    force: bool,
    require_idle: bool,
    tx: Sender<Event>,
) -> Result<(), Error> {
    let automatic = request == Request::Autorun;
    let description = request_description(&settings, &request);
    send(
        &tx,
        Event::Prepared {
            description,
            reboot: settings.autorun.reboot,
            serial_verbosity: settings.serial_verbosity,
            stream_image: None,
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
            return send(
                &tx,
                Event::Error {
                    details: format!("{error:#}"),
                    uncertain: require_idle,
                },
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
            stream_image: match &plan {
                Plan::Stream { image } => Some(image.clone()),
                Plan::Install { .. } => None,
            },
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
        let force = stream_force_requested(automatic, settings.autorun.force, force);
        match task::spawn_blocking(move || source::stream_preflight(&image, force)).await? {
            Ok((true, _)) => return send(&tx, Event::AlreadyPresent(
                "Every COSI filesystem UUID was found on an attached disk.\nNo installation was started.".into()
            )).await,
            Ok((false, hash)) => Some(hash),
            Err(error) => return send(&tx, Event::Error { details: format!("{error:#}"), uncertain: false }).await,
        }
    } else {
        None
    };
    send(&tx, Event::Preparing("Connecting to Trident daemon".into())).await?;
    let client = match Client::connect_default().await {
        Ok(client) => client,
        Err(error) => {
            return send(
                &tx,
                Event::Error {
                    details: format!("{error:#}"),
                    uncertain: require_idle,
                },
            )
            .await
        }
    };
    if require_idle {
        match client.servicing_state().await {
            Ok(state) if recognized_servicing_state(state) => {}
            Ok(state) => {
                return send(&tx, Event::Error {
                    details: format!("The previous operation's outcome is unknown. Trident returned an unsupported servicing state {state:?}; refusing another write."),
                    uncertain: true,
                }).await;
            }
            Err(error) => {
                return send(&tx, Event::Error {
                    details: format!("The previous operation's outcome is unknown. Cannot establish that Trident is idle; refusing another write.\n{error}"),
                    uncertain: true,
                }).await;
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
            return send(
                &tx,
                Event::Error {
                    details: format!("gRPC request failed: {error}"),
                    uncertain: operation_uncertain(&error),
                },
            )
            .await;
        }
    };
    receive(response, &tx).await
}

/// Forwards domain events until a completed result; a broken stream leaves the outcome unknown.
async fn receive(mut operation: ServicingOperation, tx: &Sender<Event>) -> Result<(), Error> {
    loop {
        let event = match operation.next_event().await {
            Ok(Some(event)) => event,
            Ok(None) => {
                return send(
                    tx,
                    Event::Error {
                        details: "The gRPC stream ended without a final Completed response. Installation outcome is unknown; it may still be running.".into(),
                        uncertain: true,
                    },
                )
                .await;
            }
            Err(error) => {
                return send(
                    tx,
                    Event::Error {
                        details: format!(
                            "The gRPC stream failed; installation outcome is unknown.\n{error}"
                        ),
                        uncertain: operation_uncertain(&error),
                    },
                )
                .await;
            }
        };
        let completed = matches!(event, ServicingEvent::Completed(_));
        send(tx, Event::Response(event)).await?;
        if completed {
            return Ok(());
        }
    }
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

    #[test]
    /// Ensures a configured force flag does not affect manually chosen streams.
    fn only_automatic_streams_use_configured_force() {
        assert!(!stream_force_requested(true, false, false));
        assert!(stream_force_requested(true, true, false));
        assert!(!stream_force_requested(false, true, false));
        assert!(stream_force_requested(false, false, true));
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
