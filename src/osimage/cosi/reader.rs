use std::{
    fs::File,
    io::{Error as IoError, ErrorKind as IoErrorKind, Read, Result as IoResult, Seek, SeekFrom},
    path::PathBuf,
};

use anyhow::{bail, ensure, Error};
use log::{debug, trace};
use reqwest::blocking::{Client, Response};
use url::Url;

pub(super) trait ReadSeek: Read + Seek {}

impl ReadSeek for HttpFile {}
impl ReadSeek for File {}

/// An abstraction over a COSI file reader that can be either a local file or an
/// HTTP request.
#[derive(Debug, Clone)]
pub(super) enum CosiReader {
    File(PathBuf),
    Http(HttpFile),
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
}

impl HttpFile {
    /// Creates a new HTTP file reader from the given URL.
    pub fn new(url: &Url) -> IoResult<Self> {
        debug!("Opening HTTP file '{}'", url);

        // Create a new client for this file.
        let client = Client::new();

        // Query the server for the file size
        let response = client
            .head(url.as_str())
            .send()
            .map_err(Self::http_to_io_err)?;

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
                    format!("Could not parse 'Content-Length': {}", e),
                )
            })?
            .parse()
            .map_err(|e| {
                IoError::new(
                    IoErrorKind::InvalidData,
                    format!("Could not parse 'Content-Length' as an integer: {}", e),
                )
            })?;

        // Ensure the server supports range requests, this implementation
        // requires that feature!
        if response
            .headers()
            .get("Accept-Ranges")
            .ok_or(IoError::new(
                IoErrorKind::Other,
                "Server does not support range requests",
            ))?
            .to_str()
            .map_err(|e| {
                IoError::new(
                    IoErrorKind::InvalidData,
                    format!("Could not parse 'Accept-Ranges': {}", e),
                )
            })?
            .to_lowercase()
            .eq("none")
        {
            return Err(IoError::new(
                IoErrorKind::Other,
                "Server does not support range requests",
            ));
        }

        debug!("Successfully queried HTTP file '{}' of size: {}", url, size);
        Ok(Self {
            url: url.clone(),
            position: 0,
            size,
            client,
        })
    }

    /// Converts an HTTP error into an IO error.
    fn http_to_io_err(e: reqwest::Error) -> IoError {
        let formatted = format!("HTTP File error: {}", e);
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
        let mut request = self.client.get(self.url.as_str());

        // Generate the range header when appropriate
        let range_header = match (start, end) {
            (Some(start), Some(end)) => Some(format!("bytes={}-{}", start, end)),
            (Some(start), None) => Some(format!("bytes={}-", start)),
            (None, Some(end)) => Some(format!("bytes=0-{}", end)),
            (None, None) => None,
        };

        // Add the range header to the request
        if let Some(range) = range_header {
            request = request.header("Range", range);
        }

        request
            .send()
            .map_err(Self::http_to_io_err)?
            .error_for_status()
            .map_err(Self::http_to_io_err)
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
    use tempfile::NamedTempFile;

    use super::*;

    use std::io::{SeekFrom, Write};

    #[test]
    fn test_http_file_seek() {
        let mut http_file = HttpFile {
            url: Url::parse("http://example.com").unwrap(),
            position: 0,
            size: 100, // We have indices from 0 to 99
            client: Client::new(),
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
            "Did not read expected data, expected '{}' but got '{}'",
            original_data, buf
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
                "Did not read expected number of bytes, expected {} but got {}. Buffer '{}'",
                size, read, buf
            );
            assert_eq!(
                expected_slice, &buf,
                "Did not read expected slice, expected '{}' but got '{}'",
                expected_slice, buf
            );
        };
        check_section(0, 5); // Hello
        check_section(7, 6); // World!
        check_section(3, 6); // llo, W
        check_section(0, 13); // Hello, World!
    }
}
