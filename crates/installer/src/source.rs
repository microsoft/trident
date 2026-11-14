use std::{
    fs::{self, File},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail, ensure, Context, Error};
use log::warn;
use regex::{Captures, Regex};
use reqwest::blocking::Client;
use serde::{de::IgnoredAny, Deserialize};
use sha2::{Digest, Sha384};
use tar::Archive;
use url::Url;

use osutils::{
    filesystems::MountFileSystemType,
    findmnt::FindMnt,
    lsblk::{self, BlockDevice, BlockDeviceType},
    mount, mountpoint,
};
use trident_api::{
    config::HostConfiguration, constants::MOUNT_OPTION_READ_ONLY, primitives::hash::Sha384Hash,
};

use crate::config::{self, Config};

pub(super) const IMAGE_MARKER: &str = "installer://image";
const SOURCE_TIMEOUT: Duration = Duration::from_secs(30);
const REMOTE_RETRY_TIMEOUT: Duration = Duration::from_secs(60);
const REMOTE_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const MEDIA_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(60);
const MEDIA_POLL_INTERVAL: Duration = Duration::from_secs(2);
const MAX_HOST_CONFIGURATION_BYTES: u64 = 2 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const METADATA_PATH: &str = "metadata.json";
const STREAM_DISK_PREFIXES: &[&str] = &["sd", "nvme", "vd", "hd", "mmcblk"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Request {
    Autorun,
    HostConfiguration(String),
    Stream(Url),
}

#[derive(Debug, Clone)]
pub(super) enum Plan {
    Install {
        config: Box<HostConfiguration>,
        source: String,
    },
    Stream {
        image: Url,
    },
}

impl Plan {
    /// Describes the selected source for the progress window.
    pub(super) fn description(&self) -> String {
        match self {
            Self::Install { source, .. } => format!("Install from {}", display_source(source)),
            Self::Stream { image } => format!("StreamDisk from {}", display_url(image)),
        }
    }
}

/// Removes URL credentials and signed query data from diagnostic source names.
pub(super) fn display_url(source: &Url) -> String {
    let mut url = source.clone();
    if (url.password().is_some() && url.set_password(None).is_err())
        || (!url.username().is_empty() && url.set_username("").is_err())
    {
        return "<redacted URL>".into();
    }
    url.set_query(None);
    url.set_fragment(None);
    url.to_string()
}

/// Preserves local path descriptions without exposing URL secrets.
pub(super) fn display_source(source: &str) -> String {
    match Url::parse(source) {
        Ok(url) => display_url(&url),
        Err(_) if source.contains("://") => "<invalid URL>".into(),
        Err(_) => source.to_owned(),
    }
}

/// Removes credentials and signed query data from URLs embedded in diagnostics.
pub(super) fn redact_message_urls(message: &str) -> String {
    static URLS: OnceLock<Regex> = OnceLock::new();
    let urls = URLS.get_or_init(|| {
        Regex::new(r#"(?i)https?://[^\s"<>]+"#).expect("invariant: diagnostic URL pattern is valid")
    });
    urls.replace_all(message, |capture: &Captures<'_>| {
        let matched = capture
            .get(0)
            .expect("invariant: entire URL match")
            .as_str();
        let url = matched.trim_end_matches(['.', ',', ')', ']', ';', '\'']);
        let suffix = &matched[url.len()..];
        let description = Url::parse(url)
            .map(|url| display_url(&url))
            .unwrap_or_else(|_| "<redacted URL>".into());
        format!("{description}{suffix}")
    })
    .into_owned()
}

/// Resolves installation media and returns an install or stream plan with progress callbacks.
///
/// Only autorun waits for media enumeration; explicit recovery sources proceed immediately.
pub(super) fn prepare_with_progress(
    settings: &Config,
    request: &Request,
    mut progress: impl FnMut(&str) -> Result<(), Error>,
) -> Result<(Config, Plan), Error> {
    if matches!(request, Request::Stream(_)) {
        return prepare_from_media_with_progress(settings, request, None, &mut progress);
    }
    if request == &Request::Autorun {
        settings.require_autorun()?;
        if let Some(plan) = self_contained_install(settings, &mut progress)? {
            return Ok((settings.clone(), plan));
        }
    }
    let root = media_root(settings, request == &Request::Autorun, &mut progress)?;
    prepare_from_media_with_progress(settings, request, root.as_deref(), &mut progress)
}

/// Treats a complete live-OS or remote Host Configuration as authoritative, without media overlays.
fn self_contained_install(
    settings: &Config,
    progress: &mut impl FnMut(&str) -> Result<(), Error>,
) -> Result<Option<Plan>, Error> {
    let Some(source) = configured_host_configuration_source(settings)? else {
        return Ok(None);
    };
    if !source.starts_with('/') && !source.contains("://") {
        return Ok(None);
    }
    let url = source_url(&source, None)?;
    if file_source_may_need_media(&url, settings)? {
        return Ok(None);
    }
    progress("Checking Host Configuration")?;
    let text = read_text(&url)?;
    let config: HostConfiguration =
        serde_yaml::from_str(&text).context("Invalid Host Configuration YAML")?;
    if let Some(image) = &config.image {
        if image.url.as_str() == IMAGE_MARKER || file_source_may_need_media(&image.url, settings)? {
            return Ok(None);
        }
    }
    let config = resolve_parsed_host_configuration(config, None)?;
    Ok(Some(Plan::Install {
        config: Box::new(config),
        source,
    }))
}

/// A missing path behind a symlink may become readable only after installer media is mounted.
fn file_source_may_need_media(url: &Url, settings: &Config) -> Result<bool, Error> {
    if url.scheme() != "file" {
        return Ok(false);
    }
    let path = url
        .to_file_path()
        .map_err(|()| anyhow!("Invalid local file URL"))?;
    let media = settings
        .media
        .mount_path
        .as_deref()
        .unwrap_or(Path::new(config::MEDIA_MOUNT));
    if path.starts_with(media) {
        return Ok(true);
    }
    match path.canonicalize() {
        Ok(resolved) => Ok(resolved.starts_with(media)),
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let mut ancestor = PathBuf::new();
            for component in path.components() {
                ancestor.push(component);
                match fs::symlink_metadata(&ancestor) {
                    Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
                    Ok(_) => {}
                    Err(error) if error.kind() == ErrorKind::NotFound => break,
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("Failed to inspect '{}'", ancestor.display()))
                    }
                }
            }
            Ok(false)
        }
        Err(error) => Err(error).with_context(|| format!("Failed to resolve '{}'", path.display())),
    }
}

