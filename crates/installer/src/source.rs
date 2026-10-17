use std::{
    collections::HashSet,
    fs::{self, File},
    io::{ErrorKind, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{anyhow, bail, ensure, Context, Error};
use reqwest::blocking::Client;
use serde::Deserialize;
use tar::Archive;
use url::Url;

use osutils::{
    filesystems::MountFileSystemType,
    findmnt::FindMnt,
    lsblk::{self, BlockDevice, BlockDeviceType},
    mount, mountpoint,
};
use trident_api::{config::HostConfiguration, constants::MOUNT_OPTION_READ_ONLY};

use crate::config::{self, Config};

pub(super) const IMAGE_MARKER: &str = "installer://image";
const SOURCE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_HOST_CONFIGURATION_BYTES: u64 = 2 * 1024 * 1024;
const MAX_METADATA_BYTES: u64 = 16 * 1024 * 1024;
const METADATA_PATH: &str = "metadata.json";
const STREAM_DISK_PREFIXES: &[&str] = &["sd", "nvme", "vd", "hd", "mmcblk"];

#[derive(Debug, Clone)]
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
    pub(super) fn description(&self) -> String {
        match self {
            Self::Install { source, .. } => format!("Install from {source}"),
            Self::Stream { image } => format!("StreamDisk from {image}"),
        }
    }
}

pub(super) fn prepare_with_progress(
    settings: &Config,
    request: &Request,
    mut progress: impl FnMut(&str) -> Result<(), Error>,
) -> Result<(Config, Plan), Error> {
    let root = media_root(settings, &mut progress)?;
    prepare_from_media_with_progress(settings, request, root.as_deref(), &mut progress)
}

#[cfg(test)]
fn prepare_from_media(
    settings: &Config,
    request: &Request,
    root: Option<&Path>,
) -> Result<(Config, Plan), Error> {
    prepare_from_media_with_progress(settings, request, root, &mut |_| Ok(()))
}

fn prepare_from_media_with_progress(
    settings: &Config,
    request: &Request,
    root: Option<&Path>,
    progress: &mut impl FnMut(&str) -> Result<(), Error>,
) -> Result<(Config, Plan), Error> {
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
    let source = match request {
        Request::HostConfiguration(source) => {
            remote_url(source)?;
            Some(source.clone())
        }
        Request::Stream(source) => {
            return Ok((
                settings,
                Plan::Stream {
                    image: source.clone(),
                },
            ));
        }
        Request::Autorun => match &settings.autorun.host_configuration {
            Some(source) => Some(source.clone()),
            None => match fs::metadata(config::DEFAULT_HOST_CONFIGURATION) {
                Ok(_) => Some(config::DEFAULT_HOST_CONFIGURATION.into()),
                Err(error) if error.kind() == ErrorKind::NotFound => None,
                Err(error) => {
                    return Err(error).context("Failed to inspect default Host Configuration")
                }
            },
        },
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

fn media_root(
    settings: &Config,
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
    let label = settings
        .media
        .cdrom_label
        .as_deref()
        .unwrap_or(config::DEFAULT_MEDIA_LABEL);
    progress("Looking for installer media by filesystem label")?;
    let devices = lsblk::devices_by_label(label)?;
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

fn file_url(path: &Path) -> Result<Url, Error> {
    Url::from_file_path(path)
        .map_err(|()| anyhow!("Cannot convert '{}' to a file URL", path.display()))
}

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
        "http" | "https" => Ok(Box::new(
            Client::builder()
                .timeout(SOURCE_TIMEOUT)
                .build()?
                .get(url.clone())
                .send()?
                .error_for_status()?,
        )),
        _ => bail!("Unsupported source scheme '{}'", url.scheme()),
    }
}

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

pub(super) fn resolve_host_configuration(
    text: &str,
    selected: Option<&Path>,
) -> Result<HostConfiguration, Error> {
    let mut config: HostConfiguration =
        serde_yaml::from_str(text).context("Invalid Host Configuration YAML")?;
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

// Only the repeat-install preflight needs these fields; Trident validates the full COSI.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Metadata {
    version: String,
    disk: Option<DiskMetadata>,
    images: Vec<ImageMetadata>,
}

#[derive(Debug, Deserialize)]
struct DiskMetadata {
    size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageMetadata {
    fs_uuid: Option<String>,
}

fn metadata(reader: impl Read) -> Result<Metadata, Error> {
    let mut archive = Archive::new(reader.take(MAX_METADATA_BYTES));
    for entry in archive.entries().context("Invalid COSI archive")? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == Path::new(METADATA_PATH) {
            ensure!(
                entry.header().entry_type().is_file(),
                "COSI metadata must be a regular file"
            );
            ensure!(
                entry.size() <= MAX_METADATA_BYTES,
                "COSI metadata is too large"
            );
            let metadata: Metadata =
                serde_json::from_reader(&mut entry).context("Invalid COSI metadata")?;
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
            return Ok(metadata);
        }
    }
    bail!("COSI metadata.json was not found within the metadata read limit")
}

pub(super) fn stream_preflight(image: &Url, force: bool) -> Result<bool, Error> {
    let metadata = metadata(reader(image)?)?;
    let devices = lsblk::list().context("Failed to inspect attached disks")?;
    let required = metadata
        .disk
        .as_ref()
        .context("COSI disk metadata is missing")?
        .size;
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
            "StreamDisk could select a read-only or mounted disk '{}'; refusing to overwrite live media",
            device.device_path().display()
        );
    }
    if force {
        return Ok(false);
    }
    let expected = metadata
        .images
        .iter()
        .filter_map(|image| image.fs_uuid.as_deref())
        .filter(|uuid| !uuid.is_empty())
        .map(str::to_ascii_lowercase)
        .collect::<HashSet<_>>();
    ensure!(
        !expected.is_empty(),
        "COSI has no filesystem UUIDs for repeat-install protection"
    );
    Ok(contains_image(&devices, &expected))
}

fn contains_image(devices: &[BlockDevice], expected: &HashSet<String>) -> bool {
    fn collect(devices: &[BlockDevice], present: &mut HashSet<String>) {
        for device in devices {
            if let Some(uuid) = &device.fsuuid {
                present.insert(uuid.to_string().to_ascii_lowercase());
            }
            collect(&device.children, present);
        }
    }
    let mut present = HashSet::new();
    collect(devices, &mut present);
    expected.iter().all(|uuid| present.contains(uuid))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Cursor;

    use indoc::indoc;
    use tar::Builder as TarBuilder;
    use tempfile::TempDir;

    #[test]
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
    fn truncated_and_missing_metadata_are_errors() {
        metadata(Cursor::new(b"not a tar file")).unwrap_err();
        let mut builder = TarBuilder::new(Vec::new());
        builder.finish().unwrap();
        metadata(Cursor::new(builder.into_inner().unwrap())).unwrap_err();
    }

    #[test]
    fn repeat_guard_requires_the_complete_nonempty_uuid_set() {
        let devices = vec![BlockDevice {
            children: vec![BlockDevice {
                fsuuid: Some("uuid-1".into()),
                ..Default::default()
            }],
            ..Default::default()
        }];
        assert!(contains_image(&devices, &HashSet::from(["uuid-1".into()])));
        assert!(!contains_image(
            &devices,
            &HashSet::from(["uuid-1".into(), "uuid-2".into()])
        ));
    }
}
