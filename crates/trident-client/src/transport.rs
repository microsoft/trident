use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use hyper_util::rt::TokioIo;
use tokio::{net::UnixStream, time};
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;
use url::Url;

use crate::ClientError;

const CONNECTOR_ENDPOINT: &str = "http://[::]:50051";

pub(crate) async fn connect_channel(
    endpoint: &str,
    timeout: Duration,
) -> Result<Channel, ClientError> {
    let url =
        Url::parse(endpoint).map_err(|error| invalid_endpoint(endpoint, error.to_string()))?;

    match url.scheme() {
        "unix" => connect_unix(endpoint, &url, timeout).await,
        "http" => connect_tcp(endpoint, timeout).await,
        scheme => Err(invalid_endpoint(
            endpoint,
            format!("unsupported endpoint scheme '{scheme}'"),
        )),
    }
}

async fn connect_tcp(endpoint: &str, timeout: Duration) -> Result<Channel, ClientError> {
    let transport = Endpoint::from_shared(endpoint.to_string())
        .map_err(|error| invalid_endpoint(endpoint, error.to_string()))?;
    time::timeout(timeout, transport.connect())
        .await
        .map_err(|_| connection_error(endpoint, format!("connection timed out after {timeout:?}")))?
        .map_err(|error| connection_error(endpoint, error.to_string()))
}

async fn connect_unix(
    endpoint: &str,
    url: &Url,
    timeout: Duration,
) -> Result<Channel, ClientError> {
    if url.host().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err(invalid_endpoint(
            endpoint,
            "Unix endpoints must contain only an absolute socket path",
        ));
    }
    let path = unix_socket_path(endpoint, url)?;
    if !path.is_absolute() || path == Path::new("/") {
        return Err(invalid_endpoint(
            endpoint,
            "Unix endpoint path must be absolute and non-empty",
        ));
    }

    let transport = Endpoint::from_shared(CONNECTOR_ENDPOINT.to_string())
        .map_err(|error| invalid_endpoint(endpoint, error.to_string()))?;
    let connector = service_fn(move |_: Uri| {
        let path = path.clone();
        async move { UnixStream::connect(path).await.map(TokioIo::new) }
    });
    time::timeout(timeout, transport.connect_with_connector(connector))
        .await
        .map_err(|_| connection_error(endpoint, format!("connection timed out after {timeout:?}")))?
        .map_err(|error| connection_error(endpoint, error.to_string()))
}

fn unix_socket_path(endpoint: &str, url: &Url) -> Result<PathBuf, ClientError> {
    let file_url = Url::parse(&format!("file://{}", url.path())).map_err(|error| {
        invalid_endpoint(
            endpoint,
            format!("Unix endpoint path could not be decoded: {error}"),
        )
    })?;
    file_url
        .to_file_path()
        .map_err(|()| invalid_endpoint(endpoint, "Unix endpoint path could not be decoded"))
}

fn invalid_endpoint(endpoint: &str, details: impl Into<String>) -> ClientError {
    ClientError::invalid_endpoint(
        endpoint_for_diagnostics(endpoint),
        endpoint_details_for_diagnostics(endpoint, details),
    )
}

fn connection_error(endpoint: &str, details: impl Into<String>) -> ClientError {
    ClientError::connection(
        endpoint_for_diagnostics(endpoint),
        endpoint_details_for_diagnostics(endpoint, details),
    )
}

pub(crate) fn endpoint_for_diagnostics(endpoint: &str) -> String {
    let Ok(mut url) = Url::parse(endpoint) else {
        return "<invalid endpoint>".to_string();
    };

    if !url.username().is_empty() && url.set_username("redacted").is_err() {
        return "<redacted endpoint>".to_string();
    }
    if url.password().is_some() && url.set_password(Some("redacted")).is_err() {
        return "<redacted endpoint>".to_string();
    }
    if url.query().is_some() {
        url.set_query(Some("redacted"));
    }
    if url.fragment().is_some() {
        url.set_fragment(Some("redacted"));
    }

    url.to_string()
}

fn endpoint_details_for_diagnostics(endpoint: &str, details: impl Into<String>) -> String {
    match Url::parse(endpoint) {
        Ok(url)
            if !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some() =>
        {
            "details redacted because the endpoint contains sensitive data".to_string()
        }
        _ => details.into(),
    }
}
