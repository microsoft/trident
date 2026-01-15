use std::{
    io::{Read, Result as IoResult},
    time::Duration,
};

use log::trace;
use reqwest::{
    blocking::{Client, Response},
    header::RANGE,
};
use url::Url;

use super::HttpRangeRequest;

/// Object that represents a subfile located entirely and contiguously within a
/// single HTTP resource. It implements `Read` to read only the specified byte
/// range from the resource. It uses HTTP Range requests to fetch only the
/// needed data, and can handle performing multiple requests as needed when
/// reading in case the server cannot provide the full subfile at once.
pub struct HttpSubFile {
    /// The URL of the HTTP resource.
    url: Url,
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
}

impl HttpSubFile {
    // Creates a new HttpSubFile that reads the byte range [start, end] from the
    // given URL. The range is inclusive like the HTTP Range header, and is
    // expected to have been validated beforehand.
    pub fn new(url: Url, start: u64, end: u64) -> Self {
        HttpSubFile {
            url,
            start,
            end,
            client: Client::new(),
            position: 0,
            reader: None,
        }
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
        }
    }

    /// Returns the length of the subfile in bytes.
    pub fn len(&self) -> u64 {
        // Add 1 because the range is inclusive.
        self.end - self.start + 1
    }

    /// Returns whether we have reached the end of the subfile.
    fn is_eof(&self) -> bool {
        self.position >= self.len()
    }

    fn populate_reader(&mut self) -> IoResult<&mut PartialResponseReader> {
        if self.reader.as_ref().is_none_or(|r| r.exhausted()) {
            // Create a new partial response reader for the next range and
            // replace any existing reader.
            self.reader = Some(self.new_partial_response_reader()?);
        }

        // Safe to unwrap because we just populated it if it was None.
        Ok(self.reader.as_mut().unwrap())
    }

    /// Creates a new PartialResponseReader for the current position within
    /// the subfile.
    fn new_partial_response_reader(&self) -> IoResult<PartialResponseReader> {
        // Always attempt to read up to the end of the subfile.
        let range =
            HttpRangeRequest::new_bounded(self.start + self.position, self.end).to_header_value();

        // Perform the HTTP request with retries. This function guarantees that
        // the resulting response was successful.
        let response = super::retriable_request_sender(
            || {
                self.client
                    .get(self.url.clone())
                    .header(RANGE, &range)
                    .send()
            },
            Duration::from_secs(30),
        )?;

        PartialResponseReader::new_from_response(response)
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
                    trace!(
                        "Error reading from HTTP subfile at position {}: {e}",
                        self.position,
                    );

                    continue;
                }
            };
            self.position += bytes_read as u64;
            buf_position += bytes_read;

            // Return conditions:
            // - We've reached the end of the subfile, in which case we have
            //   nothing more to read and we can just return.
            // - We've satisfied the read request, in which case we can return
            //   the number of bytes read.
            if self.is_eof() || buf_position == buf.len() {
                // Reached the end of the subfile.
                trace!(
                    "Subfile read request of {} bytes satisfied ({} bytes read, position {}, EOF: {})",
                    buf.len(),
                    buf_position,
                    self.position,
                    self.is_eof(),
                );

                return Ok(buf_position);
            }

            // Otherwise, we need to continue reading from the next range.
            trace!(
                "Subfile read request of {} bytes partially satisfied ({} bytes read so far, position {}), continuing with next HTTP range request",
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
            return Ok(0);
        }

        // Determine the maximum number of bytes we can read.
        let max_bytes_to_read = std::cmp::min(buf.len() as u64, self.bytes_remaining) as usize;

        // Read from the inner response.
        let bytes_read = self.inner.read(&mut buf[..max_bytes_to_read])?;

        // Update the bytes remaining.
        self.bytes_remaining -= bytes_read as u64;

        Ok(bytes_read)
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

    use reqwest::header::{CONTENT_LENGTH, CONTENT_RANGE};
    use url::Url;

    use super::HttpSubFile;

    #[test]
    fn test_subfile_single_request_full_read() {
        let mut server = mockito::Server::new();
        let full_body = "abcdefghij";
        let start = 2_u64;
        let end = 6_u64;
        let sub_body = &full_body[start as usize..=end as usize];

        let relative_path = "/subfile.bin";
        let expected_range = format!("bytes={}-{}", start, end - 1);
        let expected_content_range = format!("bytes {}-{}/{}", start, end, full_body.len());
        let expected_content_length = sub_body.len().to_string();

        let mock = server
            .mock("GET", relative_path)
            .match_header("range", expected_range.as_str())
            .with_status(206)
            .with_header(CONTENT_LENGTH, expected_content_length.as_str())
            .with_header(CONTENT_RANGE, expected_content_range.as_str())
            .with_body(sub_body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join(relative_path).unwrap();

        let mut subfile = HttpSubFile::new(request_url, start, end);
        let mut buf = vec![0_u8; subfile.len() as usize];
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
        let expected_range = format!("bytes={}-{}", start, end - 1);
        let expected_content_range = format!("bytes {}-{}/{}", start, end, full_body.len());
        let expected_content_length = sub_body.len().to_string();

        let mock = server
            .mock("GET", relative_path)
            .match_header("range", expected_range.as_str())
            .with_status(206)
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
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .init();

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
            let expected_range = format!("bytes={}-{}", range_start, end - 1);
            let chunk_content_length = chunk.len().to_string();

            let mock = server
                .mock("GET", relative_path)
                .match_header("range", expected_range.as_str())
                .with_status(206)
                .with_header(CONTENT_LENGTH, chunk_content_length.as_str())
                .with_body(chunk)
                .expect(1)
                .create();
            mocks.push(mock);
        }

        let url = Url::parse(&server.url()).unwrap();
        let request_url = url.join(relative_path).unwrap();

        let mut subfile = HttpSubFile::new(request_url, start, end);
        let mut buf = vec![0_u8; subfile.len() as usize];
        let bytes_read = subfile.read(&mut buf).unwrap();

        assert_eq!(bytes_read, sub_body.len());
        assert_eq!(buf, sub_body.as_bytes());

        for mock in mocks {
            mock.assert();
        }
    }

    #[test]
    fn test_subfile_three_chunks_two_reads() {
        env_logger::builder()
            .filter(Some("request"), log::LevelFilter::Info)
            .filter(Some("hyper_util"), log::LevelFilter::Info)
            .filter(Some("trident"), log::LevelFilter::Trace)
            // .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .init();

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
            let expected_range = format!("bytes={}-{}", range_start, end - 1);
            let chunk_content_length = chunk.len().to_string();

            let mock = server
                .mock("GET", relative_path)
                .match_header("range", expected_range.as_str())
                .with_status(206)
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
}
