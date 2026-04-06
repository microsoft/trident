//! # Harpoon
//!
//! Harpoon is a lightweight Omaha protocol client for documents. It queries a
//! server at a given address for a specific app and track to fetch an updated
//! document.
//!
//! This crate is specifically meant to function as an Omaha client for Trident
//! to fetch updated Host Configuration documents.
//!
//! <img src="../logo.jpeg" width="200px"/>
//!

use clap::{Parser, Subcommand};
use futures::StreamExt;
use log::{debug, error, info, trace, warn, LevelFilter};
use semver::Version;
use sha2::{Digest, Sha256};
use tonic::{transport::Endpoint, Streaming};
use trident_proto::v1::{
    servicing_response::Response as ResponseBody, update_service_client::UpdateServiceClient,
    FinalizeUpdateRequest, HostConfiguration, LogLevel, RebootHandling, RebootManagement,
    ServicingResponse, StageUpdateRequest, StatusCode, UpdateRequest,
};
use url::Url;
use uuid::Uuid;

pub mod error;
pub mod id;
pub mod omaha;

use error::HarpoonError;
use omaha::{
    event::{OmahaEvent, OmahaEventType},
    request::{AppRequest, Request},
    response::Package,
};

pub use id::IdSource;
pub use omaha::event::EventResult;

#[derive(Debug, PartialEq, Eq)]
pub struct HarpoonQueryResponse {
    pub session_id: Uuid,
    pub result: QueryResult,
}

#[derive(Debug, PartialEq, Eq)]
pub enum QueryResult {
    NoUpdate,
    NewDocument { url: Url, version: Version },
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Logging verbosity [OFF, ERROR, WARN, INFO, DEBUG, TRACE]
    #[arg(global = true, short, long, default_value_t = LevelFilter::Debug)]
    pub verbosity: LevelFilter,

    #[command(subcommand)]
    trigger: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Trigger {
        #[arg()]
        url: Url,

        #[arg()]
        hash: Option<String>,
    },
    Run {
        url: Url,
        app_id: String,
        track: String,
        document_version: Version,
    },
}

fn main() {
    let args = Args::parse();

    if let Some(Ok(journal_logger)) =
        systemd_journal_logger::connected_to_journal().then(systemd_journal_logger::JournalLog::new)
    {
        journal_logger
            .install()
            .expect("Failed to install systemd journal logger");
        log::set_max_level(args.verbosity);
    } else {
        env_logger::builder()
            .format_timestamp(None)
            .filter_level(args.verbosity)
            .init();
    }

    match args.trigger {
        Some(Command::Trigger { url, hash }) => {
            debug!("Triggering update with URL: {url} and hash: {hash:?}");
            //trigger(&url, hash).await.unwrap();
        }
        Some(Command::Run {
            url,
            app_id,
            track,
            document_version,
        }) => {
            debug!("Running update check for URL: {url}, App ID: {app_id}, Track: {track}, Document Version: {document_version}");
        }
        None => {
            debug!("No command provided. Exiting.");

            let r = query_and_fetch_yaml_document(
                &Url::parse("https://nebraska-poc-ep-cda8e2czfnhahxfk.b01.azurefd.net/v1/update")
                    .unwrap(),
                "b0ec8f0d-1c13-4bf4-9efd-ea54464a7098",
                "west-us",
                &Version::new(3, 1, 0),
                IdSource::MachineIdHashed,
            )
            .expect("Failed to query Omaha server");

            debug!("Query result: {r:?}");
        }
    }
}

async fn trigger(url: &Url, hash: Option<String>) -> Result<(), anyhow::Error> {
    // For now, we will just log the trigger. In the future, this function can be
    // used to trigger an update check on the server side, for example by sending
    // a specific event or making a specific API call to the server.
    debug!("Triggering update with URL: {url} and hash: {hash:?}");

    let channel = Endpoint::new(trident_proto::TRIDENT_DEFAULT_SOCKET_URI)?
        .connect()
        .await?;
    let mut client = UpdateServiceClient::new(channel);

    let response = client
        .update(tonic::Request::new(UpdateRequest {
            stage: Some(StageUpdateRequest {
                config: Some(HostConfiguration {
                    config: match hash {
                        Some(hash) => format!("image:\n  url: {url}\n  sha384: {hash}"),
                        None => {
                            format!("image:\n  url: {url}\n  sha384: ignored")
                        }
                    },
                }),
            }),
            finalize: Some(FinalizeUpdateRequest {
                reboot: Some(RebootManagement {
                    handling: RebootHandling::CallerHandlesReboot.into(),
                }),
            }),
        }))
        .await?;

    handle_servicing_stream(response.into_inner()).await
}

