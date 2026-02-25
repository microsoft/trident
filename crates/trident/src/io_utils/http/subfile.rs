use std::{
    io::{Read, Result as IoResult},
    ops::Not,
    thread,
    time::Duration,
};

use log::{trace, warn};
use reqwest::{
    blocking::{Client, Response},
    header::{AUTHORIZATION, RANGE},
};
use url::Url;

use super::HttpRangeRequest;

/// HttpSubFile represents a subfile located entirely and contiguously within a
/// single HTTP resource. It implements `Read` to read only the specified byte
/// range from the resource. It uses HTTP Range requests to fetch only the
/// needed data, and can handle performing multiple requests as needed when
/// reading in case the server cannot provide the full subfile at once.
///
/// HttpSubFile is optimistic and will always attempt to read all data in a
/// single request first, so on good cases it is functionally equivalent to just
/// doing one HTTP request directly. If the requested `read()` call cannot be
/// satisfied in a single request, it will transparently perform additional
/// requests as needed to satisfy the read.
///
/// Because of this behavior, HttpSubFile is resilient to transient network
/// errors that may occur during reading. If a read fails due to a network
/// error, it will discard the current request and re-issue a new request for
/// the remaining data, allowing the read to continue without requiring the
/// caller to restart the read from the beginning.
///
/// For example, assume there is a file of 8 KiB, and we want to read the
/// subfile from byte 256 to byte 4095 inclusive (3840 bytes). The first read
/// request will always attempt to read bytes 256-4095 in a single HTTP Range
/// request. Then, the server responds with a partial response of only 1536
/// bytes because it cannot provide more. The HttpSubFile will read those bytes
/// and then continue issues additional HTTP Range requests for the remaining
/// bytes (bytes 1792-4095) until the full subfile has been read. If during this
/// process a network error occurs, it will discard the current response and
/// re-issue a new request for the remaining bytes, allowing the read to
/// continue. The sequence of operations would look like this:
///
/// ```text
/// Full file:    |<------------------------- 8 KiB ------------------------>|
/// Subfile:              |<--- 3840 bytes ---->| (from byte 256 to byte 4095 inclusive)
///
/// read() call #1:       |<--->|                 (1 KiB buffer)
/// First request:        |<--- 3840 bytes ---->| (attempt to read full subfile)
/// Server response:      |<------>|              (only 1536 bytes read)
/// Bytes read so far:    |<--->|                 (1 KiB read by caller)
///
/// read() call #2:             |<--->|           (1 KiB buffer)
/// Consume from response:      |<>|              (512 bytes read from existing response)
/// Next request:                  |<---------->| (bytes 1792-4095)
/// Server response:               |<---------->| (full remaining bytes)
/// Consume from response:         |<>|           (512 bytes read from new response)
/// Bytes read so far:    |<--------->|           (2 KiB read by caller)
///
/// read() call #3:                   |<--->|     (1 KiB buffer)
/// Consume from response:            |<-X        (an error occurs after 512 bytes)
/// Next request:                     |<------->| (bytes 2304-4095)
/// Server response:                  |<------->| (full remaining bytes)
/// Consume from response:            |<--->|     (1 KiB read)
/// Bytes read so far:    |<--------------->|     (3 KiB read by caller)
///
/// read() call #4:                         |<--->| (1 KiB buffer)
/// Consume from response:                  |<->| (768 bytes read, EOF)
/// Bytes read so far:    |<------------------->| (full 3840 bytes read)
/// ```
///
/// In this example, after the server refuses to send the full subfile in the
/// first request, HttpSubFile transparently handles performing the additional
/// requests as needed to satisfy the calls to `read()`. It also handles
/// re-establishing the connection and continuing the read after a network error
/// occurs, without the caller even needing to be aware of it. Note that the
/// `read()` call never fails in this example, even though multiple requests
/// were needed and a network error occurred during the process.
pub struct HttpSubFile {
    /// The URL of the HTTP resource.
    url: Url,
    /// An optional value of the AUTHORIZATION header to use for requests. If
    /// None, no authorization header is sent.
    authorization: Option<String>,
    /// The HTTP client used to make requests.
    client: Client,
    /// The start byte of the subfile (inclusive).
    start: u64,
    /// The end byte of the subfile (inclusive).
    end: u64,
    /// The current position within the subfile.
    position: u64,
    /// The current response reader, if any.
    reader: Option<PartialResponseReader>,
    /// Request timeout duration. This is the total time allowed including
    /// retries for every specific request that is made. If a request cannot be
    /// completed within this time, the read will fail. If multiple requests are
    /// needed to satisfy a read, each request will have this timeout applied
    /// separately. Default is 30 seconds.
    timeout: Duration,

