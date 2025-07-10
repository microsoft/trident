#[cfg(test)]
use std::io::Cursor;
use std::{
    fs::File,
    io::{Error as IoError, ErrorKind as IoErrorKind, Read, Result as IoResult, Seek, SeekFrom},
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, trace, warn};
use oci_client::{secrets::RegistryAuth, Client as OciClient, Reference};
use reqwest::blocking::{Client, Response};
use tokio::runtime::Runtime;
use url::Url;

pub(super) trait ReadSeek: Read + Seek {}

impl ReadSeek for HttpFile {}
impl ReadSeek for File {}

#[cfg(test)]
impl ReadSeek for Cursor<Vec<u8>> {}

/// An abstraction over a COSI file reader that can be either a local file or an
/// HTTP request.
///
/// This abstraction contains the minimum required information to open a COSI
/// file, it does not carry any complex types and can be safely and
/// inexpensively cloned.
#[derive(Debug, Clone)]
pub(super) enum CosiReader {
    File(PathBuf),
    Http(HttpFile),

    /// Variant reserved for testing purposes only.
    #[cfg(test)]
    Mock(Cursor<Vec<u8>>),
}

impl CosiReader {
    /// Creates a new COSI file reader from the given source URL.
    pub(super) fn new(source: &Url) -> Result<Self, Error> {
        Ok(match source.scheme() {
            "file" => {
                // Load COSI from local file
                debug!("Loading COSI file: '{}'", source.path());
                let path = PathBuf::from(source.path());
                ensure!(
                    path.exists(),
                    format!("Path '{}' does not exist", path.display()),
                );
                ensure!(
                    path.is_file(),
                    format!("Path '{}' is not a file", path.display()),
                );
                Self::File(path)
            }
            "http" | "https" => {
                // Load COSI from remote URL
                debug!("Loading COSI file from URL: '{}'", source);
                Self::Http(HttpFile::new(source)?)
            }
            "oci" => {
                // Load COSI from container registry
                debug!("Loading COSI file from URL: '{}'", source);
                Self::Http(HttpFile::new_from_oci(source)?)
            }
            _ => {
                bail!("Unsupported URL scheme: {}", source.scheme());
            }
        })
    }

    /// Returns an implementation of `Read` + `Seek` over the entire COSI file.
    pub(super) fn reader(&self) -> Result<Box<dyn ReadSeek>, IoError> {
        Ok(match self {
            Self::File(file) => Box::new(File::open(file)?),
            Self::Http(http_file) => Box::new(http_file.clone()),
            #[cfg(test)]
            Self::Mock(cursor) => Box::new(cursor.clone()),
        })
    }

    /// Returns an implementation of `Read` for the given section of the COSI file.
    pub(super) fn section_reader(&self, section_offset: u64, size: u64) -> IoResult<Box<dyn Read>> {
        Ok(match self {
            Self::File(file) => {
                // Open the file and seek to the section
                let mut file = File::open(file)?;
                file.seek(SeekFrom::Start(section_offset))?;
                // Return a reader that is limited to the section size
                Box::new(file.take(size))
            }

            Self::Http(http_file) => Box::new(http_file.section_reader(section_offset, size)?),

            #[cfg(test)]
            Self::Mock(cursor) => {
                // Clone the cursor and seek to the section
                let mut cursor = cursor.clone();
                cursor.seek(SeekFrom::Start(section_offset))?;
                // Return a reader that is limited to the section size
                Box::new(cursor.take(size))
            }
        })
    }
}

/// A FILE-like object that is obtained through an HTTP request using range
/// headers instead of a local file.
///
/// It implements the `Read` and `Seek` traits to allow reading and seeking
/// through the file.
///
/// It is best used to scan through a file, like scanning tar headers, as each
/// call to `read` will result in a new HTTP request.
#[derive(Debug, Clone)]
pub struct HttpFile {
    url: Url,
    position: u64,
    size: u64,
    client: Client,
    timeout_in_seconds: u64,
    token: Option<String>,
}

impl HttpFile {
    /// Creates a new HTTP file reader from a standard HTTP URL.
    pub fn new(url: &Url) -> IoResult<Self> {
        Self::new_inner(url, None, false)
    }

