//! # Harpoon
//!
//! Harpoon is Trident's ACL update sidecar. Historically it was a one-shot
//! Omaha client that called Trident's combined `Update()` RPC once and exited.
//! This crate now defaults to the AKS annotation protocol described in the
//! local design doc (`aks-rp ↔ trident-acl-agent`, especially §3–§6 and
//! §12–§13), while preserving the original `omaha-only` mode as an explicit
//! opt-out (see `config::GoalSource`).

use anyhow::Context;
use semver::Version;
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

pub mod annotations;
pub mod config;
pub mod error;
pub mod id;
pub mod k8s;
pub mod omaha;
pub mod orchestrator;
pub mod state;
pub mod trident;

/// Only built for `cargo test` (relies on trident-proto's `server` feature,
/// which is only enabled via trident-acl-agent's dev-dependencies - see
/// mock_tridentd.rs's module docs).
#[cfg(test)]
pub mod mock_tridentd;

use error::HarpoonError;
use omaha::{
    event::{OmahaEvent, OmahaEventType},
    request::{AppRequest, Request},
    response::Package,
};
use trident::TridentClient;

pub use id::IdSource;
pub use omaha::event::EventResult;

pub const DEFAULT_NEBRASKA_APP_ID: &str = "b0ec8f0d-1c13-4bf4-9efd-ea54464a7098";
pub const DEFAULT_NEBRASKA_TRACK: &str = "west-us";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmahaUpdate {
    pub url: Url,
    pub version: Version,
    pub hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HarpoonQueryResponse {
    pub session_id: Uuid,
    pub result: QueryResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryResult {
    NoUpdate,
    NewDocument(OmahaUpdate),
}

pub async fn run_omaha_only(config: &config::AgentConfig) -> Result<(), anyhow::Error> {
    let endpoint = config.nebraska.endpoint.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "no Nebraska endpoint configured: pass <url> on the CLI or set [nebraska].endpoint in config.toml"
        )
    })?;

    // query_for_update() is a blocking call (reqwest::blocking under the
    // hood, see omaha::send) - calling it directly from this async fn can
    // panic ("Cannot drop a runtime in a context where blocking is not
    // allowed") because reqwest::blocking spins up its own inner Tokio
    // runtime per call, which isn't safe to tear down from inside an
    // already-running async task. Run it on a dedicated blocking thread.
    let app_id = config.nebraska.app_id.clone();
    let track = config.nebraska.track.clone();
    let endpoint_for_task = endpoint.clone();
    let response = tokio::task::spawn_blocking(move || {
        query_for_update(
            &endpoint_for_task,
            &app_id,
            &track,
            &Version::new(0, 0, 0),
            IdSource::MachineIdHashed,
        )
    })
    .await
    .context("Nebraska query task panicked")??;

    match response.result {
        QueryResult::NoUpdate => {
            log::debug!("No update available from Nebraska");
            Ok(())
        }
        QueryResult::NewDocument(update) => {
            log::info!("Triggering one-shot Omaha update to {}", update.version);
            let mut client = TridentClient::connect(&config.trident.socket).await?;
            let combined_timeout =
                config.orchestration.stage_timeout + config.orchestration.finalize_timeout;
            client
                .update(&update.url, update.hash.as_deref(), combined_timeout)
                .await?;
            Ok(())
        }
    }
}

