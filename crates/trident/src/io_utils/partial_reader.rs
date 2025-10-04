use std::{
    io::{Read, Result as IoResult, Take},
    time::Duration,
};

use log::trace;
use reqwest::{
    blocking::{Client, Response},
    header,
};
use url::Url;

use crate::io_utils::{http_file::HttpFile, http_range::HttpRange};

/// A reader that handles partial HTTP responses and automatically retries
/// to fetch remaining bytes when the server delivers only part of the requested range.
#[allow(dead_code)]
pub struct PartialReader {
    client: Client,
    url: Url,
    token: Option<String>,
    requested_start: u64,
    requested_end: Option<u64>,
    current_position: u64,
    current_response: Option<Take<Response>>,
    timeout: Duration,
    eof_reached: bool,
}

impl PartialReader {
    /// Creates a new PartialReader for the given range.
    pub fn new(
        client: Client,
        url: Url,
        token: Option<String>,
        start: Option<u64>,
        end: Option<u64>,
        timeout: Duration,
    ) -> IoResult<Self> {
        let requested_start = start.unwrap_or(0);
        Ok(Self {
            client,
            url,
            token,
            requested_start,
            requested_end: end,
            current_position: requested_start,
            current_response: None,
            timeout,
            eof_reached: false,
        })
    }

    /// Fetches the next chunk of data, handling partial responses.
    fn fetch_next_chunk(&mut self) -> IoResult<()> {
        let request_sender = || {
            let mut request = self.client.get(self.url.as_str());

            if let Some(token) = &self.token {
                request = request.header(header::AUTHORIZATION, format!("Bearer {token}"));
            }

            // Generate the range header based on current position
            // Only add Range header if we're not reading from the beginning with no end specified
            let use_range = match (self.current_position, self.requested_end) {
                (0, None) => false, // Reading whole file from start
                (_, Some(end)) if self.current_position > end => {
                    return Ok(None); // Already read everything
                }
                _ => true, // Use range header
            };

            if use_range {
                let range_header = match self.requested_end {
                    Some(end) => format!("bytes={}-{}", self.current_position, end),
                    None => format!("bytes={}-", self.current_position),
                };
                request = request.header(header::RANGE, range_header);
            }

            request.send().map(Some)
        };

        // Try to fetch the next chunk, but if it fails with an error, treat it as EOF
        // This handles cases where we request past the end of the file
        let response = match HttpFile::retriable_request_sender(request_sender, self.timeout) {
            Ok(resp) => resp,
            Err(_) => {
                // If we encounter an error (e.g., 416 Range Not Satisfiable), treat as EOF
                self.eof_reached = true;
                return Ok(());
            }
        };

        if let Some(response) = response {
            // Check if we got a Content-Range header to see what was actually delivered
            let actual_range =
                if let Some(content_range) = response.headers().get(header::CONTENT_RANGE) {
                    if let Ok(range_str) = content_range.to_str() {
                        if let Ok(range) = HttpRange::parse(range_str) {
                            trace!(
                                "Server returned content range: {:?}, requested from {} to {:?}",
                                range,
                                self.current_position,
                                self.requested_end
                            );
                            Some(range)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                };

            // Determine how many bytes to read from this response
            let bytes_to_read = if let Some(range) = actual_range {
                range.size().unwrap_or(0)
            } else {
                // No Content-Range header, read whatever content-length says
                if let Some(content_length) = response.headers().get(header::CONTENT_LENGTH) {
                    content_length
                        .to_str()
                        .ok()
                        .and_then(|s| s.parse::<u64>().ok())
                        .unwrap_or(u64::MAX)
                } else {
                    u64::MAX
                }
            };

            // If bytes_to_read is 0, we've reached EOF
            if bytes_to_read == 0 {
                self.eof_reached = true;
            } else {
                self.current_response = Some(response.take(bytes_to_read));
            }
            Ok(())
        } else {
            // No more data to read
            self.eof_reached = true;
            Ok(())
        }
    }

    /// Returns true if we've read all requested bytes or reached EOF.
    fn is_complete(&self) -> bool {
        if self.eof_reached {
            return true;
        }
        if let Some(end) = self.requested_end {
            self.current_position > end
        } else {
            false
        }
    }
}

impl Read for PartialReader {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        // Check if we're already done
        if self.is_complete() {
            return Ok(0);
        }

        loop {
            // If we have an active response, try to read from it
            if let Some(mut response) = self.current_response.take() {
                match response.read(buf) {
                    Ok(0) => {
                        // This chunk is exhausted, don't put it back
                        // Mark that we need to check if there's more data

                        // Check if we're done with the requested range
                        if self.is_complete() {
                            return Ok(0);
                        }

                        // For open-ended reads (no requested_end), check if we've truly reached EOF
                        // by seeing if we can fetch another chunk
                        self.fetch_next_chunk()?;

                        // If we reached EOF while fetching or fetching returned no response, we're done
                        if self.eof_reached || self.current_response.is_none() {
                            return Ok(0);
                        }
                        // Loop back to try reading from the new response
                    }
                    Ok(n) => {
                        self.current_position += n as u64;
                        // Put the response back
                        self.current_response = Some(response);
                        return Ok(n);
                    }
                    Err(e) => {
                        // Put the response back even on error
                        self.current_response = Some(response);
                        return Err(e);
                    }
                }
            } else {
                // No active response, fetch one
                if self.is_complete() {
                    return Ok(0);
                }
                self.fetch_next_chunk()?;

                // If we reached EOF while fetching or fetching returned no response, we're done
                if self.eof_reached || self.current_response.is_none() {
                    return Ok(0);
                }
                // Loop back to try reading from the new response
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn test_partial_reader_full_file_no_range_header() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let body = "Hello, World!";
        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // Mock GET request without Range header (full file read from start)
        let mock = server
            .mock("GET", file_name)
            .match_header("Range", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("Content-Length", &body.len().to_string())
            .with_body(body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        // Read entire file from position 0 with no end specified
        let mut reader =
            PartialReader::new(client, url, None, None, None, Duration::from_secs(5)).unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, body.len());
        assert_eq!(buffer, body);
        mock.assert();
    }

    #[test]
    fn test_partial_reader_with_range() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // Mock GET request with Range header
        let mock = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=5-10")
            .with_status(206)
            .with_header("Content-Range", "bytes 5-10/16")
            .with_header("Content-Length", "6")
            .with_body("56789A")
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        // Read bytes 5-10 (inclusive)
        let mut reader =
            PartialReader::new(client, url, None, Some(5), Some(10), Duration::from_secs(5))
                .unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, 6);
        assert_eq!(buffer, "56789A");
        mock.assert();
    }

    #[test]
    fn test_partial_reader_handles_partial_response() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let _body = "0123456789ABCDEF";
        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // First request: server returns only first 5 bytes instead of requested 10
        let mock1 = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=0-9")
            .with_status(206)
            .with_header("Content-Range", "bytes 0-4/16")
            .with_header("Content-Length", "5")
            .with_body("01234")
            .expect(1)
            .create();

        // Second request: server returns the rest
        let mock2 = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=5-9")
            .with_status(206)
            .with_header("Content-Range", "bytes 5-9/16")
            .with_header("Content-Length", "5")
            .with_body("56789")
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        // Request bytes 0-9 (10 bytes total)
        let mut reader =
            PartialReader::new(client, url, None, Some(0), Some(9), Duration::from_secs(5))
                .unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, 10);
        assert_eq!(buffer, "0123456789");
        mock1.assert();
        mock2.assert();
    }

