use std::{
    io::{Error as IoError, ErrorKind as IoErrorKind, Read, Result as IoResult, Seek},
    time::Duration,
};

#[cfg(feature = "dangerous-options")]
use std::{env, io::BufReader};

use anyhow::{ensure, Context, Error};
use log::{debug, trace, warn};
use oci_client::{secrets::RegistryAuth, Client as OciClient, Reference};
use reqwest::{
    blocking::Client,
    header::{ACCEPT_RANGES, AUTHORIZATION},
};
use tokio::runtime::Runtime;
use url::Url;

#[cfg(feature = "dangerous-options")]
use docker_credential::{self, DockerCredential};

use crate::io_utils::http::subfile::HttpSubFile;

#[cfg(feature = "dangerous-options")]
const DOCKER_CONFIG_FILE_PATH: &str = ".docker/config.json";

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
    pub(super) size: u64,
    client: Client,
    timeout: Duration,
    token: Option<String>,
}

impl HttpFile {
    /// Creates a new HTTP file reader from a standard HTTP URL.
    pub fn new(url: &Url, timeout: Duration) -> IoResult<Self> {
        Self::new_inner(url, None, false, timeout)
    }

    /// Creates a new HTTP file reader from an OCI URL.
    pub fn new_from_oci(url: &Url, timeout: Duration) -> Result<Self, Error> {
        let img_ref =
            Reference::try_from(url.to_string().strip_prefix("oci://").with_context(|| {
                format!("URL has incorrect scheme: expected to start with 'oci://', got '{url}'")
            })?)
            .with_context(|| format!("Failed to parse URL '{url}'"))?;

        let oci_client = OciClient::default();
        let rt = Runtime::new().context("Failed to create Tokio runtime")?;
        let token = Self::retrieve_access_token(&img_ref, &rt, &oci_client)?;
        let digest = Self::retrieve_artifact_digest(&img_ref, &rt, &oci_client)?;
        trace!("Retrieved artifact digest: {digest}");

        // Create HTTP URL
        let registry = img_ref.registry();
        let repository = img_ref.repository();
        let http_url = Url::parse(&format!(
            "https://{registry}/v2/{repository}/blobs/{digest}"
        ))?;

        Self::new_inner(&http_url, Some(token), true, timeout)
            .context("Failed to create HTTP file reader")
    }

    fn new_inner(
        url: &Url,
        token: Option<String>,
        ignore_ranges_header_absence: bool,
        timeout: Duration,
    ) -> IoResult<Self> {
        debug!("Opening HTTP file '{}'", url);

        // Create a new client for this file.
        let client = Client::new();
        let request_sender = || {
            let mut request = client.head(url.as_str());
            if let Some(token) = &token {
                request = request.header(AUTHORIZATION, format!("Bearer {token}"));
            }
            request.send()
        };
        let response = super::retriable_request_sender(request_sender, timeout)?;
        trace!("HTTP file '{}' has status: {}", url, response.status());

        // Get the file size from the response headers
        let size = super::get_content_length(&response)?;

        trace!("HTTP file '{}' has size: {}", url, size);

        // Ensure the server supports range requests, this implementation
        // requires that feature!
        let accept_ranges_header = response.headers().get(ACCEPT_RANGES);
        if accept_ranges_header.is_none() && ignore_ranges_header_absence {
            warn!("OCI server does not provide '{ACCEPT_RANGES}' header, continuing anyway");
        } else if accept_ranges_header
            .ok_or_else(|| IoError::other(
                format!("Server does not support range requests: '{ACCEPT_RANGES}' header was not provided"),
            ))?
            .to_str()
            .map_err(|e| {
                IoError::new(
                    IoErrorKind::InvalidData,
                    format!("Could not parse '{ACCEPT_RANGES}': {e}"),
                )
            })?
            .to_lowercase()
            .eq("none")
        {
            return Err(IoError::other(
                format!("Server does not support range requests: '{ACCEPT_RANGES}: none'"),
            ));
        }

        debug!("Successfully queried HTTP file '{}' of size: {}", url, size);
        Ok(Self {
            url: url.clone(),
            position: 0,
            size,
            client,
            timeout,
            token,
        })
    }

    /// Retrieve bearer token to access container registry. Even registries allowing anonymous
    /// access may require a token.
    fn retrieve_access_token(
        img_ref: &Reference,
        runtime: &Runtime,
        client: &OciClient,
    ) -> Result<String, Error> {
        trace!(
            "Retrieving access token for OCI registry '{}'",
            img_ref.registry()
        );
        let auth = Self::get_auth(img_ref);
        runtime
            .block_on(client.auth(img_ref, &auth, oci_client::RegistryOperation::Pull))
            .with_context(|| {
                format!(
                    "Registry '{}' is not accessible or does not exist",
                    img_ref.registry()
                )
            })?
            .context("Failed to retrieve authorization token")
    }