    /// Whether the end of the subfile is also the end of the parent file. Used
    /// for specific optimizations to avoid sending a Range header when not
    /// needed.
    end_is_parent_eof: bool,
}

impl HttpSubFile {
    /// Creates a new HttpSubFile that reads the byte range [start, end] from the
    /// given URL. The range is inclusive like the HTTP Range header, and is
    /// expected to have been validated beforehand.
    #[allow(dead_code)] // Used in tests
    pub fn new(url: Url, start: u64, end: u64) -> Self {
        Self::new_with_client(url, start, end, Client::new())
    }

    /// Same as `new`, but allows specifying a custom HTTP client.
    pub(super) fn new_with_client(url: Url, start: u64, end: u64, client: Client) -> Self {
        HttpSubFile {
            url,
            start,
            end,
            client,
            position: 0,
            reader: None,
            authorization: None,
            timeout: Duration::from_secs(30),
            end_is_parent_eof: false,
        }
    }

    /// Sets the authorization header value to use for requests.
    pub(super) fn with_authorization(mut self, authorization: impl Into<String>) -> Self {
        self.authorization = Some(authorization.into());
        self
    }

    /// Sets the timeout duration for each HTTP request.
    pub(super) fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Directs the subfile to optimize for the case where the end of the subfile
    /// is also the end of the parent file. This allows avoiding sending a Range
    /// header when reading to the end of the file.
    pub(super) fn with_end_is_parent_eof(mut self) -> Self {
        self.end_is_parent_eof = true;
        self
    }

    /// Returns the length of the subfile in bytes.
    pub fn size(&self) -> u64 {
        // Add 1 because the range is inclusive.
        self.end - self.start + 1
    }

    /// Returns whether we have reached the end of the subfile.
    fn is_eof(&self) -> bool {
        self.position >= self.size()
    }

    fn populate_reader(&mut self) -> IoResult<&mut PartialResponseReader> {
        if self.reader.as_ref().is_none_or(|r| r.exhausted()) {
            // Create a new partial response reader for the next range and
            // replace any existing reader.
            let mut previous_response_was_empty = false;
            let new_reader = loop {
                // If we have already tried and got a zero-length response,
                // make the new request silently to avoid log spam.
                let reader = self.new_partial_response_reader(previous_response_was_empty)?;
                if !reader.exhausted() {
                    trace!(
                        "Received a response for subfile '{}' at position {} with {} bytes",
                        self.url,
                        self.position,
                        reader.bytes_remaining,
                    );

                    break reader;
                }

                if !previous_response_was_empty {
                    previous_response_was_empty = true;
                    trace!(
                        "Received empty response when populating reader for subfile '{}' at position {}, retrying silently...",
                        self.url,
                        self.position,
                    );
                }

                // If we received an empty response, we retry after a short delay.
                thread::sleep(Duration::from_millis(50));
            };

            self.reader = Some(new_reader);
        } else {
            #[cfg(test)]
            trace!(
                "Reusing existing PartialResponseReader at position {} with remaining bytes {} for subfile: {}",
                self.position,
                self.reader.as_ref().map(|r| r.bytes_remaining).unwrap_or(0),
                self.url,
            );
        }

        // Safe to unwrap because we just populated it if it was None.
        Ok(self.reader.as_mut().unwrap())
    }

