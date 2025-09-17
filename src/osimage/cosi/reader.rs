#[cfg(test)]
use std::io::Cursor;
use std::{
    fs::File,
    io::{Error as IoError, Read, Result as IoResult, Seek, SeekFrom},
    path::PathBuf,
    time::Duration,
};

use anyhow::{bail, ensure, Error};
use log::debug;
use url::Url;

use crate::io_utils::universal_reader::HttpFile;

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
/// inexpensively cloned. CosiReader is identical to UniversalReader except
/// that CosiReader also supports reading specific sections of a COSI file.
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
    pub(super) fn new(source: &Url, timeout: Duration) -> Result<Self, Error> {
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
                Self::Http(HttpFile::new(source, timeout)?)
            }
            "oci" => {
                // Load COSI from container registry
                debug!("Loading COSI file from URL: '{}'", source);
                Self::Http(HttpFile::new_from_oci(source, timeout)?)
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    use tempfile::NamedTempFile;

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
        let reader_factory = CosiReader::new(&url, Duration::from_secs(5)).unwrap();

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
}