/// Keeps dangling default Host Configuration symlinks eligible for media-dependent installs.
fn configured_host_configuration_source(settings: &Config) -> Result<Option<String>, Error> {
    if let Some(source) = &settings.autorun.host_configuration {
        return Ok(Some(source.clone()));
    }
    match fs::symlink_metadata(config::DEFAULT_HOST_CONFIGURATION) {
        Ok(_) => Ok(Some(config::DEFAULT_HOST_CONFIGURATION.into())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Failed to inspect default Host Configuration"),
    }
}

#[cfg(test)]
/// Prepares a plan from an already-mounted media root for tests.
fn prepare_from_media(
    settings: &Config,
    request: &Request,
    root: Option<&Path>,
) -> Result<(Config, Plan), Error> {
    prepare_from_media_with_progress(settings, request, root, &mut |_| Ok(()))
}

/// Merges media settings and selects a Host Configuration before falling back to the first COSI.
fn prepare_from_media_with_progress(
    settings: &Config,
    request: &Request,
    root: Option<&Path>,
    progress: &mut impl FnMut(&str) -> Result<(), Error>,
) -> Result<(Config, Plan), Error> {
    if let Request::Stream(image) = request {
        settings.require_autorun()?;
        return Ok((
            settings.clone(),
            Plan::Stream {
                image: image.clone(),
            },
        ));
    }
    progress("Reading installer configuration")?;
    let settings = match root {
        Some(root) => settings.overlay(root)?,
        None => settings.clone(),
    };
    settings.require_autorun()?;
    progress("Scanning media for COSI images")?;
    let images = match root {
        Some(root) => config::discover_images(root, &settings.media.cosi_directory)?,
        None => Vec::new(),
    };
    progress("Checking Host Configuration")?;
    let source = if let Request::HostConfiguration(source) = request {
        remote_url(source)?;
        Some(source.clone())
    } else {
        configured_host_configuration_source(&settings)?
    };
    let plan = match source {
        Some(source) => {
            let url = source_url(&source, root)?;
            let text = read_text(&url)?;
            let config = resolve_host_configuration(&text, images.first().map(PathBuf::as_path))?;
            Plan::Install {
                config: Box::new(config),
                source,
            }
        }
        None => {
            let path = images.first().with_context(|| {
                format!(
                    "No Host Configuration at '{}' and no COSI images in '{}'. Configure networking in Shell, then choose a remote source.",
                    config::DEFAULT_HOST_CONFIGURATION,
                    settings.media.cosi_directory.display()
                )
            })?;
            Plan::Stream {
                image: file_url(path)?,
            }
        }
    };
    Ok((settings, plan))
}

/// Finds labelled media and mounts it read-only, or validates a configured mount path.
///
/// When `wait_for_media` is set, waits up to the discovery timeout for late USB partitions.
/// A pre-existing mount must belong to the selected device and be read-only.
fn media_root(
    settings: &Config,
    wait_for_media: bool,
    progress: &mut impl FnMut(&str) -> Result<(), Error>,
) -> Result<Option<PathBuf>, Error> {
    if let Some(path) = &settings.media.mount_path {
        progress("Checking the configured media mount")?;
        ensure!(
            mountpoint::check_is_mountpoint(path)?,
            "media.mountPath is not a mount point: {}",
            path.display()
        );
        return Ok(Some(path.canonicalize()?));
    }
    let labels = settings
        .media
        .filesystem_label
        .as_deref()
        .map(|label| vec![label])
        .unwrap_or_else(|| config::DEFAULT_MEDIA_LABELS.to_vec());
    progress("Looking for installer media by filesystem label")?;
    let deadline = Instant::now() + MEDIA_DISCOVERY_TIMEOUT;
    let mut attempts = 0;
    let devices = loop {
        let devices = lsblk::devices_by_labels(&labels)?;
        if !devices.is_empty() || !wait_for_media || Instant::now() >= deadline {
            break devices;
        }
        if attempts % 5 == 0 {
            progress(&format!(
                "Waiting for installation media labelled {}",
                labels.join(" or ")
            ))?;
        }
        attempts += 1;
        thread::sleep(MEDIA_POLL_INTERVAL);
    };
    let Some(device) = config::choose_media(&devices)? else {
        return Ok(None);
    };
    let target = Path::new(config::MEDIA_MOUNT);
    fs::create_dir_all(target)?;
    if mountpoint::check_is_mountpoint(target)? {
        let mounts = FindMnt::run()?
            .root()
            .context("findmnt did not report a root mount")?;
        let existing = mounts
            .find_mount_point_for_path(target)
            .context("Mounted installer media is not visible in findmnt")?;
        ensure!(
            existing.target == target
                && existing.options.contains(MOUNT_OPTION_READ_ONLY)
                && existing.source.as_ref().map(fs::canonicalize).transpose()?
                    == Some(device.canonicalize()?),
            "Existing installer mount does not match the requested read-only media"
        );
    } else {
        progress("Mounting installer media read-only")?;
        mount::mount(
            device,
            target,
            MountFileSystemType::Auto,
            &[
                MOUNT_OPTION_READ_ONLY.into(),
                "nosuid".into(),
                "nodev".into(),
                "noexec".into(),
            ],
        )?;
    }
    Ok(Some(target.into()))
}

/// Accepts only credential-free HTTP(S) URLs for manually entered recovery sources.
pub(super) fn remote_url(source: &str) -> Result<Url, Error> {
    let url = Url::parse(source).context("Enter an absolute http:// or https:// URL")?;
    ensure!(
        matches!(url.scheme(), "http" | "https"),
        "Remote source must use HTTP or HTTPS"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "Credentials in source URLs are not supported"
    );
    Ok(url)
}

/// Resolves local, remote, or media-relative Host Configuration sources.
///
/// Media-relative paths must resolve beneath the media root.
fn source_url(source: &str, root: Option<&Path>) -> Result<Url, Error> {
    if source.starts_with('/') {
        return file_url(Path::new(source));
    }
    if source.contains("://") {
        let url = Url::parse(source)?;
        ensure!(
            matches!(url.scheme(), "file" | "http" | "https"),
            "Unsupported source URL scheme"
        );
        return Ok(url);
    }
    let root = root.context("A media-relative Host Configuration requires installer media")?;
    let path = root
        .join(source)
        .canonicalize()
        .context("Failed to resolve media-relative Host Configuration")?;
    ensure!(
        path.starts_with(root.canonicalize()?),
        "Host Configuration escapes the media root"
    );
    file_url(&path)
}

/// Converts a local path to a `file://` URL, preserving URL escaping.
fn file_url(path: &Path) -> Result<Url, Error> {
    Url::from_file_path(path)
        .map_err(|()| anyhow!("Cannot convert '{}' to a file URL", path.display()))
}

/// Opens a local file or a time-limited HTTP(S) response for source inspection.
fn reader(url: &Url) -> Result<Box<dyn Read>, Error> {
    match url.scheme() {
        "file" => {
            let path = url
                .to_file_path()
                .map_err(|()| anyhow!("Invalid local file URL"))?;
            Ok(Box::new(File::open(&path).with_context(|| {
                format!("Failed to open '{}'", path.display())
            })?))
        }
        "http" | "https" => {
            let client = Client::builder().build()?;
            let deadline = Instant::now() + REMOTE_RETRY_TIMEOUT;
            let response = loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let response = client
                    .get(url.clone())
                    .timeout(SOURCE_TIMEOUT.min(remaining))
                    .send();
                match response {
                    Ok(response) => {
                        break response.error_for_status().map_err(|error| {
                            anyhow!(
                                "Failed to fetch '{}': {}",
                                display_url(url),
                                error.without_url()
                            )
                        })?
                    }
                    Err(error)
                        if (error.is_connect() || error.is_timeout())
                            && Instant::now() < deadline =>
                    {
                        warn!(
                            "Remote source '{}' unavailable ({}); retrying while network starts",
                            display_url(url),
                            error.without_url()
                        );
                        thread::sleep(
                            REMOTE_RETRY_INTERVAL
                                .min(deadline.saturating_duration_since(Instant::now())),
                        );
                    }
                    Err(error) => {
                        return Err(anyhow!(
                            "Failed to fetch '{}': {}",
                            display_url(url),
                            error.without_url()
                        ));
                    }
                }
            };
            Ok(Box::new(response))
        }
        _ => bail!("Unsupported source scheme '{}'", url.scheme()),
    }
}