    /// Creates a new PartialResponseReader for the current position within
    /// the subfile. When `silent` is true, the "Requesting HTTP range" trace
    /// log is suppressed — used during retries after an empty response to
    /// avoid flooding the logs with repeated identical messages.
    fn new_partial_response_reader(&self, silent: bool) -> IoResult<PartialResponseReader> {
        // Always attempt to read up to the end of the subfile.
        let range = HttpRangeRequest::new(
            Some(self.start + self.position),
            // If the end of the subfile is also the end of the parent file, we
            // can avoid setting the end.
            self.end_is_parent_eof.not().then_some(self.end),
        );

        // Perform the HTTP request with retries. This function guarantees that
        // the resulting response was successful.
        if !silent {
            trace!(
                "Requesting HTTP range '{}' for subfile at position {}: {}",
                range.to_header_value(),
                self.position,
                self.url,
            );
        }

        let response = super::retriable_request_sender(
            || {
                let mut req = self.client.get(self.url.clone()).timeout(self.timeout);

                if let Some(range) = range.to_header_value_option() {
                    req = req.header(RANGE, range);
                }

                if let Some(auth) = &self.authorization {
                    req = req.header(AUTHORIZATION, auth);
                }

                req.send()
            },
            self.timeout,
        )?;

        let resp = PartialResponseReader::new_from_response(response)?;

        #[cfg(test)]
        trace!(
            "Server responded with {} bytes for range '{}' of size {}",
            resp.bytes_remaining,
            range.to_header_value(),
            range
                .size()
                .map(|size| size.to_string())
                .unwrap_or_else(|| "undetermined".to_string()),
        );

        Ok(resp)
    }
}

impl Read for HttpSubFile {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        // If we've reached the end of the subfile, return 0 bytes read.
        if self.is_eof() {
            return Ok(0);
        }

        // Location within the buffer to write to.
        let mut buf_position = 0;

        loop {
            // Ensure we have a reader populated.
            let reader = self.populate_reader()?;

            // Attempt to read from the current reader.
            let bytes_read = match reader.read(&mut buf[buf_position..]) {
                Ok(n) => n,
                Err(e) => {
                    warn!(
                        "Error reading from HTTP subfile at position {}: {e}",
                        self.position,
                    );

                    // We failed to execute the last read, possibly because of a
                    // network error. Discard the current reader and try again.
                    // This should allow us to resume reading from the current
                    // position and save the caller from having to restart the
                    // read entirely if the connection can be re-established.
                    //
                    // Callers of read will *generally* use buffers of at most
                    // some MiBs, so re-requesting the current range should not
                    // be too expensive, and will provide better resiliency when
                    // downloading a subfile that could be hundreds of MiBs or
                    // more.
                    self.reader = None;
                    continue;
                }
            };

            // On success, update our position in the file and the buffer position.
            self.position += bytes_read as u64;
            buf_position += bytes_read;

            // Return conditions:
            // - We've reached the end of the subfile, in which case we have
            //   nothing more to read and we can just return.
            // - We've satisfied the read request, in which case we can return
            //   the number of bytes read.
            if self.is_eof() || buf_position == buf.len() {
                // Reached the end of the subfile.
                #[cfg(test)]
                trace!(
                    "Subfile read request of {} bytes satisfied ({} bytes read, position {}, EOF: {})",
                    buf.len(),
                    buf_position,
                    self.position,
                    self.is_eof(),
                );

                return Ok(buf_position);
            }

            // Otherwise, we need to continue reading.
            #[cfg(test)]
            trace!(
                "Subfile read request of {} bytes partially satisfied ({} bytes read so far, position {}), continuing...",
                buf.len(),
                buf_position,
                self.position,
            )
        }
    }
}

struct PartialResponseReader {
    inner: Response,
    bytes_remaining: u64,
}

impl PartialResponseReader {
    /// Creates a new PartialResponseReader from the given HTTP response.
    /// Assumes that the response contains a Content-Length header indicating
    /// the total number of bytes in the response.
    fn new_from_response(response: Response) -> IoResult<Self> {
        let content_length = super::get_content_length(&response)?;

        Ok(PartialResponseReader {
            inner: response,
            bytes_remaining: content_length,
        })
    }

    /// Returns whether all bytes have been read.
    fn exhausted(&self) -> bool {
        self.bytes_remaining == 0
    }
}

impl Read for PartialResponseReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.exhausted() {
            #[cfg(test)]
            trace!("PartialResponseReader exhausted, returning 0 bytes read");

