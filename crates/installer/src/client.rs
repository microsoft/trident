use std::time::Duration;

use anyhow::{anyhow, bail, ensure, Context, Error};
use tokio::{sync::mpsc::Sender, task};
use tonic::{
    transport::{Channel, Endpoint},
    Code, Status, Streaming,
};
use url::Url;

use trident_proto::{
    v1::{
        servicing_response::Response as ResponseBody,
        streaming_service_client::StreamingServiceClient, Completed, HostConfiguration,
        RebootHandling, RebootManagement, RebootStatus, ServicingKind, ServicingResponse,
        StatusCode, StreamDiskRequest, TridentError as RemoteError,
        TridentErrorKind as ErrorCategory,
    },
    v1preview::{
        install_service_client::InstallServiceClient, status_service_client::StatusServiceClient,
        FinalizeInstallRequest, GetServicingStateRequest, InstallRequest, StageInstallRequest,
    },
    TRIDENT_DEFAULT_SOCKET_URI,
};

use crate::{
    config::{Config, SerialVerbosity, DEFAULT_HOST_CONFIGURATION},
    source::{self, Plan, Request},
};

const CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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
    Response(ServicingResponse),
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

pub(super) fn completion(completed: Completed) -> Result<Completion, Error> {
    match StatusCode::try_from(completed.status).context("Invalid final status code")? {
        StatusCode::Success => {
            ensure!(
                completed.error.is_none(),
                "Success response unexpectedly includes an error"
            );
            let reboot =
                RebootStatus::try_from(completed.reboot_status).context("Invalid reboot status")?;
            ensure!(
                matches!(
                    reboot,
                    RebootStatus::RebootRequired | RebootStatus::RebootNotRequired
                ),
                "Daemon did not honour caller-managed reboot: {reboot:?}"
            );
            let performed = match completed.servicing_kind {
                Some(kind) => {
                    ServicingKind::try_from(kind).context("Invalid servicing kind")?
                        != ServicingKind::NoneRequired
                }
                None => true,
            };
            Ok(Completion::Success {
                reboot_required: reboot == RebootStatus::RebootRequired,
                performed,
            })
        }
        StatusCode::Failure => Ok(Completion::Failure(match completed.error {
            Some(error) => format_error(&error),
            None => "Trident reported failure without error details".into(),
        })),
        StatusCode::Unspecified => bail!("Daemon returned an unspecified final status"),
    }
}