    /// Get authentication credentials for accessing registry. Unless "dangerous-options" flag is
    /// enabled, will default to anonymous access.
    fn get_auth(_img_ref: &Reference) -> RegistryAuth {
        #[cfg(feature = "dangerous-options")]
        'config_auth: {
            let Some(user_home) = env::home_dir() else {
                debug!("Could not determine user home directory, using anonymous access.");
                break 'config_auth;
            };

            let docker_config_path = user_home.join(DOCKER_CONFIG_FILE_PATH);
            if !docker_config_path.exists() {
                debug!(
                    "Docker config file does not exist at '{}'",
                    docker_config_path.display()
                );
                break 'config_auth;
            }

            let docker_config = match std::fs::File::open(docker_config_path) {
                Ok(file) => file,
                Err(e) => {
                    debug!("Failed to open docker config file: {}", e);
                    break 'config_auth;
                }
            };

            let registry = _img_ref
                .resolve_registry()
                .strip_suffix('/')
                .unwrap_or_else(|| _img_ref.resolve_registry());

            match docker_credential::get_credential_from_reader(
                BufReader::new(docker_config),
                registry,
            ) {
                Ok(DockerCredential::UsernamePassword(username, password)) => {
                    debug!("Using username and password docker credential");
                    return RegistryAuth::Basic(username, password);
                }
                Ok(DockerCredential::IdentityToken(_)) => {
                    debug!("Found identity token docker credential, ignoring")
                }
                Err(e) => debug!("Failed to retrieve docker credentials: {e}"),
            }
        }

        debug!("Proceeding with anonymous access");
        RegistryAuth::Anonymous
    }

    /// Retrieve artifact digest, which is necessary to send HTTP request to container registry.
    fn retrieve_artifact_digest(
        img_ref: &Reference,
        runtime: &Runtime,
        client: &OciClient,
    ) -> Result<String, Error> {
        trace!("Retrieving artifact digest");
        Ok(match img_ref.digest() {
            Some(digest) => digest.to_string(),
            None => {
                let tag = img_ref.tag().with_context(|| {
                    format!("Failed to retrieve tag from OCI URL '{}'", img_ref.whole())
                })?;
                // Attempt to retrieve digest from manifest
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

    /// Performs a request of a specific section of the file. Returns an
    /// HTTPSubFile object.
    pub(crate) fn section_reader(&self, section_offset: u64, size: u64) -> HttpSubFile {
        let end = section_offset + size - 1;
        trace!(
            "Reading HTTP file '{}' from {} to {} (inclusive) [{} bytes]",
            self.url,
            section_offset,
            end,
            size
        );

        let mut subfile = HttpSubFile::new_with_client(
            self.url.clone(),
            section_offset,
            end,
            self.client.clone(),
        )
        .with_timeout(self.timeout);

        if let Some(token) = self.token.as_ref() {
            subfile = subfile.with_authorization(format!("Bearer {token}"));
        }

        subfile
    }

    /// Performs a request to read the complete file. Returns the HTTP response.
    pub(crate) fn complete_reader(&self) -> HttpSubFile {
        trace!("Reading complete HTTP file '{}'", self.url);
        self.section_reader(0, self.size)
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
        let mut subfile = self.section_reader(self.position, buf.len() as u64);
        let res = subfile.read(buf)?;
        self.position += res as u64;
        Ok(res)
    }

    fn read_exact(&mut self, buf: &mut [u8]) -> IoResult<()> {
        let mut subfile = self.section_reader(self.position, buf.len() as u64);
        subfile.read_exact(buf)?;
        self.position += buf.len() as u64;
        Ok(())
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> IoResult<usize> {
        let mut subfile = self.section_reader(self.position, buf.len() as u64);
        let res = subfile.read_to_end(buf)?;
        self.position += res as u64;
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::SeekFrom;

    #[test]
    fn test_retrieve_access_token() {
        let client = OciClient::default();
        let rt = Runtime::new().unwrap();
        let url = "oci://docker.io/library/hello-world:latest".to_string();
        let img_ref = url
            .strip_prefix("oci://")
            .and_then(|url| url.parse::<Reference>().ok())
            .unwrap();
        HttpFile::retrieve_access_token(&img_ref, &rt, &client).unwrap();
    }

    #[test]
    fn test_retrieve_artifact_digest() {
        let client = OciClient::default();
        let rt = Runtime::new().unwrap();
        // TODO(12732): Fix this test to use test COSI file instead of hello-world image
        let url = "oci://docker.io/library/hello-world@sha256:940c619fbd418f9b2b1b63e25d8861f9cc1b46e3fc8b018ccfe8b78f19b8cc4f".to_string();
        let img_ref = url
            .strip_prefix("oci://")
            .and_then(|url| url.parse::<Reference>().ok())
            .unwrap();
        assert_eq!(
            HttpFile::retrieve_artifact_digest(&img_ref, &rt, &client).unwrap(),
            "sha256:940c619fbd418f9b2b1b63e25d8861f9cc1b46e3fc8b018ccfe8b78f19b8cc4f"
        );
    }

    #[test]
    fn test_http_file_seek() {
        let mut http_file = HttpFile {
            url: Url::parse("http://example.com").unwrap(),
            position: 0,
            size: 100, // We have indices from 0 to 99
            client: Client::new(),
            timeout: Duration::from_secs(1),
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
}
