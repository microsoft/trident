use anyhow::{anyhow, ensure, Error};
use tokio::{sync::mpsc::Sender, task};
use url::Url;

use trident_api::status::ServicingType;
use trident_client::{
    Client, ClientError, Completion as ClientCompletion, ExecutionState, OperationOutcome,
    RebootDisposition, RebootPolicy, RemoteError, RemoteErrorCategory, ServicingEvent,
    ServicingOperation,
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
        level: log::Level,
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
pub(super) fn completion(outcome: OperationOutcome) -> Result<Completion, Error> {
    match outcome {
        OperationOutcome::Succeeded(ClientCompletion {
            reboot,
            servicing_type,
            ..
        }) => {
            ensure!(
                reboot != RebootDisposition::Started,
                "Daemon did not honour caller-managed reboot: {reboot:?}"
            );
            Ok(Completion::Success {
                reboot_required: reboot == RebootDisposition::Required,
                performed: servicing_type != Some(ServicingType::NoActiveServicing),
            })
        }
        OperationOutcome::Failed(error) => Ok(Completion::Failure(format_error(&error))),
    }
}

/// Renders a structured Trident error as readable multi-line diagnostics.
fn format_error(error: &RemoteError) -> String {
    let category = match error.category {
        RemoteErrorCategory::ExecutionEnvironmentMisconfiguration => "Execution environment",
        RemoteErrorCategory::HealthChecks => "Health checks",
        RemoteErrorCategory::Initialization => "Initialization",
        RemoteErrorCategory::Internal => "Internal error",
        RemoteErrorCategory::InvalidInput => "Invalid input",
        RemoteErrorCategory::Servicing => "Servicing",
        RemoteErrorCategory::UnsupportedConfiguration => "Unsupported configuration",
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
    !matches!(error, ClientError::RemoteFailure { .. })
        && error.execution_state() != ExecutionState::NotStarted
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
    let automatic = matches!(request, Request::Autorun);
    let description = match &request {
        Request::Autorun => format!(
            "Autorun: HC '{}' or first media COSI in '{}'",
            settings
                .autorun
                .host_configuration
                .as_deref()
                .unwrap_or(DEFAULT_HOST_CONFIGURATION),
            settings.media.cosi_directory.display()
        ),
        Request::HostConfiguration(url) => format!("Install from {url}"),
        Request::Stream(url) => format!("StreamDisk from {url}"),
    };
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
    if let Plan::Stream { image } = &plan {
        send(
            &tx,
            Event::Preparing("Checking image metadata and available disks".into()),
        )
        .await?;
        let image = image.clone();
        let force = stream_force_requested(automatic, settings.autorun.force, force);
        match task::spawn_blocking(move || source::stream_preflight(&image, force)).await? {
            Ok(true) => return send(&tx, Event::AlreadyPresent(
                "Every COSI filesystem UUID was found on an attached disk.\nNo installation was started.".into()
            )).await,
            Ok(false) => {}
            Err(error) => return send(&tx, Event::Error { details: format!("{error:#}"), uncertain: false }).await,
        }
    }
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
        if let Err(error) = client.servicing_state().await {
            return send(&tx, Event::Error {
                details: format!("The previous operation's outcome is unknown. Cannot establish that Trident is idle; refusing another write.\n{error}"),
                uncertain: true,
            }).await;
        }
    }
    let response = match start(&client, plan).await {
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

/// Selects the shared client's typed RPC while leaving reboot control to the installer.
async fn start(client: &Client, plan: Plan) -> Result<ServicingOperation, ClientError> {
    match plan {
        Plan::Stream { image } => client.stream_disk(&image, None, RebootPolicy::Caller).await,
        Plan::Install { config, .. } => client.install(&config, RebootPolicy::Caller).await,
    }
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

    use trident_client::{Operation, RpcCode, SourceLocation};

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
            servicing_type: Some(ServicingType::CleanInstall),
            image_hash: None,
        }
    }

    #[test]
    /// Ensures caller-managed reboot and no-op completions retain their installer semantics.
    fn final_response_is_strict() {
        assert_eq!(
            completion(OperationOutcome::Succeeded(successful())).unwrap(),
            Completion::Success {
                reboot_required: true,
                performed: true,
            }
        );
        completion(OperationOutcome::Succeeded(ClientCompletion {
            reboot: RebootDisposition::Started,
            ..successful()
        }))
        .unwrap_err();
        assert_eq!(
            completion(OperationOutcome::Succeeded(ClientCompletion {
                servicing_type: Some(ServicingType::NoActiveServicing),
                ..successful()
            }))
            .unwrap(),
            Completion::Success {
                reboot_required: true,
                performed: false,
            }
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
            completion(OperationOutcome::Failed(remote_failure())).unwrap(),
            Completion::Failure(formatted)
        );
    }

    #[test]
    /// Distinguishes rejected writes and known failures from ambiguous servicing outcomes.
    fn execution_uncertainty_is_preserved() {
        assert!(!operation_uncertain(&ClientError::RequestRejected {
            operation: Operation::StreamDisk,
            code: RpcCode::FailedPrecondition,
            message: "not started".into(),
        }));
        assert!(operation_uncertain(&ClientError::MissingCompletion {
            operation: Operation::StreamDisk,
        }));
        assert!(operation_uncertain(&ClientError::InvalidProtocol {
            operation: Operation::StreamDisk,
            details: "invalid final status".into(),
            state: ExecutionState::Completed,
        }));
        assert!(!operation_uncertain(&ClientError::RemoteFailure {
            operation: Operation::StreamDisk,
            remote: remote_failure(),
        }));
    }
}