            return Ok(0);
        }

        // Determine the maximum number of bytes we can read.
        let max_bytes_to_read = std::cmp::min(buf.len() as u64, self.bytes_remaining) as usize;

        #[cfg(test)]
        trace!(
            "Reading up to {} bytes from PartialResponseReader ({} bytes remaining)",
            max_bytes_to_read,
            self.bytes_remaining,
        );

        // Read from the inner response.
        let bytes_read = self.inner.read(&mut buf[..max_bytes_to_read])?;

        // Update the bytes remaining.
        self.bytes_remaining -= bytes_read as u64;

        #[cfg(test)]
        trace!(
            "Read {} bytes from PartialResponseReader ({} bytes remaining)",
            bytes_read,
            self.bytes_remaining
        );

        Ok(bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        io::{Error as IoError, ErrorKind as IoErrorKind},
        sync::{Arc, Mutex},
        thread,
    };

    use hyper::StatusCode;
    use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE};

    static PARTIAL_CONTENT: usize = StatusCode::PARTIAL_CONTENT.as_u16() as usize;
    static OK: usize = StatusCode::OK.as_u16() as usize;

    #[test]
    fn test_subfile_single_request_full_read() {
        let mut server = mockito::Server::new();
        let full_body = "abcdefghij";
        let start = 2_u64;
        let end = 6_u64;
        let sub_body = &full_body[start as usize..=end as usize];

        let relative_path = "/subfile.bin";
        let expected_range = format!("bytes={}-{}", start, end);
        let expected_content_range = format!("bytes {}-{}/{}", start, end, full_body.len());
        let expected_content_length = sub_body.len().to_string();

        let mock = server
            .mock("GET", relative_path)
            .match_header("range", expected_range.as_str())
            .with_status(PARTIAL_CONTENT)
            .with_header(CONTENT_LENGTH, expected_content_length.as_str())
            .with_header(CONTENT_RANGE, expected_content_range.as_str())
            .with_body(sub_body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join(relative_path).unwrap();

        let mut subfile = HttpSubFile::new(request_url, start, end);
        let mut buf = vec![0_u8; subfile.size() as usize];
        let bytes_read = subfile.read(&mut buf).unwrap();

        assert_eq!(bytes_read, sub_body.len());
        assert_eq!(buf, sub_body.as_bytes());

        mock.assert();
    }

    #[test]
    fn test_subfile_single_request_multiple_small_reads() {
        let mut server = mockito::Server::new();
        let full_body = "abcdefghijklmnopqrstuvwxyz0123456789";
        let start = 5_u64;
        let end = 30_u64;
        let sub_body = &full_body[start as usize..=end as usize];

        let relative_path = "/subfile-large.bin";
        let expected_range = format!("bytes={}-{}", start, end);
        let expected_content_range = format!("bytes {}-{}/{}", start, end, full_body.len());
        let expected_content_length = sub_body.len().to_string();

        let mock = server
            .mock("GET", relative_path)
            .match_header("range", expected_range.as_str())
            .with_status(PARTIAL_CONTENT)
            .with_header(CONTENT_LENGTH, expected_content_length.as_str())
            .with_header(CONTENT_RANGE, expected_content_range.as_str())
            .with_body(sub_body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join(relative_path).unwrap();

        let mut subfile = HttpSubFile::new(request_url, start, end);
        let mut collected = Vec::new();
        let mut buf = vec![0_u8; 4];

        loop {
            let bytes_read = subfile.read(&mut buf).unwrap();
            if bytes_read == 0 {
                break;
            }
            collected.extend_from_slice(&buf[..bytes_read]);
        }

        assert_eq!(collected, sub_body.as_bytes());
        mock.assert();
    }

    #[test]
    fn test_subfile_multiple_requests_single_read() {
        let _ = env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut server = mockito::Server::new();
        let full_body = "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
        let start = 10_u64;
        let end = 49_u64;
        let sub_body = &full_body[start as usize..=end as usize];

        let relative_path = "/subfile-chunked.bin";

        let chunk_size = 10_usize;
        let mut mocks = Vec::new();
        for chunk_index in 0..4_u64 {
            let chunk_start = (chunk_index as usize) * chunk_size;
            let chunk_end = chunk_start + chunk_size;
            let chunk = &sub_body[chunk_start..chunk_end];

            let range_start = start + (chunk_index * chunk_size as u64);
            let expected_range = format!("bytes={}-{}", range_start, end);
            let chunk_content_length = chunk.len().to_string();

            let mock = server
                .mock("GET", relative_path)
                .match_header("range", expected_range.as_str())
                .with_status(PARTIAL_CONTENT)
                .with_header(CONTENT_LENGTH, chunk_content_length.as_str())
                .with_body(chunk)
                .expect(1)
                .create();
            mocks.push(mock);
        }

        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join(relative_path).unwrap();

        let mut subfile = HttpSubFile::new(request_url, start, end);
        let mut buf = vec![0_u8; subfile.size() as usize];
        let bytes_read = subfile.read(&mut buf).unwrap();

        assert_eq!(bytes_read, sub_body.len());
        assert_eq!(buf, sub_body.as_bytes());

        for mock in mocks {
            mock.assert();
        }
    }

    #[test]
    fn test_subfile_three_chunks_two_reads() {
        let _ = env_logger::builder()
            .filter(Some("request"), log::LevelFilter::Info)
            .filter(Some("hyper_util"), log::LevelFilter::Info)
            .filter(Some("trident"), log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut server = mockito::Server::new();
        let full_body = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
        let start = 10_u64;
        let end = start + 59;
        let sub_body = &full_body[start as usize..=end as usize];

        let relative_path = "/subfile-three-chunks.bin";

        let chunk_size = 20_usize;
        let mut mocks = Vec::new();
        for chunk_index in 0..3_u64 {
            let chunk_start = (chunk_index as usize) * chunk_size;
            let chunk_end = chunk_start + chunk_size;
            let chunk = &sub_body[chunk_start..chunk_end];

            let range_start = start + (chunk_index * chunk_size as u64);
            let expected_range = format!("bytes={}-{}", range_start, end);
            let chunk_content_length = chunk.len().to_string();

            let mock = server
                .mock("GET", relative_path)
                .match_header("range", expected_range.as_str())
                .with_status(PARTIAL_CONTENT)
                .with_header(CONTENT_LENGTH, chunk_content_length.as_str())
                .with_body(chunk)
                .expect(1)
                .create();
            mocks.push(mock);
        }

        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join(relative_path).unwrap();

        let mut subfile = HttpSubFile::new(request_url, start, end);
        let mut collected = Vec::new();

        let mut first_buf = vec![0_u8; 30];
        let first_read = subfile.read(&mut first_buf).unwrap();
        collected.extend_from_slice(&first_buf[..first_read]);

        let mut second_buf = vec![0_u8; 40];
        let second_read = subfile.read(&mut second_buf).unwrap();
        collected.extend_from_slice(&second_buf[..second_read]);

        assert_eq!(collected, sub_body.as_bytes());

        for mock in mocks {
            mock.assert();
        }
    }

    #[test]
    fn test_interrupted_download_resumes() {
        let _ = env_logger::builder()
            .filter(Some("request"), log::LevelFilter::Info)
            .filter(Some("hyper_util"), log::LevelFilter::Info)
            .filter(Some("trident"), log::LevelFilter::Trace)
            .filter(Some("mockito"), log::LevelFilter::Trace)
            .is_test(true)
            .try_init();

        let mut server = mockito::Server::new();
        let full_body =
            "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789abcdefghijklmnopqrstuvwxyz";
        let start = 10_u64;
        let end = 69_u64;
        let sub_body = &full_body[start as usize..=end as usize];

        let relative_path = "/subfile-interrupted.bin";
        let interrupt = 20;

        let mut mocks = Vec::new();

        // To make this test deterministic, we use a mutex to control when the
        // simulated network error occurs. The reading code will hold the lock
        // until after the first read, ensuring we read the initial bytes before
        // the error is triggered. The server waits to acquire the lock before
        // simulating the error, ensuring the client has read the initial bytes
        // first.
        let err_lock = Arc::new(Mutex::new(()));
        let err_lock_clone = err_lock.clone();
        let mut err_guard = Some(err_lock.lock().unwrap());

        let full_range = format!("bytes={}-{}", start, end);
        let interrupted_payload: Vec<u8> = sub_body.as_bytes()[..interrupt as usize].to_vec();
        let mock_interrupted = server
            .mock("GET", relative_path)
            .match_header("range", full_range.as_str())
            .with_status(PARTIAL_CONTENT)
            .with_header(CONTENT_LENGTH, sub_body.len().to_string().as_str())
            .with_chunked_body(move |writer| {
                // Write part of the payload, then simulate a network error.
                writer.write_all(&interrupted_payload)?;
                // Sleep a bit to ensure the bytes are flushed and client has
                // time to read the data.
                trace!(
                    "Simulating network interruption after sending {} bytes",
                    interrupted_payload.len()
                );
                let _guard = err_lock_clone.lock().unwrap();
                thread::sleep(Duration::from_millis(250));
                trace!("Simulated network interruption occurring now");
                Err(IoError::new(
                    IoErrorKind::ConnectionReset,
                    "simulated network drop",
                ))
            })
            .expect(1)
            .create();
        mocks.push(mock_interrupted);

        let retry_range = format!("bytes={}-{}", start + interrupt, end);
        let retry_payload = &sub_body.as_bytes()[interrupt as usize..];
        let mock_retry = server
            .mock("GET", relative_path)
            .match_header("range", retry_range.as_str())
            .with_status(PARTIAL_CONTENT)
            .with_header(CONTENT_LENGTH, retry_payload.len().to_string().as_str())
            .with_body(retry_payload)
            .expect(1)
            .create();
        mocks.push(mock_retry);

        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join(relative_path).unwrap();

        let mut subfile = HttpSubFile::new(request_url, start, end);
        let mut collected = Vec::new();
        let mut buf = vec![0_u8; 8];

        loop {
            let bytes_read = subfile.read(&mut buf).unwrap();
            if bytes_read == 0 {
                break;
            }
            collected.extend_from_slice(&buf[..bytes_read]);

            if let Some(guard) = err_guard.take() {
                trace!("Releasing error lock to allow interrupted mock to proceed");
                drop(guard);
            }
        }

        assert_eq!(collected, sub_body.as_bytes());

        for mock in mocks {
            mock.assert();
        }
    }

    #[test]
    fn test_with_authorization_header() {
        // Simple 1-request test to ensure we get the authorization header as expected.
        let authorization = "Bearer testtoken123";

        let mut server = mockito::Server::new();
        let full_body = "abcdefghij";
        let mock = server
            .mock("GET", "/auth-subfile.bin")
            .match_header(AUTHORIZATION, authorization)
            .with_status(OK)
            .with_body(full_body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join("/auth-subfile.bin").unwrap();

        let mut subfile = HttpSubFile::new(request_url, 0, (full_body.len() - 1) as u64)
            .with_authorization(authorization)
            .with_end_is_parent_eof();
        let mut buf = vec![0_u8; subfile.size() as usize];
        let bytes_read = subfile.read(&mut buf).unwrap();
        assert_eq!(bytes_read, full_body.len());
        assert_eq!(buf, full_body.as_bytes());

        mock.assert();
    }

    #[test]
    fn test_exhaust_partial_response_reader() {
        let mut server = mockito::Server::new();
        let body = "1234567890";
        let content_length = body.len().to_string();

        let mock = server
            .mock("GET", "/partial-reader.bin")
            .with_status(OK)
            .with_header(CONTENT_LENGTH, content_length.as_str())
            .with_body(body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join("/partial-reader.bin").unwrap();

        let client = Client::new();
        let response = client.get(request_url).send().unwrap();
        let mut reader = PartialResponseReader::new_from_response(response).unwrap();

        let mut buf = vec![0_u8; 4];
        let bytes_read1 = reader.read(&mut buf).unwrap();
        assert_eq!(bytes_read1, 4);
        assert_eq!(&buf[..bytes_read1], b"1234");
        assert_eq!(reader.bytes_remaining, 6);

        let bytes_read2 = reader.read(&mut buf).unwrap();
        assert_eq!(bytes_read2, 4);
        assert_eq!(&buf[..bytes_read2], b"5678");
        assert_eq!(reader.bytes_remaining, 2);

        let bytes_read3 = reader.read(&mut buf).unwrap();
        assert_eq!(bytes_read3, 2);
        assert_eq!(&buf[..bytes_read3], b"90");
        assert_eq!(reader.bytes_remaining, 0);

        let bytes_read4 = reader.read(&mut buf).unwrap();
        assert_eq!(bytes_read4, 0); // EOF

        mock.assert();
    }

    #[test]
    fn test_timeout_is_respected() {
        let mut subfile = HttpSubFile::new(
            Url::parse("http://localhost:45555/does/not/exist").unwrap(),
            0,
            16,
        )
        .with_timeout(Duration::from_millis(200));

        let mut buf = vec![0_u8; 16];
        let result = subfile.read(&mut buf);

        let err = result.unwrap_err();
        assert_eq!(err.kind(), IoErrorKind::TimedOut);
    }
}