/// Query the Omaha server at the given URL for the given app and track.
pub fn query_for_update(
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

    log::debug!(
        "Received response from Omaha server at '{}' for app '{}' on track '{}': {response:#?}",
        url,
        app_id,
        track,
    );
    if response.apps().len() != 1 {
        return Err(HarpoonError::InvalidResponse(
            "Expected exactly one app in response".to_string(),
        ));
    }

    let app = response.apps().first().expect("validated len above");

    if app.app_id() != app_id {
        return Err(HarpoonError::InvalidResponse(
            "Unexpected app ID in response".to_string(),
        ));
    }

    if app.status().is_error() {
        return Err(HarpoonError::QueryError(format!(
            "Received a non-OK app status: {}",
            app.status()
        )));
    }

    let update_check = app.update_check().ok_or_else(|| {
        HarpoonError::InvalidResponse("Missing update check in response".to_string())
    })?;
    log::debug!("Received update check response: {update_check:#?}");

    if update_check.status().is_error() {
        return Err(HarpoonError::QueryError(format!(
            "Received an error status in update check: {}",
            update_check.status()
        )));
    }

    if update_check.status().is_no_update() {
        log::debug!(
            "No update available for app '{}' v{}",
            app_id,
            document_version
        );
        return Ok(HarpoonQueryResponse {
            session_id: request.session_id(),
            result: QueryResult::NoUpdate,
        });
    }

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

    let package = update_check
        .packages()
        .first()
        .expect("validated len above");
    let package_url = update_base_url.join(&package.name).map_err(|err| {
        HarpoonError::InvalidResponse(format!("Failed to join URL with package name: {err}"))
    })?;

    log::debug!(
        "Update available for app '{}' v{} -> v{} ({})",
        app_id,
        document_version,
        new_version,
        package_url,
    );

    Ok(HarpoonQueryResponse {
        session_id: request.session_id(),
        result: QueryResult::NewDocument(OmahaUpdate {
            url: package_url,
            version: new_version.as_version().clone(),
            hash: normalized_sha384_hash(package),
        }),
    })
}

fn normalized_sha384_hash(package: &Package) -> Option<String> {
    fn is_sha384(candidate: &str) -> bool {
        candidate.len() == 96 && candidate.chars().all(|c| c.is_ascii_hexdigit())
    }

    package
        .hash_sha256
        .as_deref()
        .filter(|candidate| is_sha384(candidate))
        .map(str::to_owned)
        .or_else(|| {
            if is_sha384(&package.hash) {
                Some(package.hash.clone())
            } else {
                None
            }
        })
}

/// Downloads an update package provided by the Omaha server at the given base URL.
#[allow(unused)]
fn download_document(
    update_base_url: &Url,
    package: &Package,
    file_extension: &str,
) -> Result<(String, Url), HarpoonError> {
    if !package.name.ends_with(file_extension) {
        return Err(HarpoonError::ExpectedYamlDocument(package.name.clone()));
    }

    if package.size >= 1024 * 1024 {
        log::warn!(
            "Reported document size is larger than 1MB ({}). This may NOT be a '{}' text document.",
            package.size,
            file_extension
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

    log::trace!(
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

    if !package.hash.is_empty() {
        let actual = format!("{:x}", Sha256::digest(document.as_bytes()));
        let expected = package.hash.to_lowercase();
        log::trace!(
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

/// A wrapper to hide away the details of what Omaha events are actually relevant.
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
    fn test_query_for_update() {
        let mut server = mockito::Server::new();
        let data = "test document";

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

        let response = query_for_update(
            &Url::parse(&server.url()).unwrap(),
            "test",
            "track",
            &Version::new(0, 1, 0),
            IdSource::MachineIdHashed,
        )
        .unwrap();

        omaha_mock.assert();

        assert_eq!(
            response,
            HarpoonQueryResponse {
                session_id: response.session_id,
                result: QueryResult::NewDocument(OmahaUpdate {
                    url: Url::parse(&format!("{}/test.yaml", server.url())).unwrap(),
                    version: Version::new(1, 0, 0),
                    hash: None,
                })
            }
        );
    }

    #[test]
    fn test_query_for_update_no_update() {
        let mut server = mockito::Server::new();

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .match_body(Matcher::Regex(".*<updatecheck.*".to_string()))
            .with_body(indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="ok">
                        <updatecheck status="noupdate">
                            <urls></urls>
                        </updatecheck>
                    </app>
                </response>"#})
            .expect(1)
            .create();

        let response = query_for_update(
            &Url::parse(&server.url()).unwrap(),
            "test",
            "track",
            &Version::new(0, 1, 0),
            IdSource::MachineIdHashed,
        )
        .unwrap();

        omaha_mock.assert();
        assert!(matches!(response.result, QueryResult::NoUpdate));
    }
}
