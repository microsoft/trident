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

use crate::io_utils::http_file::HttpFile;

pub(crate) trait ReadSeek: Read + Seek {}

impl ReadSeek for HttpFile {}
impl ReadSeek for File {}

#[cfg(test)]
impl ReadSeek for Cursor<Vec<u8>> {}

/// An abstraction over a file reader that can be either a local file or an
/// HTTP request.
///
/// This abstraction contains the minimum required information to open a file,
/// it does not carry any complex types and can be safely and inexpensively
/// cloned.
#[derive(Debug, Clone)]
pub(crate) enum FileReader {
    File(PathBuf),
    Http(HttpFile),

    /// Variant reserved for testing purposes only.
    #[cfg(test)]
    Buffer(Cursor<Vec<u8>>),
}

impl FileReader {
    /// Creates a new file reader from the given source URL.
    pub(crate) fn new(source: &Url, timeout: Duration) -> Result<Self, Error> {
        Ok(match source.scheme() {
            "file" => {
                // Load from local file
                debug!("Loading file: '{}'", source.path());
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
                // Load from remote URL
                debug!("Loading file from URL: '{}'", source);
                Self::Http(HttpFile::new(source, timeout)?)
            }
            "oci" => {
                // Load from container registry
                debug!("Loading file from URL: '{}'", source);
                Self::Http(HttpFile::new_from_oci(source, timeout)?)
            }
            _ => {
                bail!("Unsupported URL scheme: {}", source.scheme());
            }
        })
    }

    /// Returns an implementation of `Read` + `Seek` over the entire file.
    pub(crate) fn reader(&self) -> Result<Box<dyn ReadSeek>, IoError> {
        Ok(match self {
            Self::File(file) => Box::new(File::open(file)?),
            Self::Http(http_file) => Box::new(http_file.clone()),
            #[cfg(test)]
            Self::Buffer(cursor) => Box::new(cursor.clone()),
        })
    }

    /// Returns an implementation of `Read` for the given section of the file.
    pub(crate) fn section_reader(&self, section_offset: u64, size: u64) -> IoResult<Box<dyn Read>> {
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
            Self::Buffer(cursor) => {
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

    use log::trace;
    use tempfile::NamedTempFile;

    #[test]
    fn test_file_reader_factory_file() {
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
        let reader_factory = FileReader::new(&url, Duration::from_secs(5)).unwrap();

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
            .expect_at_least(9)
            .create();

        let file_url = Url::parse(&server.url()).unwrap().join(file_name).unwrap();

        // Create a new HTTP file reader.
        let file_reader = FileReader::new(&file_url, Duration::from_secs(5)).unwrap();

        // Get a reference to the inner HTTP file reader
        let FileReader::Http(ref http_file) = file_reader else {
            panic!("Expected a HTTP file reader, got {file_reader:?}");
        };

        // Clone the file to test that the server is only called once.
        let _ = http_file.clone();
        // This function also just clones the http_file.
        let _ = file_reader.reader().unwrap();

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
