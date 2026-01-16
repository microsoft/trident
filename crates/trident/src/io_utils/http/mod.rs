use std::{
    io::{Error as IoError, ErrorKind as IoErrorKind, Result as IoResult},
    thread,
    time::{Duration, Instant},
};

use log::{trace, warn};
use reqwest::{
    blocking::Response, header::CONTENT_LENGTH, Error as ReqwestError, Result as ReqwestResult,
    StatusCode,
};

pub mod file;
mod range;
mod subfile;

pub use file::HttpFile;
use range::HttpRangeRequest;

/// Converts an reqwest HTTP error into an IO error.
fn http_to_io_err(e: ReqwestError) -> IoError {
    let formatted = format!("HTTP File error: {e}");
    if let Some(status) = e.status() {
        match status {
            StatusCode::BAD_REQUEST => IoError::new(IoErrorKind::InvalidInput, formatted),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                IoError::new(IoErrorKind::PermissionDenied, formatted)
            }
            StatusCode::NOT_FOUND => IoError::new(IoErrorKind::NotFound, formatted),
            StatusCode::REQUEST_TIMEOUT => IoError::new(IoErrorKind::TimedOut, formatted),
            _ if status.is_server_error() => {
                IoError::new(IoErrorKind::ConnectionAborted, formatted)
            }
            _ => IoError::other(formatted),
        }
    } else if e.is_timeout() {
        IoError::new(IoErrorKind::TimedOut, formatted)
    } else if e.is_connect() {
        IoError::new(IoErrorKind::ConnectionRefused, formatted)
    } else if e.is_request() {
        IoError::new(IoErrorKind::InvalidData, formatted)
    } else {
        IoError::other(formatted)
    }
}

/// Extracts the Content-Length header from an HTTP response and parses it as
/// a u64. Returns an IO error if the header is missing or invalid.
fn get_content_length(response: &Response) -> IoResult<u64> {
    response
        .headers()
        .get(CONTENT_LENGTH)
        .ok_or_else(|| {
            IoError::new(
                IoErrorKind::InvalidData,
                "Missing Content-Length header in HTTP response",
            )
        })
        .and_then(|value| {
            value.to_str().map_err(|e| {
                IoError::new(
                    IoErrorKind::InvalidData,
                    format!("Invalid Content-Length header: {e}"),
                )
            })
        })
        .and_then(|s| {
            s.parse::<u64>().map_err(|e| {
                IoError::new(
                    IoErrorKind::InvalidData,
                    format!("Failed to parse Content-Length header: {e}"),
                )
            })
        })
}

/// Performs an HTTP request and retries it for up to `timeout` if
/// it fails. The HTTP request is created and invoked by `request_sender`, a
/// closure that that returns a `reqwest::Result<Response>`. If the request is
/// successful, it returns the response. If the request fails after all retries,
/// it returns an IO error.
fn retriable_request_sender<F>(request_sender: F, timeout: Duration) -> IoResult<Response>
where
    F: Fn() -> ReqwestResult<Response>,
{
    let mut retry = 0;
    let now = Instant::now();
    let timeout_time = now + timeout;
    let mut sleep_duration = Duration::from_millis(10);
    loop {
        if retry != 0 {
            trace!("Retrying HTTP request (attempt {})", retry + 1);
        }
        match request_sender() {
            Ok(response) => {
                if response.status().is_success() {
                    return Ok(response);
                } else if std::time::Instant::now() > timeout_time {
                    return response.error_for_status().map_err(http_to_io_err);
                } else {
                    warn!("HTTP request failed with status: {}", response.status());
                }
            }
            Err(e) => {
                if Instant::now() > timeout_time {
                    return Err(http_to_io_err(e));
                }
                warn!("HTTP request failed: {e}");
            }
        };

        // Sleep for a short duration before retrying
        if Instant::now() + sleep_duration > timeout_time {
            return Err(IoError::new(
                IoErrorKind::TimedOut,
                "HTTP request timed out",
            ));
        }

        thread::sleep(sleep_duration);
        sleep_duration *= 2;
        retry += 1;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicU16, Ordering},
        Arc,
    };

    use reqwest::{
        blocking::Client,
        header::{CONTENT_LENGTH, CONTENT_TYPE},
    };
    use url::Url;

    use super::*;

    #[test]
    fn test_retriable_request_sender_retry_count() {
        let tries = Arc::new(AtomicU16::new(0));
        let closure_tries = tries.clone();
        let request_sender = || {
            closure_tries.fetch_add(1, Ordering::SeqCst);
            let client = Client::new();
            client.get("").send()
        };

        retriable_request_sender(request_sender, Duration::from_secs(2)).unwrap_err();
        assert!(tries.load(Ordering::SeqCst) > 1);
    }

    #[test]
    fn test_retriable_request_sender_initial_failure() {
        let relative_file_path = "/test.yaml";
        let mut server = mockito::Server::new();
        let data = "test document";
        let document_mock = server
            .mock("GET", relative_file_path)
            .with_body(data)
            .with_header(CONTENT_LENGTH, &data.len().to_string())
            .with_header(CONTENT_TYPE, "text/plain")
            .with_status(200)
            .expect(1)
            .create();
        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join(relative_file_path).unwrap().to_string();

        let tries = Arc::new(AtomicU16::new(0));
        let closure_tries = tries.clone();
        let request_sender = || {
            closure_tries.fetch_add(1, Ordering::SeqCst);
            if closure_tries.load(Ordering::SeqCst) < 2 {
                let client = Client::new();
                return client.get("").send();
            }
            let client = Client::new();
            client.get(&request_url).send()
        };
        let document = retriable_request_sender(request_sender, Duration::from_secs(5))
            .unwrap()
            .text()
            .unwrap();
        assert!(tries.load(Ordering::SeqCst) > 1);
        assert_eq!(document, data);
        document_mock.assert();
    }
}