    /// Creates a new HTTP file reader from an OCI URL.
    pub fn new_from_oci(url: &Url) -> Result<Self, Error> {
        let img_ref =
            Reference::try_from(url.to_string().strip_prefix("oci://").with_context(|| {
                format!("URL has incorrect scheme: expected to start with 'oci://', got '{url}'")
            })?)
            .with_context(|| format!("Failed to parse URL '{url}'"))?;

        let rt = Runtime::new().context("Failed to create Tokio runtime")?;
        let token = Self::retrieve_access_token(&img_ref, &rt)?;
        let digest = Self::retrieve_artifact_digest(&img_ref, &rt)?;
        trace!("Retrieved artifact digest: {digest}");

        // Create HTTP URL
        let registry = img_ref.registry();
        let repository = img_ref.repository();
        let http_url = Url::parse(&format!(
            "https://{registry}/v2/{repository}/blobs/{digest}"
        ))?;

        Self::new_inner(&http_url, Some(token), true).context("Failed to create HTTP file reader")
    }

    fn new_inner(
        url: &Url,
        token: Option<String>,
        ignore_ranges_header_absence: bool,
    ) -> IoResult<Self> {
        debug!("Opening HTTP file '{}'", url);
        let timeout_in_seconds = 5;

        // Create a new client for this file.
        let client = Client::new();
        let request_sender = || {
            let mut request = client.head(url.as_str());
            if let Some(token) = &token {
                request = request.header("Authorization", format!("Bearer {token}"));
            }
            request.send()
        };
        let response = Self::retriable_request_sender(request_sender, timeout_in_seconds)?;
        trace!("HTTP file '{}' has status: {}", url, response.status());

        // Get the file size from the response headers
        let size = response
            .headers()
            .get("Content-Length")
            .ok_or(IoError::new(
                IoErrorKind::Other,
                "Failed to get 'Content-Length' in the response header",
            ))?
            .to_str()
            .map_err(|e| {
                IoError::new(
                    IoErrorKind::InvalidData,
                    format!("Could not parse 'Content-Length': {e}"),
                )
            })?
            .parse()
            .map_err(|e| {
                IoError::new(
                    IoErrorKind::InvalidData,
                    format!("Could not parse 'Content-Length' as an integer: {e}"),
                )
            })?;

        trace!("HTTP file '{}' has size: {}", url, size);

        // Ensure the server supports range requests, this implementation
        // requires that feature!
        let accept_ranges_header = response.headers().get("Accept-Ranges");
        if accept_ranges_header.is_none() && ignore_ranges_header_absence {
            warn!("OCI server does not provide 'Accept-Ranges' header, continuing anyway");
        } else if accept_ranges_header
            .ok_or(IoError::new(
                IoErrorKind::Other,
                "Server does not support range requests: 'Accept-Ranges' header was not provided",
            ))?
            .to_str()
            .map_err(|e| {
                IoError::new(
                    IoErrorKind::InvalidData,
                    format!("Could not parse 'Accept-Ranges': {e}"),
                )
            })?
            .to_lowercase()
            .eq("none")
        {
            return Err(IoError::new(
                IoErrorKind::Other,
                "Server does not support range requests: 'Accept-Ranges: none'",
            ));
        }

        debug!("Successfully queried HTTP file '{}' of size: {}", url, size);
        Ok(Self {
            url: url.clone(),
            position: 0,
            size,
            client,
            timeout_in_seconds,
            token,
        })
    }

    /// Retrieve bearer token to access container registry. Even registries allowing anonymous
    /// access may require a token.
    fn retrieve_access_token(img_ref: &Reference, runtime: &Runtime) -> Result<String, Error> {
        trace!(
            "Retrieving access token for OCI registry '{}'",
            img_ref.registry()
        );
        let client = OciClient::default();
        runtime
            .block_on(client.auth(
                img_ref,
                &RegistryAuth::Anonymous,
                oci_client::RegistryOperation::Pull,
            ))
            .with_context(|| {
                format!(
                    "Registry '{}' is not accessible or does not exist",
                    img_ref.registry()
                )
            })?
            .context("Failed to retrieve authorization token")
    }

