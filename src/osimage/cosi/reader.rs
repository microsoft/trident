use std::{
    fs::File,
    io::{Error as IoError, ErrorKind as IoErrorKind, Read, Result as IoResult, Seek},
};

use anyhow::{bail, Context, Error};
use log::{debug, trace};
use url::Url;

/// An abstraction over a COSI file reader that can be either a local file or an
/// HTTP request.
#[derive(Debug)]
pub(super) enum CosiReader {
    File(File),
    Http(HttpFile),
}

impl CosiReader {
    /// Creates a new COSI file reader from the given source URL.
    pub(super) fn new(source: &Url) -> Result<Self, Error> {
        Ok(match source.scheme() {
            "file" => {
                // Load COSI from local file
                debug!("Loading COSI file: '{}'", source.path());
                CosiReader::File(
                    File::open(source.path())
                        .with_context(|| format!("Failed to open COSI file '{}'", source.path()))?,
                )
            }
            "http" | "https" => {
                // Load COSI from remote URL
                debug!("Loading COSI file from URL: '{}'", source);
                CosiReader::Http(HttpFile::new(source)?)
            }
            _ => {
                bail!("Unsupported URL scheme: {}", source.scheme());
            }
        })
    }
}

impl Read for CosiReader {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        match self {
            Self::File(file) => file.read(buf),
            Self::Http(http_file) => http_file.read(buf),
        }
    }
}

impl Seek for CosiReader {
    fn seek(&mut self, pos: std::io::SeekFrom) -> IoResult<u64> {
        match self {
            Self::File(file) => file.seek(pos),
            Self::Http(http_file) => http_file.seek(pos),
        }
    }
}

impl CosiReader {
    /// Reads data from the COSI file at the given offset and size.
    pub(super) fn read_data(&mut self, offset: u64, size: u64) -> IoResult<Vec<u8>> {
        let mut data = vec![0; size as usize];
        self.seek(std::io::SeekFrom::Start(offset))?;
        self.read_exact(&mut data)?;
        Ok(data)
    }

    /// Reads a range of data from the COSI file.
    pub(super) fn read_range(&mut self, range: (u64, u64)) -> IoResult<Vec<u8>> {
        self.read_data(range.0, range.1)
    }

    /// Returns a section reader for the given range of the COSI file.
    pub(super) fn section_reader(&self, entry: (u64, u64)) -> IoResult<CosiSectionReader> {
        let clone = match self {
            Self::File(file) => Self::File(file.try_clone()?),
            Self::Http(http_file) => Self::Http(http_file.clone()),
        };
        Ok(CosiSectionReader {
            reader: clone,
            position: 0,
            offset: entry.0,
            size: entry.1,
        })
    }
}

/// A section reader for a COSI file that reads a specific range of data.
pub(crate) struct CosiSectionReader {
    reader: CosiReader,
    position: u64,
    offset: u64,
    size: u64,
}

impl Read for CosiSectionReader {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        if self.position >= self.size {
            return Ok(0);
        }

        let read_size = std::cmp::min(buf.len() as u64, self.size - self.position) as usize;
        let data = self
            .reader
            .read_data(self.offset + self.position, read_size as u64)?;
        buf[..read_size].copy_from_slice(&data);
        self.position += read_size as u64;
        Ok(read_size)
    }
}

/// A FILE-like object that is obtained through an HTTP request using range
/// headers instead of a local file.
#[derive(Debug, Clone)]
pub struct HttpFile {
    url: Url,
    position: u64,
    size: u64,
}

impl HttpFile {
    /// Creates a new HTTP file reader from the given URL.
    pub fn new(url: &Url) -> IoResult<Self> {
        debug!("Opening HTTP file '{}'", url);
        let size = reqwest::blocking::Client::new()
            .head(url.as_str())
            .send()
            .map_err(Self::http_to_io_err)?
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
        debug!("Successfully queried HTTP file '{}' of size: {}", url, size);
        Ok(Self {
            url: url.clone(),
            position: 0,
            size,
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
        let size = buf.len();
        let end = self.position + size as u64 - 1;

        trace!(
            "Reading HTTP file '{}' from {} to {} (inclusive) [{} bytes]",
            self.url,
            self.position,
            end,
            size
        );

        let mut response = reqwest::blocking::Client::new()
            .get(self.url.as_str())
            .header("Range", format!("bytes={}-{}", self.position, end))
            .send()
            .map_err(Self::http_to_io_err)?
            .error_for_status()
            .map_err(Self::http_to_io_err)?;

        if let Some(data) = response.headers().get("Content-Range") {
            trace!(
                "Returned content range: {:?}",
                String::from_utf8_lossy(data.as_bytes())
            );
        }

        let res = response.read(buf)?;
        self.position += res as u64;
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::SeekFrom;

    #[test]
    fn test_http_file_seek() {
        let mut http_file = HttpFile {
            url: Url::parse("http://example.com").unwrap(),
            position: 0,
            size: 100, // We have indices from 0 to 99
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
}