fn format_error(error: &RemoteError) -> String {
    let category = match ErrorCategory::try_from(error.kind) {
        Ok(ErrorCategory::Unspecified) => "Unspecified",
        Ok(ErrorCategory::ExecutionEnvironmentMisconfigurationError) => "Execution environment",
        Ok(ErrorCategory::HealthChecksError) => "Health checks",
        Ok(ErrorCategory::InitializationError) => "Initialization",
        Ok(ErrorCategory::InternalError) => "Internal error",
        Ok(ErrorCategory::InvalidInputError) => "Invalid input",
        Ok(ErrorCategory::ServicingError) => "Servicing",
        Ok(ErrorCategory::UnsupportedConfigurationError) => "Unsupported configuration",
        Err(_) => "Unknown",
    };
    let mut details = format!(
        "{}\n\nCategory: {category} ({})\nCode: {}",
        error.message.trim(),
        error.kind,
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

pub(super) async fn execute(
    settings: Config,
    request: Request,
    force: bool,
    require_idle: bool,
    tx: Sender<Event>,
) -> Result<(), Error> {
    let description = match &request {
        Request::Autorun => format!(
            "Autorun: HC '{}' or first ISO COSI in '{}'",
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
        match task::spawn_blocking(move || source::stream_preflight(&image, force)).await? {
            Ok(true) => return send(&tx, Event::AlreadyPresent(
                "Every COSI filesystem UUID was found on an attached disk.\nNo installation was started.\nThis is an identity guard, not a health check.".into()
            )).await,
            Ok(false) => {}
            Err(error) => return send(&tx, Event::Error { details: format!("{error:#}"), uncertain: false }).await,
        }
    }
    send(&tx, Event::Preparing("Connecting to Trident daemon".into())).await?;
    let channel = match connect(TRIDENT_DEFAULT_SOCKET_URI).await {
        Ok(channel) => channel,
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
        if let Err(error) = StatusServiceClient::new(channel.clone())
            .get_servicing_state(GetServicingStateRequest {})
            .await
        {
            return send(&tx, Event::Error {
                details: format!("The previous operation's outcome is unknown. Cannot establish that Trident is idle; refusing another write.\n{error}"),
                uncertain: true,
            }).await;
        }
    }
    let started = tokio::time::timeout(REQUEST_TIMEOUT, start(channel, plan))
        .await
        .unwrap_or_else(|_| {
            Err(Status::deadline_exceeded(
                "Timed out waiting for Trident to accept the request; outcome is unknown",
            ))
        });
    let response = match started {
        Ok(response) => response,
        Err(error) => {
            let uncertain = !matches!(
                error.code(),
                Code::InvalidArgument | Code::Unimplemented | Code::FailedPrecondition
            );
            return send(
                &tx,
                Event::Error {
                    details: format!("gRPC request failed: {error}"),
                    uncertain,
                },
            )
            .await;
        }
    };
    receive(response, &tx).await
}

async fn connect(address: &str) -> Result<Channel, Error> {
    Endpoint::new(address.to_owned())?
        .connect_timeout(CONNECTION_TIMEOUT)
        .connect()
        .await
        .with_context(|| format!("Failed to connect to Trident at '{address}'"))
}

async fn start(channel: Channel, plan: Plan) -> Result<Streaming<ServicingResponse>, Status> {
    let reboot = Some(RebootManagement {
        handling: RebootHandling::CallerHandlesReboot.into(),
    });
    match plan {
        Plan::Stream { image } => Ok(StreamingServiceClient::new(channel)
            .stream_disk(StreamDiskRequest {
                image_url: image.to_string(),
                image_hash: None,
                reboot,
            })
            .await?
            .into_inner()),
        Plan::Install { config, .. } => {
            let config = serde_yaml::to_string(&config).map_err(|error| {
                Status::internal(format!("Failed to serialize Host Configuration: {error}"))
            })?;
            Ok(InstallServiceClient::new(channel)
                .install(InstallRequest {
                    stage: Some(StageInstallRequest {
                        config: Some(HostConfiguration { config }),
                    }),
                    finalize: Some(FinalizeInstallRequest { reboot }),
                })
                .await?
                .into_inner())
        }
    }
}

async fn receive(
    mut stream: Streaming<ServicingResponse>,
    tx: &Sender<Event>,
) -> Result<(), Error> {
    loop {
        let response = match stream.message().await {
            Ok(Some(response)) => response,
            Ok(None) => return send(tx, Event::Error {
                details: "The gRPC stream ended without a final Completed response. Installation outcome is unknown; it may still be running.".into(),
                uncertain: true,
            }).await,
            Err(error) => return send(tx, Event::Error {
                details: format!("The gRPC stream failed; installation outcome is unknown.\n{error}"),
                uncertain: true,
            }).await,
        };
        let completed = matches!(response.response, Some(ResponseBody::Completed(_)));
        if response.response.is_none() {
            return send(
                tx,
                Event::Error {
                    details: "gRPC response has no body; installation outcome is unknown".into(),
                    uncertain: true,
                },
            )
            .await;
        }
        send(tx, Event::Response(response)).await?;
        if completed {
            return Ok(());
        }
    }
}

pub(super) async fn send(tx: &Sender<Event>, event: Event) -> Result<(), Error> {
    tx.send(event)
        .await
        .map_err(|_| anyhow!("Installer event receiver closed"))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        sync::{Arc, Mutex},
        vec::IntoIter,
    };

    use tempfile::TempDir;
    use tokio::{net::UnixListener, sync::mpsc};
    use tokio_stream::{wrappers::UnixListenerStream, Iter};
    use tonic::{async_trait, transport::Server, Request as TonicRequest, Response};
    use url::Url;

    use trident_api::config::{HostConfiguration as ApiHostConfiguration, ImageSha384, OsImage};
    use trident_proto::{
        v1::{
            streaming_service_server::{StreamingService, StreamingServiceServer},
            FileLocation, Log, LogLevel, Started,
        },
        v1preview::install_service_server::{InstallService, InstallServiceServer},
    };

    type ResponseStream = Iter<IntoIter<Result<ServicingResponse, Status>>>;

    fn successful() -> Completed {
        Completed {
            status: StatusCode::Success.into(),
            reboot_status: RebootStatus::RebootRequired.into(),
            servicing_kind: Some(ServicingKind::CleanInstall.into()),
            ..Default::default()
        }
    }

    #[test]
    fn final_response_is_strict() {
        assert_eq!(
            completion(successful()).unwrap(),
            Completion::Success {
                reboot_required: true,
                performed: true
            }
        );
        completion(Completed::default()).unwrap_err();
        completion(Completed {
            status: 999,
            ..successful()
        })
        .unwrap_err();
        completion(Completed {
            reboot_status: RebootStatus::RebootStarted.into(),
            ..successful()
        })
        .unwrap_err();
        assert_eq!(
            completion(Completed {
                servicing_kind: Some(ServicingKind::NoneRequired.into()),
                ..successful()
            })
            .unwrap(),
            Completion::Success {
                reboot_required: true,
                performed: false
            }
        );
    }

    #[test]
    fn errors_are_readable_without_debug_dumps() {
        let error = RemoteError {
            kind: ErrorCategory::ServicingError.into(),
            subkind: "download-image".into(),
            message: "Installation failed\n  Failed to download the image".into(),
            error_message: "Connection timed out".into(),
            location: Some(FileLocation {
                path: "source.rs".into(),
                line: 42,
            }),
        };
        let formatted = format_error(&error);
        assert!(formatted.contains("Installation failed\n  Failed to download the image"));
        assert!(formatted.contains("Category: Servicing"));
        assert!(formatted.contains("Code: download-image"));
        assert!(formatted.contains("Cause: Connection timed out"));
        assert!(formatted.contains("Location: source.rs:42"));
        assert!(!formatted.contains("TridentError {"));
        assert!(!formatted.contains("\\n"));
    }

    #[derive(Clone)]
    struct Fake {
        complete: bool,
    }

    #[async_trait]
    impl StreamingService for Fake {
        type StreamDiskStream = ResponseStream;

        async fn stream_disk(
            &self,
            request: TonicRequest<StreamDiskRequest>,
        ) -> Result<Response<Self::StreamDiskStream>, Status> {
            assert_eq!(
                request.into_inner().reboot.unwrap().handling,
                i32::from(RebootHandling::CallerHandlesReboot)
            );
            let mut responses = vec![
                ServicingResponse {
                    response: Some(ResponseBody::Started(Started {})),
                    ..Default::default()
                },
                ServicingResponse {
                    response: Some(ResponseBody::Log(Log {
                        level: LogLevel::Trace.into(),
                        message: "full detail".into(),
                        ..Default::default()
                    })),
                    ..Default::default()
                },
            ];
            if self.complete {
                responses.push(ServicingResponse {
                    response: Some(ResponseBody::Completed(successful())),
                    ..Default::default()
                });
            }

            Ok(Response::new(tokio_stream::iter(
                responses.into_iter().map(Ok).collect::<Vec<_>>(),
            )))
        }
    }

    #[derive(Clone)]
    struct FakeInstall {
        request: Arc<Mutex<Option<InstallRequest>>>,
    }

    #[async_trait]
    impl InstallService for FakeInstall {
        type InstallStream = ResponseStream;
        type InstallStageStream = ResponseStream;
        type InstallFinalizeStream = ResponseStream;

        async fn install(
            &self,
            request: TonicRequest<InstallRequest>,
        ) -> Result<Response<Self::InstallStream>, Status> {
            *self.request.lock().unwrap() = Some(request.into_inner());
            Ok(Response::new(tokio_stream::iter(vec![Ok(
                ServicingResponse {
                    response: Some(ResponseBody::Completed(successful())),
                    ..Default::default()
                },
            )])))
        }

        async fn install_stage(
            &self,
            _: TonicRequest<StageInstallRequest>,
        ) -> Result<Response<Self::InstallStageStream>, Status> {
            Err(Status::unimplemented("test only supports full install"))
        }

        async fn install_finalize(
            &self,
            _: TonicRequest<FinalizeInstallRequest>,
        ) -> Result<Response<Self::InstallFinalizeStream>, Status> {
            Err(Status::unimplemented("test only supports full install"))
        }
    }

    #[tokio::test]
    async fn install_sends_full_configuration_and_caller_managed_reboot() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("trident.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let captured = Arc::new(Mutex::new(None));
        let server = tokio::spawn(
            Server::builder()
                .add_service(InstallServiceServer::new(FakeInstall {
                    request: captured.clone(),
                }))
                .serve_with_incoming(UnixListenerStream::new(listener)),
        );
        let image = OsImage {
            url: Url::parse("file:///media/cosi/selected.cosi").unwrap(),
            sha384: ImageSha384::new(&format!("sha384:{}", "ab".repeat(48))).unwrap(),
        };
        let config = ApiHostConfiguration {
            image: Some(image.clone()),
            ..Default::default()
        };
        let channel = connect(&format!("unix://{}", path.display()))
            .await
            .unwrap();
        let stream = start(
            channel,
            Plan::Install {
                config: Box::new(config),
                source: "test".into(),
            },
        )
        .await
        .unwrap();
        let (tx, mut rx) = mpsc::channel(4);
        receive(stream, &tx).await.unwrap();
        assert!(matches!(rx.recv().await.unwrap(), Event::Response(_)));
        let request = captured.lock().unwrap().take().unwrap();
        assert_eq!(
            request.finalize.unwrap().reboot.unwrap().handling,
            i32::from(RebootHandling::CallerHandlesReboot)
        );
        let received: ApiHostConfiguration =
            serde_yaml::from_str(&request.stage.unwrap().config.unwrap().config).unwrap();
        assert_eq!(received.image, Some(image));
        server.abort();
    }

    #[tokio::test]
    async fn real_unix_transport_preserves_trace_and_detects_missing_completion() {
        for complete in [true, false] {
            let root = TempDir::new().unwrap();
            let path = root.path().join("trident.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let server = tokio::spawn(
                Server::builder()
                    .add_service(StreamingServiceServer::new(Fake { complete }))
                    .serve_with_incoming(UnixListenerStream::new(listener)),
            );
            let channel = connect(&format!("unix://{}", path.display()))
                .await
                .unwrap();
            let stream = start(
                channel,
                Plan::Stream {
                    image: Url::parse("https://example.com/image.cosi").unwrap(),
                },
            )
            .await
            .unwrap();
            let (tx, mut rx) = mpsc::channel(8);
            receive(stream, &tx).await.unwrap();
            drop(tx);
            let mut trace = false;
            let mut completed = false;
            let mut uncertain = false;
            while let Some(event) = rx.recv().await {
                match event {
                    Event::Response(response) => match response.response.unwrap() {
                        ResponseBody::Log(log) => trace = log.level() == LogLevel::Trace,
                        ResponseBody::Completed(_) => completed = true,
                        _ => {}
                    },
                    Event::Error {
                        uncertain: value, ..
                    } => uncertain = value,
                    _ => {}
                }
            }
            assert!(trace);
            assert_eq!(completed, complete);
            assert_eq!(uncertain, !complete);
            server.abort();
        }
    }
}