    /// Retrieve artifact digest, which is necessary to send HTTP request to container registry.
    fn retrieve_artifact_digest(img_ref: &Reference, runtime: &Runtime) -> Result<String, Error> {
        Ok(match img_ref.digest() {
            Some(digest) => digest.to_string(),
            None => {
                let tag = img_ref.tag().with_context(|| {
                    format!("Failed to retrieve tag from OCI URL '{}'", img_ref.whole())
                })?;
                // Attempt to retrieve digest from manifest
                let client = OciClient::default();
                let manifest = client.pull_image_manifest(img_ref, &RegistryAuth::Anonymous);
                let (oci_image_manifest, _) = runtime.block_on(manifest).with_context(||
                    format!(
                        "Repository '{}' does not exist in registry '{}' or tag '{tag}' not found in repository",
                        img_ref.repository(),
                        img_ref.registry()
                    ))?;
                // Expect the artifact to have one layer, which is the image
                ensure!(
                    oci_image_manifest.layers.len() == 1,
                    format!(
                        "Expected OCI artifact to contain 1 layer, found {}",
                        oci_image_manifest.layers.len()
                    )
                );
                oci_image_manifest.layers[0].digest.clone()
            }
        })
    }

    /// Converts an HTTP error into an IO error.
    fn http_to_io_err(e: reqwest::Error) -> IoError {
        let formatted = format!("HTTP File error: {e}");
        if let Some(status) = e.status() {
            match status.as_u16() {
                400 => IoError::new(IoErrorKind::InvalidInput, formatted),
                401 | 403 => IoError::new(IoErrorKind::PermissionDenied, formatted),
                404 => IoError::new(IoErrorKind::NotFound, formatted),
                408 => IoError::new(IoErrorKind::TimedOut, formatted),
                500..=599 => IoError::new(IoErrorKind::ConnectionAborted, formatted),
                _ => IoError::new(IoErrorKind::Other, formatted),
            }
        } else if e.is_timeout() {
            IoError::new(IoErrorKind::TimedOut, formatted)
        } else if e.is_connect() {
            IoError::new(IoErrorKind::ConnectionRefused, formatted)
        } else if e.is_request() {
            IoError::new(IoErrorKind::InvalidData, formatted)
        } else {
            IoError::new(IoErrorKind::Other, formatted)
        }
    }

    /// Performs a request with optional range headers to get the file content.
    /// Returns the HTTP response.
    fn reader(&self, start: Option<u64>, end: Option<u64>) -> IoResult<Response> {
        let request_sender = || {
            let mut request = self.client.get(self.url.as_str());

            if let Some(token) = self.token.clone() {
                request = request.header("Authorization", format!("Bearer {token}"));
            }

            // Generate the range header when appropriate
            let range_header = match (start, end) {
                (Some(start), Some(end)) => Some(format!("bytes={start}-{end}")),
                (Some(start), None) => Some(format!("bytes={start}-")),
                (None, Some(end)) => Some(format!("bytes=0-{end}")),
                (None, None) => None,
            };

            // Add the range header to the request
            if let Some(range) = range_header {
                request = request.header("Range", range);
            }

            request.send()
        };

        Self::retriable_request_sender(request_sender, self.timeout_in_seconds)
    }

    /// Performs an HTTP request and retries it for up to `timeout_in_seconds` if
    /// it fails. The HTTP request is created and invoked by `request_sender`, a
    /// closure that that returns a `reqwest::Result<Response>`. If the request is
    /// successful, it returns the response. If the request fails after all retries,
    /// it returns an IO error.
    fn retriable_request_sender<F>(request_sender: F, timeout_in_seconds: u64) -> IoResult<Response>
    where
        F: Fn() -> reqwest::Result<Response>,
    {
        let mut retry = 0;
        let now = Instant::now();
        let timeout_time = now + Duration::from_secs(timeout_in_seconds);
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
                        return response.error_for_status().map_err(Self::http_to_io_err);
                    } else {
                        warn!("HTTP request failed with status: {}", response.status());
                    }
                }
                Err(e) => {
                    if Instant::now() > timeout_time {
                        return Err(Self::http_to_io_err(e));
                    }
                    warn!("HTTP request failed: {}", e);
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

    /// Performs a request of a specific section of the file. Returns the HTTP
    /// response.
    fn section_reader(&self, section_offset: u64, size: u64) -> IoResult<Response> {
        let end = section_offset + size - 1;
        trace!(
            "Reading HTTP file '{}' from {} to {} (inclusive) [{} bytes]",
            self.url,
            section_offset,
            end,
            size
        );

        let response = self.reader(Some(section_offset), Some(end))?;

        if let Some(data) = response.headers().get("Content-Range") {
            trace!(
                "Returned content range: {:?}",
                String::from_utf8_lossy(data.as_bytes())
            );
        }

        Ok(response)
    }
}