/// Reads a Host Configuration as UTF-8 while enforcing the configured byte limit.
fn read_text(url: &Url) -> Result<String, Error> {
    let mut text = String::new();
    reader(url)?
        .take(MAX_HOST_CONFIGURATION_BYTES + 1)
        .read_to_string(&mut text)?;
    ensure!(
        text.len() as u64 <= MAX_HOST_CONFIGURATION_BYTES,
        "Host Configuration exceeds the size limit"
    );
    Ok(text)
}

/// Parses and validates a Host Configuration, resolving `installer://image` to the chosen COSI.
pub(super) fn resolve_host_configuration(
    text: &str,
    selected: Option<&Path>,
) -> Result<HostConfiguration, Error> {
    let config: HostConfiguration =
        serde_yaml::from_str(text).context("Invalid Host Configuration YAML")?;
    resolve_parsed_host_configuration(config, selected)
}

fn resolve_parsed_host_configuration(
    mut config: HostConfiguration,
    selected: Option<&Path>,
) -> Result<HostConfiguration, Error> {
    if let Some(image) = &mut config.image {
        if image.url.as_str() == IMAGE_MARKER {
            image.url = file_url(selected.context(
                "Host Configuration uses installer://image but no COSI was discovered",
            )?)?;
        }
    }
    config
        .validate()
        .context("Host Configuration validation failed")?;
    Ok(config)
}