async fn handle_servicing_stream(
    mut stream: Streaming<ServicingResponse>,
) -> Result<(), anyhow::Error> {
    // Iterate through the stream until we get a Completed message
    loop {
        match stream.next().await {
            Some(Ok(response)) => match response.response {
                Some(ResponseBody::Started(_)) => {
                    info!("[Trident] Install started");
                    // Continue to next message
                }
                Some(ResponseBody::Log(log)) => {
                    let msg = format!("[Trident] {}", log.message);
                    match log.level() {
                        LogLevel::Unspecified | LogLevel::Trace => trace!("{msg}"),
                        LogLevel::Debug => debug!("{msg}"),
                        LogLevel::Info => info!("{msg}"),
                        LogLevel::Warn => warn!("{msg}"),
                        LogLevel::Error => error!("{msg}"),
                    }
                }
                Some(ResponseBody::Completed(final_status)) => {
                    if final_status.status() == StatusCode::Success {
                        info!(
                            "Trident install succeeded: status={:?}",
                            final_status.status()
                        );
                        break Ok(());
                    } else {
                        error!("Trident install failed: status={:?}", final_status.status());
                        match final_status.error {
                            Some(err) => {
                                error!("Trident reported error: {}", err.message);
                                break Err(anyhow::anyhow!(err.message));
                            }
                            None => {
                                break Err(anyhow::anyhow!("Trident install failed"));
                            }
                        }
                    }
                }
                None => {
                    // Empty response, continue
                    continue;
                }
            },
            Some(Err(e)) => {
                break Err(anyhow::anyhow!("Error reading from Trident stream: {e}"));
            }
            None => {
                break Err(anyhow::anyhow!(
                    "Trident install stream ended without control message"
                ));
            }
        }
    }
}