impl Seek for HttpFile {
    /// Implements seeking for the HTTP file reader.
    ///
    /// This implementation strictly forbids seeking after the end of the file.
    fn seek(&mut self, pos: std::io::SeekFrom) -> IoResult<u64> {
        let add_relative = |base: u64, delta: i64| -> IoResult<u64> {
            Ok(if delta < 0 {
                let neg_delta = -delta as u64;
                if base < neg_delta {
                    return Err(IoError::new(
                        IoErrorKind::InvalidInput,
                        "Cannot seek before the beginning of the file",
                    ));
                }
                base - neg_delta
            } else if let Some(new_base) = base.checked_add(delta as u64) {
                new_base
            } else {
                return Err(IoError::new(
                    IoErrorKind::InvalidInput,
                    "New file position is too large",
                ));
            })
        };

        let new_pos = match pos {
            std::io::SeekFrom::Start(pos) => pos,
            std::io::SeekFrom::End(pos) => add_relative(self.size, pos)?,
            std::io::SeekFrom::Current(pos) => add_relative(self.position, pos)?,
        };

        if new_pos >= self.size {
            return Err(IoError::new(
                IoErrorKind::InvalidInput,
                "New file position is beyond the end of the file",
            ));
        }

        trace!(
            "Seeking HTTP file '{}' to position {} after seek: {:?}",
            self.url,
            new_pos,
            pos
        );

        self.position = new_pos;

        Ok(self.position)
    }
}