    #[test]
    fn test_partial_reader_multiple_chunks() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let _body = "ABCDEFGHIJKLMNOP";
        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // Server returns data in 4-byte chunks
        let mock1 = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=0-11")
            .with_status(206)
            .with_header("Content-Range", "bytes 0-3/16")
            .with_header("Content-Length", "4")
            .with_body("ABCD")
            .expect(1)
            .create();

        let mock2 = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=4-11")
            .with_status(206)
            .with_header("Content-Range", "bytes 4-7/16")
            .with_header("Content-Length", "4")
            .with_body("EFGH")
            .expect(1)
            .create();

        let mock3 = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=8-11")
            .with_status(206)
            .with_header("Content-Range", "bytes 8-11/16")
            .with_header("Content-Length", "4")
            .with_body("IJKL")
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        // Request bytes 0-11 (12 bytes total)
        let mut reader =
            PartialReader::new(client, url, None, Some(0), Some(11), Duration::from_secs(5))
                .unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, 12);
        assert_eq!(buffer, "ABCDEFGHIJKL");
        mock1.assert();
        mock2.assert();
        mock3.assert();
    }

    #[test]
    fn test_partial_reader_from_offset() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // Mock GET request starting from offset 10
        let mock = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=10-")
            .with_status(206)
            .with_header("Content-Range", "bytes 10-15/16")
            .with_header("Content-Length", "6")
            .with_body("ABCDEF")
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        // Read from position 10 to end (no end specified)
        let mut reader =
            PartialReader::new(client, url, None, Some(10), None, Duration::from_secs(5)).unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, 6);
        assert_eq!(buffer, "ABCDEF");
        mock.assert();
    }

    #[test]
    fn test_partial_reader_small_buffer_reads() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let body = "HELLO WORLD!";
        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // Mock GET request
        let mock = server
            .mock("GET", file_name)
            .match_header("Range", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("Content-Length", &body.len().to_string())
            .with_body(body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        let mut reader =
            PartialReader::new(client, url, None, None, None, Duration::from_secs(5)).unwrap();

        // Read in small chunks
        let mut result = String::new();
        let mut buf = [0u8; 3];

        loop {
            let n = reader.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            result.push_str(&String::from_utf8_lossy(&buf[..n]));
        }

        assert_eq!(result, body);
        mock.assert();
    }

    #[test]
    fn test_partial_reader_with_auth_token() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let body = "Authenticated content";
        let mut server = mockito::Server::new();
        let file_name = "/secure.txt";
        let token = "test-bearer-token";

        // Mock GET request with authorization header
        let mock = server
            .mock("GET", file_name)
            .match_header("Authorization", format!("Bearer {}", token).as_str())
            .match_header("Range", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("Content-Length", &body.len().to_string())
            .with_body(body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        let mut reader = PartialReader::new(
            client,
            url,
            Some(token.to_string()),
            None,
            None,
            Duration::from_secs(5),
        )
        .unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, body.len());
        assert_eq!(buffer, body);
        mock.assert();
    }

    #[test]
    fn test_partial_reader_empty_response() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let mut server = mockito::Server::new();
        let file_name = "/empty.txt";

        // Mock GET request returning empty content
        let mock = server
            .mock("GET", file_name)
            .match_header("Range", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("Content-Length", "0")
            .with_body("")
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        let mut reader =
            PartialReader::new(client, url, None, None, None, Duration::from_secs(5)).unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, 0);
        assert_eq!(buffer, "");
        mock.assert();
    }

    #[test]
    fn test_partial_reader_read_exact() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let body = "EXACTDATA";
        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        let mock = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=0-8")
            .with_status(206)
            .with_header("Content-Range", "bytes 0-8/9")
            .with_header("Content-Length", "9")
            .with_body(body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        let mut reader =
            PartialReader::new(client, url, None, Some(0), Some(8), Duration::from_secs(5))
                .unwrap();

        let mut buffer = [0u8; 9];
        reader.read_exact(&mut buffer).unwrap();

        assert_eq!(&buffer, body.as_bytes());
        mock.assert();
    }

    #[test]
    fn test_partial_reader_single_byte() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // Read a single byte
        let mock = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=5-5")
            .with_status(206)
            .with_header("Content-Range", "bytes 5-5/10")
            .with_header("Content-Length", "1")
            .with_body("X")
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        let mut reader =
            PartialReader::new(client, url, None, Some(5), Some(5), Duration::from_secs(5))
                .unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, 1);
        assert_eq!(buffer, "X");
        mock.assert();
    }

    #[test]
    fn test_partial_reader_zero_length_range() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let server = mockito::Server::new();
        let file_name = "/test.txt";

        // Request where start > end (invalid range) - should result in no bytes read
        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        let mut reader = PartialReader::new(
            client,
            url,
            None,
            Some(10),
            Some(5), // end < start
            Duration::from_secs(5),
        )
        .unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, 0);
        assert_eq!(buffer, "");
    }

    #[test]
    fn test_partial_reader_content_range_without_size() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // Server returns Content-Range without total size (using *)
        let mock = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=0-9")
            .with_status(206)
            .with_header("Content-Range", "bytes 0-9/*")
            .with_body("0123456789")
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        let mut reader =
            PartialReader::new(client, url, None, Some(0), Some(9), Duration::from_secs(5))
                .unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, 10);
        assert_eq!(buffer, "0123456789");
        mock.assert();
    }

    #[test]
    fn test_partial_reader_no_content_length_header() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let body = "NoContentLength";
        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // Server doesn't provide Content-Length or Content-Range
        let mock = server
            .mock("GET", file_name)
            .match_header("Range", mockito::Matcher::Missing)
            .with_status(200)
            .with_body(body)
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        let mut reader =
            PartialReader::new(client, url, None, None, None, Duration::from_secs(5)).unwrap();

        let mut buffer = String::new();
        let bytes_read = reader.read_to_string(&mut buffer).unwrap();

        assert_eq!(bytes_read, body.len());
        assert_eq!(buffer, body);
        mock.assert();
    }

    #[test]
    fn test_partial_reader_retry_after_partial_with_multiple_small_reads() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let mut server = mockito::Server::new();
        let file_name = "/test.txt";

        // First chunk: 4 bytes instead of requested 10
        let mock1 = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=0-9")
            .with_status(206)
            .with_header("Content-Range", "bytes 0-3/10")
            .with_header("Content-Length", "4")
            .with_body("ABCD")
            .expect(1)
            .create();

        // Second chunk: remaining 6 bytes
        let mock2 = server
            .mock("GET", file_name)
            .match_header("Range", "bytes=4-9")
            .with_status(206)
            .with_header("Content-Range", "bytes 4-9/10")
            .with_header("Content-Length", "6")
            .with_body("EFGHIJ")
            .expect(1)
            .create();

        let url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();
        let client = Client::new();

        let mut reader =
            PartialReader::new(client, url, None, Some(0), Some(9), Duration::from_secs(5))
                .unwrap();

        // Read in very small chunks to test incremental reading
        let mut result = Vec::new();
        let mut buf = [0u8; 2]; // Only 2 bytes at a time

        loop {
            let n = reader.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            result.extend_from_slice(&buf[..n]);
        }

        assert_eq!(result.len(), 10);
        assert_eq!(String::from_utf8(result).unwrap(), "ABCDEFGHIJ");
        mock1.assert();
        mock2.assert();
    }
}