/// Query the Omaha server at the given URL for the given app and track to fetch
/// an updated YAML document.
///
/// Returns the session ID and the result of the query. If an update is
/// available, the new version and the updated document are returned.
///
/// This function should ONLY be used for querying YAML documents (i.e YAML text
/// files) because the whole file will be downloaded, and the function will only
/// look at the first package returned by the omaha server to fetch the
/// document. The function expects the document to be a single file with `.yaml`
/// extension.
pub fn query_and_fetch_yaml_document(
    url: &Url,
    app_id: &str,
    track: &str,
    document_version: &Version,
    machine_id_source: IdSource,
) -> Result<HarpoonQueryResponse, HarpoonError> {
    let request = Request::default().with_app(
        AppRequest::new(app_id, document_version, track, machine_id_source)?.with_update_check(),
    );

    let response = omaha::send(url, &request)?;

    debug!(
        "Received response from Omaha server at '{url}' for app '{app_id}' on track '{track}': {response:#?}",
        url = url,
        app_id = app_id,
        track = track,
        response = response
    );
    if response.apps().len() != 1 {
        return Err(HarpoonError::InvalidResponse(
            "Expected exactly one app in response".to_string(),
        ));
    }

    let app = response.apps().first().unwrap();

    if app.app_id() != app_id {
        return Err(HarpoonError::InvalidResponse(
            "Unexpected app ID in response".to_string(),
        ));
    }

    if app.status().is_error() {
        return Err(HarpoonError::QueryError(format!(
            "Received a non-OK app status: {0}",
            app.status()
        )));
    }

    let update_check = app.update_check().ok_or_else(|| {
        HarpoonError::InvalidResponse("Missing update check in response".to_string())
    })?;
    debug!("Received update check response: {update_check:#?}");

    if update_check.status().is_error() {
        return Err(HarpoonError::QueryError(format!(
            "Received an error status in update check: {0}",
            update_check.status()
        )));
    }

    if update_check.status().is_no_update() {
        // Successfully checked that there is no update available!
        debug!(
            "No update available for app '{}' v{}",
            app_id, document_version
        );
        return Ok(HarpoonQueryResponse {
            session_id: request.session_id(),
            result: QueryResult::NoUpdate,
        });
    }

    // If we got here, an update is available!
    let new_version = update_check.version().ok_or_else(|| {
        HarpoonError::InvalidResponse("Missing new version in update check response".to_string())
    })?;

    let update_base_url = update_check.urls().next().ok_or_else(|| {
        HarpoonError::InvalidResponse("Missing URL in update check response".to_string())
    })?;

    if update_check.packages().len() != 1 {
        return Err(HarpoonError::InvalidResponse(
            "Expected exactly one package in update check response".to_string(),
        ));
    }

    let package_url = update_base_url
        .join(&update_check.packages().first().unwrap().name)
        .map_err(|err| {
            HarpoonError::InvalidResponse(format!("Failed to join URL with package name: {err}"))
        })?;

    // // Send an Update Download Finished event to the server as best effort. Do
    // // not fail the query if the event fails to send.
    // if let Err(err) = report_omaha_event(
    //     url,
    //     app_id,
    //     track,
    //     OmahaEventType::UpdateDownloadFinished,
    //     match response_result {
    //         Ok(_) => EventResult::Success,
    //         Err(_) => EventResult::Error,
    //     },
    //     machine_id_source,
    // ) {
    //     error!("Failed to send UpdateDownloadFinished event to server at '{url}': {err}");
    // }

    // Now let's report that we dowloaded the update!
    debug!(
        "Downloaded update for app '{}' v{} to v{}",
        app_id, document_version, new_version
    );
    debug!("Document URL: {package_url}");

    Ok(HarpoonQueryResponse {
        session_id: request.session_id(),
        result: QueryResult::NewDocument {
            url: package_url,
            version: new_version.as_version().clone(),
        },
    })
}

/// Downloads an update package provided by the Omaha server at the given base
/// URL.
///
/// On success, returns the document as a string and the URL from which it was
/// downloaded.
///
/// The function takes care of validating the size and hash of the downloaded
/// document.
fn download_document(
    update_base_url: &Url,
    package: &Package,
    file_extension: &str,
) -> Result<(String, Url), HarpoonError> {
    if !package.name.ends_with(file_extension) {
        return Err(HarpoonError::ExpectedYamlDocument(package.name.clone()));
    }

    // If the package size is larger than 1MB, log a warning. This may mean that
    // we are not downloading the correct document.
    if package.size >= 1024 * 1024 {
        warn!(
            "Reported document size is larger than 1MB ({}). This may NOT be a '{}' text document.",
            package.size, file_extension
        );
    }

    let package_url = update_base_url.join(&package.name).map_err(|err| {
        HarpoonError::InvalidResponse(format!("Failed to join URL with package name: {err}"))
    })?;

    let document = reqwest::blocking::Client::new()
        .get(package_url.clone())
        .send()
        .map_err(|err| HarpoonError::FetchError(err.to_string()))?
        .text()
        .map_err(|err| HarpoonError::FetchError(err.to_string()))?;

    // Check that the downloaded document size matches the package size.
    trace!(
        "Validating document size: actual [{}] == expected [{}]",
        document.len(),
        package.size
    );
    if package.size != document.len() as u64 {
        return Err(HarpoonError::FetchError(format!(
            "Downloaded document size does not match package size: {} != {}",
            document.len(),
            package.size
        )));
    }

    // If we have a hash, validate it.
    if !package.hash.is_empty() {
        let actual = format!("{:x}", Sha256::digest(document.as_bytes()));
        let expected = package.hash.to_lowercase();
        trace!(
            "Validating document hash: actual [{}] == expected [{}]",
            actual,
            expected
        );
        if actual != expected {
            return Err(HarpoonError::FetchError(format!(
                "Downloaded document hash does not match package hash: {actual} != {expected}"
            )));
        }
    }

    Ok((document, package_url))
}