// Only disk sizing and the metadata hash are needed here; Trident validates the full COSI.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    version: String,
    disk: Option<DiskMetadata>,
    images: Vec<IgnoredAny>,
}

#[derive(Debug, Deserialize)]
struct DiskMetadata {
    size: u64,
}

/// Extracts and hashes the raw COSI metadata within a bounded archive read.
fn metadata(reader: impl Read) -> Result<(Metadata, Sha384Hash), Error> {
    let mut archive = Archive::new(reader.take(MAX_METADATA_BYTES));
    for entry in archive.entries().context("Invalid COSI archive")? {
        let mut entry = entry?;
        let path = entry.path()?;
        if path.strip_prefix("./").unwrap_or(&path) == Path::new(METADATA_PATH) {
            ensure!(
                entry.header().entry_type().is_file(),
                "COSI metadata must be a regular file"
            );
            ensure!(
                entry.size() <= MAX_METADATA_BYTES,
                "COSI metadata is too large"
            );
            let mut raw = Vec::new();
            entry.read_to_end(&mut raw)?;
            let metadata: Metadata =
                serde_json::from_slice(&raw).context("Invalid COSI metadata")?;
            ensure!(
                metadata.disk.as_ref().is_some_and(|disk| disk.size > 0),
                "StreamDisk requires COSI disk metadata"
            );
            ensure!(
                !metadata.images.is_empty(),
                "COSI metadata contains no images"
            );
            let (major, minor) = metadata
                .version
                .split_once('.')
                .context("Invalid COSI metadata version")?;
            ensure!(
                major == "1" && minor.parse::<u32>()? >= 2,
                "StreamDisk requires COSI v1.2 or newer"
            );
            return Ok((
                metadata,
                Sha384Hash::from(format!("{:x}", Sha384::digest(&raw))),
            ));
        }
    }
    bail!("COSI metadata.json was not found within the metadata read limit")
}