impl Read for HttpFile {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let mut response = self.section_reader(self.position, buf.len() as u64)?;
        let res = response.read(buf)?;
        self.position += res as u64;
        Ok(res)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> IoResult<()> {
        let mut response = self.section_reader(self.position, buf.len() as u64)?;
        response.read_exact(buf)?;
        self.position += buf.len() as u64;
        Ok(())
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> IoResult<usize> {
        let mut response = self.reader(Some(self.position), None)?;
        let res = response.read_to_end(buf)?;
        self.position += res as u64;
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        io::{SeekFrom, Write},
        sync::{
            atomic::{AtomicU16, Ordering},
            Arc,
        },
    };

    use tempfile::NamedTempFile;

    #[test]
    fn test_retrieve_access_token() {
        let rt = Runtime::new().unwrap();
        let url = "oci://docker.io/library/hello-world:latest".to_string();
        let img_ref = url
            .strip_prefix("oci://")
            .and_then(|url| url.parse::<Reference>().ok())
            .unwrap();
        HttpFile::retrieve_access_token(&img_ref, &rt).unwrap();
    }

    #[test]
    fn test_retrieve_artifact_digest() {
        let rt = Runtime::new().unwrap();
        // TODO(12732): Fix this test to use test COSI file instead of hello-world image
        let url = "oci://docker.io/library/hello-world@sha256:940c619fbd418f9b2b1b63e25d8861f9cc1b46e3fc8b018ccfe8b78f19b8cc4f".to_string();
        let img_ref = url
            .strip_prefix("oci://")
            .and_then(|url| url.parse::<Reference>().ok())
            .unwrap();
        assert_eq!(
            HttpFile::retrieve_artifact_digest(&img_ref, &rt).unwrap(),
            "sha256:940c619fbd418f9b2b1b63e25d8861f9cc1b46e3fc8b018ccfe8b78f19b8cc4f"
        );
    }

    #[test]
    fn test_retriable_request_sender_retry_count() {
        let tries = Arc::new(AtomicU16::new(0));
        let closure_tries = tries.clone();
        let request_sender = || {
            closure_tries.fetch_add(1, Ordering::SeqCst);
            let client = Client::new();
            client.get("").send()
        };
        HttpFile::retriable_request_sender(request_sender, 2).unwrap_err();
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
            .with_header("content-length", &data.len().to_string())
            .with_header("content-type", "text/plain")
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
        let document = HttpFile::retriable_request_sender(request_sender, 5)
            .unwrap()
            .text()
            .unwrap();
        assert!(tries.load(Ordering::SeqCst) > 1);
        assert_eq!(document, data);
        document_mock.assert();
    }

    #[test]
    fn test_http_file_seek() {
        let mut http_file = HttpFile {
            url: Url::parse("http://example.com").unwrap(),
            position: 0,
            size: 100, // We have indices from 0 to 99
            client: Client::new(),
            timeout_in_seconds: 1,
            token: None,
        };

        assert_eq!(http_file.seek(SeekFrom::Start(50)).unwrap(), 50);
        assert_eq!(http_file.position, 50);

        assert_eq!(http_file.seek(SeekFrom::End(-1)).unwrap(), 99);
        assert_eq!(http_file.position, 99);

        assert_eq!(http_file.seek(SeekFrom::End(-50)).unwrap(), 50);
        assert_eq!(http_file.position, 50);

        assert_eq!(http_file.seek(SeekFrom::Current(49)).unwrap(), 99);
        assert_eq!(http_file.position, 99);

        assert_eq!(http_file.seek(SeekFrom::Current(-50)).unwrap(), 49);
        assert_eq!(http_file.position, 49);

        // Internally calls .seek(SeekFrom::Current(0))
        assert_eq!(http_file.stream_position().unwrap(), 49);
        assert_eq!(http_file.position, 49);

        // Return to the beginning
        http_file.seek(SeekFrom::Start(0)).unwrap();

        // Now test errors

        // This implementation strictly forbids seeking after the end of the file
        http_file.seek(SeekFrom::End(0)).unwrap_err();
        assert_eq!(http_file.position, 0);

        http_file.seek(SeekFrom::Start(100)).unwrap_err();
        assert_eq!(http_file.position, 0);

        http_file.seek(SeekFrom::End(1)).unwrap_err();
        assert_eq!(http_file.position, 0);

        http_file.seek(SeekFrom::End(-101)).unwrap_err();
        assert_eq!(http_file.position, 0);

        http_file.seek(SeekFrom::Current(500)).unwrap_err();
        assert_eq!(http_file.position, 0);

        http_file.seek(SeekFrom::Current(-1)).unwrap_err();
        assert_eq!(http_file.position, 0);
    }

    #[test]
    fn test_cosi_reader_factory_file() {
        let mut file = NamedTempFile::new().expect("Failed to create temp file");
        let original_data: &'static str = "Hello, World!";
        assert_eq!(
            file.write(original_data.as_bytes())
                .expect("Failed to write data to file"),
            original_data.len(),
            "Did not write expected number of bytes"
        );
        file.flush().expect("Failed to flush file");

        let url = Url::from_file_path(file.path()).expect("Failed to create file:// URL");
        let reader_factory = CosiReader::new(&url).unwrap();

        // Check full file reader
        let mut reader = reader_factory.reader().expect("Failed to create reader");
        let mut buf = String::new();
        let read = reader
            .read_to_string(&mut buf)
            .expect("Failed to read data from file");
        assert_eq!(
            read as u64,
            original_data.len() as u64,
            "Did not read expected number of bytes, expected {} but got {}.",
            original_data.len(),
            read
        );
        assert_eq!(
            original_data, &buf,
            "Did not read expected data, expected '{original_data}' but got '{buf}'"
        );

        // Helper to check specific sections
        let check_section = |start: u64, size: u64| {
            let expected_slice = &original_data[start as usize..(start + size) as usize];
            let mut reader = reader_factory
                .section_reader(start, size)
                .expect("Failed to create section reader");
            let mut buf = String::new();
            let read = reader
                .read_to_string(&mut buf)
                .expect("Failed to read data from file");
            assert_eq!(
                read as u64, size,
                "Did not read expected number of bytes, expected {size} but got {read}. Buffer '{buf}'"
            );
            assert_eq!(
                expected_slice, &buf,
                "Did not read expected slice, expected '{expected_slice}' but got '{buf}'"
            );
        };
        check_section(0, 5); // Hello
        check_section(7, 6); // World!
        check_section(3, 6); // llo, W
        check_section(0, 13); // Hello, World!
    }

    #[test]
    fn test_http_file() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();
        // Sample body of data that will be served
        let body = "Lorem ipsum dolor sit amet, consectetur adipiscing elit. \
            Mauris dolor massa, ultrices vitae urna et, volutpat euismod dui. Cras \
            nisi ipsum, tristique eu nibh eu, varius feugiat mi. Sed id urna aliquam, \
            sollicitudin lorem quis, imperdiet sem. Vestibulum in mauris quis velit \
            suscipit bibendum. Phasellus faucibus eros sed gravida pulvinar.";

        // Request a new server from the pool
        let mut server = mockito::Server::new();

        let file_name = "/file.cosi";

        // Mock the HEAD request to get the file size.
        let size_mock = server
            .mock("HEAD", file_name)
            .with_status(200)
            .with_header("Content-Length", &body.len().to_string())
            .with_header("Accept-Ranges", "bytes")
            // We expect this to be called exactly once
            .expect(1)
            .create();

        // Mock the GET request for the full content without range.
        let mock_full = server
            .mock("GET", file_name)
            .match_header("Range", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(body)
            .expect(1)
            .create();

        // Mock the GET request to get the file content.
        let mock_range = server
            .mock("GET", file_name)
            .match_header(
                "Range",
                mockito::Matcher::Regex(r"^bytes=\d*-\d*$".to_string()),
            )
            .with_status(200)
            .with_body_from_request(|req| {
                let ranges = req.header("Range")[0]
                    .to_str()
                    .unwrap()
                    .strip_prefix("bytes=")
                    .unwrap()
                    .split('-')
                    .collect::<Vec<&str>>();
                let start = if ranges[0].is_empty() {
                    0
                } else {
                    ranges[0].parse::<usize>().expect("Failed to parse start")
                };
                let end = if ranges[1].is_empty() {
                    body.len()
                } else {
                    ranges[1].parse::<usize>().expect("Failed to parse end")
                }
                .min(body.len() - 1);
                trace!("Mocking range {} to {}", start, end);
                body.as_bytes()[start..=end].to_vec()
            })
            .expect(9)
            .create();

        let file_url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();

        // Create a new HTTP Cosi reader.
        let cosi_reader = CosiReader::new(&file_url).unwrap();

        // Get a reference to the inner HTTP file reader
        let CosiReader::Http(ref http_file) = cosi_reader else {
            panic!("Expected a HTTP file reader, got {cosi_reader:?}");
        };

        // Clone the file to test that the server is only called once.
        let _ = http_file.clone();
        // This function also just clones the http_file.
        let _ = cosi_reader.reader().unwrap();

        // Check that size_mock was called exactly once
        size_mock.assert();

        // Test read the whole thing, do not specify a range.
        let mut buf = String::new();
        let read = http_file
            .reader(None, None)
            .unwrap()
            .read_to_string(&mut buf)
            .unwrap();
        assert_eq!(read, body.len(), "Did not read expected number of bytes");
        assert_eq!(buf, body, "Did not read expected data");
        mock_full.assert();

        // Test reading a specific section
        let test_section = |start: usize, size: usize| {
            trace!(
                "Testing section {} to {} (inclusive)",
                start,
                start + size - 1
            );
            let expected_slice = &body[start..start + size];
            let mut buf = String::new();
            let read = http_file
                .section_reader(start as u64, size as u64)
                .unwrap()
                .read_to_string(&mut buf)
                .unwrap();
            trace!("Read: '{}' ({} bytes)", buf, read);
            assert_eq!(read, size, "Did not read expected number of bytes");
            assert_eq!(buf, expected_slice, "Did not read expected data");
        };

        test_section(0, 5); // Lorem
        test_section(7, 5); // ipsum
        test_section(3, 5); // em ip
        test_section(0, 13); // Lorem ipsum d
        test_section(45, 10); // scing elit
        test_section(0, body.len()); // Whole file

        // Test Read trait
        // Clone into a mutable variable to test the Read trait
        let mut http_file = http_file.clone();

        // read_to_end
        let mut buf = Vec::new();
        http_file.seek(SeekFrom::Start(0)).unwrap();
        let read = http_file.read_to_end(&mut buf).unwrap();
        assert_eq!(read, body.len(), "Did not read expected number of bytes");
        assert_eq!(buf, body.as_bytes(), "Did not read expected data");

        // read_exact
        let mut buf = vec![0; body.len()];
        http_file.seek(SeekFrom::Start(0)).unwrap();
        http_file.read_exact(&mut buf).unwrap();
        assert_eq!(buf, body.as_bytes(), "Did not read expected data");

        // read
        let mut buf = vec![0; body.len()];
        http_file.seek(SeekFrom::Start(0)).unwrap();
        let read = http_file.read(&mut buf).unwrap();
        assert_eq!(read, body.len(), "Did not read expected number of bytes");
        assert_eq!(buf, body.as_bytes(), "Did not read expected data");

        // Check that we made the exact number of requests we expected
        mock_range.assert();
    }
}