/// A wrapper to hide away the details of what Omaha events are actually
/// relevant. Trident only needs to know about Install and Update events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    Install,
    Update,
}

impl From<EventType> for OmahaEventType {
    fn from(event_type: EventType) -> Self {
        match event_type {
            EventType::Install => OmahaEventType::EventUpdateInstalled,
            EventType::Update => OmahaEventType::UpdateComplete,
        }
    }
}

/// Reports an Omaha event to the server at the given URL for the given app and
/// track.
fn report_omaha_event(
    url: &Url,
    app_id: &str,
    track: &str,
    event: OmahaEventType,
    result: EventResult,
    machine_id_source: IdSource,
) -> Result<(), HarpoonError> {
    omaha::send_event(
        url,
        &Request::default().with_app(
            AppRequest::new_event(app_id, track, machine_id_source)?
                .with_event(OmahaEvent::new(event, result)),
        ),
    )?;
    Ok(())
}

/// Reports a generic event to the Omaha server at the given URL for the given
/// app and track.
pub fn report_event(
    url: &Url,
    app_id: &str,
    track: &str,
    event: EventType,
    result: EventResult,
    machine_id_source: IdSource,
) -> Result<(), HarpoonError> {
    report_omaha_event(url, app_id, track, event.into(), result, machine_id_source)
}

#[cfg(test)]
mod tests {
    use mockito::Matcher;

    use super::*;

    #[test]
    fn test_download_document() {
        let mut server = mockito::Server::new();

        let data = "test document";

        let document_mock = server
            .mock("GET", "/test.yaml")
            .with_body(data)
            .with_header("content-length", &data.len().to_string())
            .with_header("content-type", "text/plain")
            .with_status(200)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let package = Package {
            name: "test.yaml".to_string(),
            size: 13,
            hash: format!("{:x}", Sha256::digest(data.as_bytes())),
            hash_sha256: None,
            required: true,
        };

        let (document, package_url) = download_document(&url, &package, ".yaml").unwrap();

        document_mock.assert();

        assert_eq!(document, data);

        assert_eq!(
            package_url,
            Url::parse(&format!("{}/test.yaml", server.url())).unwrap()
        );
    }

    #[test]
    fn test_query_and_fetch_document() {
        let mut server = mockito::Server::new();

        let data = "test document";

        let document_mock = server
            .mock("GET", "/test.yaml")
            .with_body(data)
            .with_header("content-length", &data.len().to_string())
            .with_header("content-type", "text/plain")
            .with_status(200)
            .expect(1)
            .create();

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .match_body(Matcher::Regex(".*<updatecheck.*".to_string()))
            .with_body(format!(
                indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="ok">
                        <updatecheck status="ok">
                            <urls>
                                <url codebase="{}"/>
                            </urls>
                            <manifest version="1.0.0">
                                <packages>
                                    <package hash="{:x}" name="test.yaml" size="{}" required="true"/>
                                </packages>
                            </manifest>
                        </updatecheck>
                    </app>
                </response>"#},
                server.url(),
                Sha256::digest(data.as_bytes()),
                data.len()
            ))
            .expect(1)
            .create();

        let omaha_event_mock = server
            .mock("POST", "/")
            .with_status(200)
            .match_body(Matcher::Regex(".*<event.*".to_string()))
            .with_body(indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="ok">
                        <event status="ok"/>
                    </app>
                </response>"#})
            .expect(1)
            .create();

        let response = query_and_fetch_yaml_document(
            &Url::parse(&server.url()).unwrap(),
            "test",
            "track",
            &Version::new(0, 1, 0),
            IdSource::MachineIdHashed,
        )
        .unwrap();

        document_mock.assert();
        omaha_mock.assert();
        omaha_event_mock.assert();

        assert_eq!(
            response,
            HarpoonQueryResponse {
                session_id: response.session_id,
                result: QueryResult::NewDocument {
                    url: Url::parse(&format!("{}/test.yaml", server.url())).unwrap(),
                    version: Version::new(1, 0, 0),
                    document: data.to_string(),
                }
            }
        );
    }
}