/// Checks disk eligibility and returns the metadata hash to bind the daemon's fetch.
pub(super) fn stream_preflight(image: &Url) -> Result<Sha384Hash, Error> {
    let (metadata, hash) = metadata(reader(image)?)?;
    let devices = lsblk::list().context("Failed to inspect attached disks")?;
    checked_stream_hash(&metadata, hash, &devices)
}

/// Keeps target-media safety independent of previously installed filesystem UUIDs.
fn checked_stream_hash(
    metadata: &Metadata,
    hash: Sha384Hash,
    devices: &[BlockDevice],
) -> Result<Sha384Hash, Error> {
    let required = metadata
        .disk
        .as_ref()
        .context("COSI disk metadata is missing")?
        .size;
    ensure_stream_target_safe(devices, required)?;
    Ok(hash)
}

/// Refuses streaming if Trident could choose a mounted or read-only smallest fitting disk.
fn ensure_stream_target_safe(devices: &[BlockDevice], required: u64) -> Result<(), Error> {
    let candidates = devices
        .iter()
        .filter(|device| device.blkdev_type == BlockDeviceType::Disk)
        .filter(|device| {
            STREAM_DISK_PREFIXES
                .iter()
                .any(|prefix| device.name.starts_with(prefix))
        })
        .filter(|device| device.size >= required)
        .collect::<Vec<_>>();
    let minimum = candidates
        .iter()
        .map(|device| device.size)
        .min()
        .context("No disk fits the COSI image")?;
    // Until StreamDisk accepts an explicit target, any minimum-sized candidate may win.
    for device in candidates.iter().filter(|device| device.size == minimum) {
        ensure!(
            !device.readonly && device.get_all_mountpoints_recursive().is_empty(),
            "StreamDisk could select mounted or read-only disk '{}', possibly the installer USB; refusing to overwrite installation media. Use media smaller than the COSI disk size or a Host Configuration with an explicit target disk",
            device.device_path().display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        io::{Cursor, Write},
        net::TcpListener,
        os::unix::fs as unix_fs,
    };

    use indoc::indoc;
    use tar::Builder as TarBuilder;
    use tempfile::TempDir;

    #[test]
    /// Ensures recovery sources cannot silently fall back to local files or other schemes.
    fn remote_sources_are_explicit_and_limited() {
        remote_url("https://example.com/image.cosi").unwrap();
        for invalid in [
            "file:///image.cosi",
            "ftp://example.com/image.cosi",
            "image.cosi",
            "https://user:secret@example.com/image.cosi",
        ] {
            remote_url(invalid).unwrap_err();
        }
    }

    #[test]
    /// Remote sources recover when the network becomes reachable after startup.
    fn remote_source_retries_until_listener_is_ready() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let url = Url::parse(&format!("http://{address}/host.yaml")).unwrap();
        let initial = Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap()
            .get(url.clone())
            .send()
            .unwrap_err();
        assert!(initial.is_connect(), "{initial}");
        let server = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            let listener = TcpListener::bind(address).unwrap();
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 512];
            assert_ne!(socket.read(&mut request).unwrap(), 0);
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 16\r\nConnection: close\r\n\r\nmode = 'autorun'",
                )
                .unwrap();
        });
        assert_eq!(read_text(&url).unwrap(), "mode = 'autorun'");
        server.join().unwrap();
    }

    #[test]
    /// Keeps signed source credentials out of operator-visible descriptions.
    fn source_descriptions_redact_url_secrets() {
        let url =
            Url::parse("https://user:password@example.com/image.cosi?token=secret#key").unwrap();
        assert_eq!(display_url(&url), "https://example.com/image.cosi");
        assert_eq!(
            display_source(url.as_str()),
            "https://example.com/image.cosi"
        );
        assert_eq!(
            Plan::Stream { image: url.clone() }.description(),
            "StreamDisk from https://example.com/image.cosi"
        );
        assert_eq!(
            Plan::Install {
                config: Box::default(),
                source: url.to_string()
            }
            .description(),
            "Install from https://example.com/image.cosi"
        );
        assert_eq!(
            display_source("/etc/trident/config.yaml"),
            "/etc/trident/config.yaml"
        );
        assert_eq!(
            display_url(&Url::parse("file:///media/image.cosi").unwrap()),
            "file:///media/image.cosi"
        );
        assert_eq!(
            display_source("https://bad URL?token=secret"),
            "<invalid URL>"
        );
        assert_eq!(
            redact_message_urls(
                "Loaded 'https://u:p@example.test/image.cosi?sig=sec'ret#fragment', done"
            ),
            "Loaded 'https://example.test/image.cosi', done"
        );
    }

    #[test]
    /// Explicit stream recovery ignores broken media mounts, overlays and COSI directories.
    fn explicit_stream_ignores_local_media() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("installer")).unwrap();
        fs::write(root.path().join(config::MEDIA_CONFIG), "invalid TOML").unwrap();
        let mut settings = Config::default();
        settings.media.mount_path = Some("/nonexistent-installer-media".into());
        let url = Url::parse("https://example.com/image.cosi?token=secret").unwrap();
        let (_, plan) =
            prepare_from_media(&settings, &Request::Stream(url.clone()), Some(root.path()))
                .unwrap();
        assert!(matches!(plan, Plan::Stream { image } if image == url));
        let (_, plan) =
            prepare_with_progress(&settings, &Request::Stream(url.clone()), |_| Ok(())).unwrap();
        assert!(matches!(plan, Plan::Stream { image } if image == url));
    }

    #[test]
    /// A complete live-OS Host Configuration never waits for media or applies its overlay.
    fn self_contained_autorun_skips_media() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("host.yaml");
        fs::write(
            &path,
            valid_host_configuration().replace(IMAGE_MARKER, "https://example.com/image.cosi"),
        )
        .unwrap();
        let mut settings = Config::default();
        settings.autorun.host_configuration = Some(path.display().to_string());
        settings.media.mount_path = Some(root.path().join("not-mounted"));
        let (_, plan) = prepare_with_progress(&settings, &Request::Autorun, |_| Ok(())).unwrap();
        assert!(matches!(plan, Plan::Install { source, .. } if source == path.to_string_lossy()));

        settings.mode = config::Mode::Interactive;
        let error = prepare_with_progress(&settings, &Request::Autorun, |_| Ok(())).unwrap_err();
        assert!(
            error.to_string().contains("Interactive installation"),
            "{error:#}"
        );
    }

    #[test]
    /// An HTTP Host Configuration with its own image does not need a local media mount.
    fn self_contained_remote_autorun_skips_media() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/host.yaml", listener.local_addr().unwrap());
        let server = thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0; 512];
            assert_ne!(socket.read(&mut request).unwrap(), 0);
            let body =
                valid_host_configuration().replace(IMAGE_MARKER, "https://example.com/image.cosi");
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).unwrap();
        });
        let mut settings = Config::default();
        settings.autorun.host_configuration = Some(url.clone());
        settings.media.mount_path = Some("/nonexistent-installer-media".into());
        let (_, plan) = prepare_with_progress(&settings, &Request::Autorun, |_| Ok(())).unwrap();
        assert!(matches!(plan, Plan::Install { source, .. } if source == url));
        server.join().unwrap();
    }

    #[test]
    /// Media-relative Host Configurations and media-backed images still need discovery.
    fn media_dependent_host_configuration_is_not_self_contained() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("host.yaml");
        fs::write(&path, valid_host_configuration()).unwrap();
        let mut settings = Config::default();
        settings.autorun.host_configuration = Some(path.display().to_string());
        assert!(self_contained_install(&settings, &mut |_| Ok(()))
            .unwrap()
            .is_none());
        settings.autorun.host_configuration = Some("installer/host.yaml".into());
        assert!(self_contained_install(&settings, &mut |_| Ok(()))
            .unwrap()
            .is_none());
        fs::write(
            &path,
            valid_host_configuration().replace(
                IMAGE_MARKER,
                &format!("file://{}/cosi/image.cosi", config::MEDIA_MOUNT),
            ),
        )
        .unwrap();
        settings.autorun.host_configuration = Some(path.display().to_string());
        assert!(self_contained_install(&settings, &mut |_| Ok(()))
            .unwrap()
            .is_none());
    }

    #[test]
    /// Dangling links into installer media must not be mistaken for an absent Host Configuration.
    fn symlinked_host_configuration_waits_for_media() {
        let root = TempDir::new().unwrap();
        let link = root.path().join("host.yaml");
        unix_fs::symlink(
            Path::new(config::MEDIA_MOUNT).join("installer/host.yaml"),
            &link,
        )
        .unwrap();
        assert!(fs::metadata(&link).is_err());
        assert!(fs::symlink_metadata(&link).is_ok());
        let mut settings = Config::default();
        settings.autorun.host_configuration = Some(link.display().to_string());
        assert!(self_contained_install(&settings, &mut |_| Ok(()))
            .unwrap()
            .is_none());

        let directory_link = root.path().join("media");
        unix_fs::symlink(config::MEDIA_MOUNT, &directory_link).unwrap();
        settings.autorun.host_configuration = Some(
            directory_link
                .join("installer/host.yaml")
                .display()
                .to_string(),
        );
        assert!(self_contained_install(&settings, &mut |_| Ok(()))
            .unwrap()
            .is_none());

        let local = root.path().join("local.yaml");
        fs::write(
            &local,
            valid_host_configuration().replace(IMAGE_MARKER, "https://example.com/image.cosi"),
        )
        .unwrap();
        let local_link = root.path().join("local-link.yaml");
        unix_fs::symlink(local, &local_link).unwrap();
        settings.autorun.host_configuration = Some(local_link.display().to_string());
        assert!(self_contained_install(&settings, &mut |_| Ok(()))
            .unwrap()
            .is_some());
    }

    #[test]
    /// Ensures the image marker needs a COSI and leaves other image URLs untouched.
    fn marker_requires_an_image_and_does_not_touch_other_urls() {
        let yaml = valid_host_configuration();
        resolve_host_configuration(yaml, None).unwrap_err();
        let resolved =
            resolve_host_configuration(yaml, Some(Path::new("/media/cosi/an image.cosi"))).unwrap();
        assert_eq!(
            resolved.image.unwrap().url.as_str(),
            "file:///media/cosi/an%20image.cosi"
        );
        let remote = yaml.replace(IMAGE_MARKER, "https://example.com/image.cosi");
        assert_eq!(
            resolve_host_configuration(&remote, None)
                .unwrap()
                .image
                .unwrap()
                .url
                .as_str(),
            "https://example.com/image.cosi"
        );
    }

    /// Provides a valid Host Configuration containing the installer image marker.
    fn valid_host_configuration() -> &'static str {
        indoc! {"
            storage:
              disks:
                - id: os
                  device: /dev/test
                  partitionTableType: gpt
                  partitions:
                    - id: esp
                      type: esp
                      size: 192M
                    - id: root
                      type: root
                      size: 1G
              filesystems:
                - deviceId: esp
                  mountPoint:
                    path: /boot/efi
                - deviceId: root
                  mountPoint:
                    path: /
            image:
              url: installer://image
              sha384: ignored
        "}
    }

    #[test]
    /// Ensures Host Configuration takes priority and invalid input never falls back to COSI.
    fn host_configuration_wins_and_invalid_hc_never_falls_back() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("cosi")).unwrap();
        fs::write(root.path().join("cosi/z.cosi"), "").unwrap();
        fs::write(root.path().join("cosi/a.cosi"), "").unwrap();
        let path = root.path().join("host.yaml");
        fs::write(&path, valid_host_configuration()).unwrap();
        let mut settings = Config::default();
        settings.autorun.host_configuration = Some(path.display().to_string());
        let (_, plan) =
            prepare_from_media(&settings, &Request::Autorun, Some(root.path())).unwrap();
        let Plan::Install { config, .. } = plan else {
            panic!("HC must select Install")
        };
        assert_eq!(
            config.image.unwrap().url,
            file_url(&root.path().join("cosi/a.cosi")).unwrap()
        );
        fs::write(&path, "unknown: field").unwrap();
        prepare_from_media(&settings, &Request::Autorun, Some(root.path())).unwrap_err();
    }

    #[test]
    /// Ensures an oversized Host Configuration is rejected before parsing.
    fn configuration_read_limit_is_enforced() {
        let root = TempDir::new().unwrap();
        let path = root.path().join("oversized.yaml");
        fs::write(
            &path,
            vec![b'x'; usize::try_from(MAX_HOST_CONFIGURATION_BYTES + 1).unwrap()],
        )
        .unwrap();
        let error = read_text(&file_url(&path).unwrap()).unwrap_err();
        assert!(error.to_string().contains("size limit"), "{error:#}");
    }

    #[test]
    /// Ensures unreadable and missing COSI metadata cannot pass preflight.
    fn truncated_and_missing_metadata_are_errors() {
        metadata(Cursor::new(b"not a tar file")).unwrap_err();
        let mut builder = TarBuilder::new(Vec::new());
        builder.finish().unwrap();
        metadata(Cursor::new(builder.into_inner().unwrap())).unwrap_err();
    }

    #[test]
    /// Hashes the original metadata bytes rather than re-serialized JSON.
    fn preflight_metadata_hash_matches_daemon_contract() {
        let raw = br#"{"version":"1.2","disk":{"size":4294967296},"images":[{}]}"#;
        let inspect = |raw: &[u8], path: &str| {
            let mut builder = TarBuilder::new(Vec::new());
            let mut header = tar::Header::new_gnu();
            header.set_size(raw.len() as u64);
            header.set_cksum();
            builder.append_data(&mut header, path, raw).unwrap();
            builder.finish().unwrap();
            metadata(Cursor::new(builder.into_inner().unwrap())).unwrap()
        };
        let (parsed, hash) = inspect(raw, METADATA_PATH);
        assert_eq!(parsed.disk.unwrap().size, 4294967296);
        assert_eq!(hash, Sha384Hash::from(format!("{:x}", Sha384::digest(raw))));
        let (_, prefixed_hash) = inspect(raw, "./metadata.json");
        assert_eq!(hash, prefixed_hash);

        let padded = br#"{ "version":"1.2","disk":{"size":4294967296},"images":[{}]}"#;
        let (_, padded_hash) = inspect(padded, METADATA_PATH);
        assert_ne!(hash, padded_hash);
    }

    #[test]
    /// Autorun still streams to a safe disk with matching UUIDs already present.
    fn existing_filesystem_uuids_do_not_block_stream_preflight() {
        let raw = br#"{"version":"1.2","disk":{"size":4294967296},"images":[{"fsUuid":"existing-root"}]}"#;
        let mut builder = TarBuilder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(raw.len() as u64);
        header.set_cksum();
        builder
            .append_data(&mut header, METADATA_PATH, &raw[..])
            .unwrap();
        builder.finish().unwrap();
        let (metadata, hash) = metadata(Cursor::new(builder.into_inner().unwrap())).unwrap();
        let disk = BlockDevice {
            name: "vda".into(),
            size: 8 * 1024 * 1024 * 1024,
            blkdev_type: BlockDeviceType::Disk,
            children: vec![BlockDevice {
                fsuuid: Some("existing-root".into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert_eq!(
            checked_stream_hash(&metadata, hash.clone(), &[disk]).unwrap(),
            hash
        );
    }

    #[test]
    /// Ensures a small installer USB is skipped and an eligible mounted USB is refused.
    fn smaller_gpt_usb_media_is_not_a_stream_target() {
        let media = BlockDevice {
            name: "sdb".into(),
            size: 2 * 1024 * 1024 * 1024,
            blkdev_type: BlockDeviceType::Disk,
            children: vec![BlockDevice {
                name: "sdb1".into(),
                mountpoints: vec![config::MEDIA_MOUNT.into()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let target = BlockDevice {
            name: "vda".into(),
            size: 8 * 1024 * 1024 * 1024,
            blkdev_type: BlockDeviceType::Disk,
            ..Default::default()
        };
        let required = 4 * 1024 * 1024 * 1024;
        ensure_stream_target_safe(&[media.clone(), target.clone()], required).unwrap();
        let larger_media = BlockDevice {
            size: 6 * 1024 * 1024 * 1024,
            ..media
        };
        let error = ensure_stream_target_safe(&[larger_media, target], required).unwrap_err();
        assert!(error.to_string().contains("installer USB"), "{error:#}");
    }
}
