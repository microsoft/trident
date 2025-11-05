use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use serde_json::Value;

use sysdefs::filesystems::NodevFilesystemType;
use trident_api::constants::ROOT_MOUNT_POINT_PATH;

use crate::{dependencies::Dependency, filesystems::TabFileSystemType};

/// A representation of a fstab file.
#[derive(Debug, Default)]
pub struct TabFile {
    pub entries: Vec<TabFileEntry>,
}

/// A representation of a single entry in a tab file.
#[derive(Debug, PartialEq, Eq)]
pub struct TabFileEntry {
    pub device: TabDevice,
    pub mount_point: TabMountPoint,
    pub fs_type: TabFileSystemType,
    pub options: Vec<String>,

    /// Whether this entry is disabled (commented out).
    /// If `None`, the entry is enabled.
    /// If `Some`, the entry is disabled and the reason is provided.
    pub disabled_reason: Option<String>,
}

/// A representation of a device in a tab file.
#[derive(Debug, PartialEq, Eq)]
pub enum TabDevice {
    None,
    Overlay,
    Tmpfs,
    BlockDevice(PathBuf),
}

/// A representation of a mount point in a tab file.
#[derive(Debug, PartialEq, Eq)]
pub enum TabMountPoint {
    None,
    Path(PathBuf),
}

impl TabFile {
    /// Write this tab file to disk at location `tab_file_path`.
    pub fn write(&self, tab_file_path: impl AsRef<Path>) -> Result<(), Error> {
        std::fs::write(tab_file_path.as_ref(), self.render().as_bytes())
            .with_context(|| format!("Failed to write new {}", tab_file_path.as_ref().display()))
    }

    /// Render this tab file as a string.
    pub fn render(&self) -> String {
        self.entries.iter().map(|entry| entry.render()).collect()
    }

    pub fn merge_and_write(&self, tab_file_path: impl AsRef<Path>) -> Result<(), Error> {
        let existing_contents = std::fs::read_to_string(tab_file_path.as_ref())
            .context("Failed to read existing fstab file")?;

        let merged_contents = self.merge_with_existing(&existing_contents);
        std::fs::write(tab_file_path.as_ref(), merged_contents.as_bytes()).with_context(|| {
            format!(
                "Failed to write merged {}",
                tab_file_path.as_ref().display()
            )
        })
    }

    pub fn merge_with_existing(&self, existing: &str) -> String {
        let mut merged = String::new();

        let mount_points = self
            .entries
            .iter()
            .filter_map(|e| match e.mount_point {
                TabMountPoint::Path(ref p) => Some(&**p),
                TabMountPoint::None => None,
            })
            .collect::<HashSet<_>>();

        for line in existing.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                let parts: Vec<&str> = trimmed.split_whitespace().collect();
                if let Some(ref path) = parts.get(1) {
                    if mount_points.contains(Path::new(path)) {
                        continue;
                    }
                }
            }

            merged.push_str(line);
            merged.push('\n');
        }

        merged.push_str("\nEntries below were created by Trident:\n");
        merged.push_str(&self.render());
        merged
    }
}

impl TabFileEntry {
    /// Create a new regular entry for a block device mounted at a path.
    pub fn new_path(
        device: impl Into<PathBuf>,
        mount_point: impl Into<PathBuf>,
        fs_type: TabFileSystemType,
    ) -> Self {
        Self {
            device: TabDevice::BlockDevice(device.into()),
            mount_point: TabMountPoint::Path(mount_point.into()),
            fs_type,
            options: Vec::new(),
            disabled_reason: None,
        }
    }

    /// Create a new entry for a block device mounted as swap.
    pub fn new_swap(device: impl Into<PathBuf>) -> Self {
        Self {
            device: TabDevice::BlockDevice(device.into()),
            mount_point: TabMountPoint::None,
            fs_type: TabFileSystemType::Swap,
            options: Vec::new(),
            disabled_reason: None,
        }
    }

    /// Create a new entry for a tmpfs mount.
    pub fn new_tmpfs(mount_point: impl Into<PathBuf>) -> Self {
        Self {
            device: TabDevice::Tmpfs,
            mount_point: TabMountPoint::Path(mount_point.into()),
            fs_type: NodevFilesystemType::Tmpfs.into(),
            options: Vec::new(),
            disabled_reason: None,
        }
    }

    /// Create a new entry for an overlay mount.
    pub fn new_overlay(mount_point: impl Into<PathBuf>) -> Self {
        Self {
            device: TabDevice::Overlay,
            mount_point: TabMountPoint::Path(mount_point.into()),
            fs_type: NodevFilesystemType::Overlay.into(),
            options: Vec::new(),
            disabled_reason: None,
        }
    }

    /// Add options to this entry.
    pub fn with_options(mut self, options: Vec<String>) -> Self {
        self.options = options;
        self
    }

    /// Disable the entry with a reason.
    pub fn with_disabled_reason(mut self, reason: Option<impl Into<String>>) -> Self {
        self.disabled_reason = reason.map(Into::into);
        self
    }

    /// Render this entry as a string suitable for writing to a tab file.
    pub fn render(&self) -> String {
        // fsck pass is 1 for root, 2 for everything else, 0 for none
        let fsck_pass = match self.mount_point {
            TabMountPoint::None => 0,
            TabMountPoint::Path(ref path) if path == Path::new(ROOT_MOUNT_POINT_PATH) => 1,
            _ => 2,
        };

        // If the options are empty, use "defaults" as the default
        let options = if self.options.is_empty() {
            "defaults".into()
        } else {
            self.options.join(",")
        };

        let line = format!(
            "{} {} {} {} 0 {}\n",
            self.device.render(),
            self.mount_point.render(),
            self.fs_type.name(),
            options,
            fsck_pass,
        );

        match &self.disabled_reason {
            // If the entry is disabled, comment it out and add the reason.
            // Replace all newlines with newlines followed by a `#` to keep the
            // comment formatting.
            Some(reason) => {
                let escaped = reason.replace("\n", "\n# ");
                format!("# {escaped}\n# {line}")
            }

            // If the entry is enabled, just use an empty string.
            None => line,
        }
    }
}

impl TabDevice {
    /// Render this device as a string.
    pub fn render(&self) -> String {
        match self {
            TabDevice::None => "none".to_string(),
            TabDevice::Overlay => "overlay".to_string(),
            TabDevice::Tmpfs => "tmpfs".to_string(),
            TabDevice::BlockDevice(path) => path.to_string_lossy().to_string(),
        }
    }
}

impl TabMountPoint {
    /// Render this mount point as a string.
    pub fn render(&self) -> String {
        match self {
            TabMountPoint::None => "none".to_string(),
            TabMountPoint::Path(path) => path.to_string_lossy().to_string(),
        }
    }
}

/// Based on the given tab file, get the device path for the partition with mount point `path`.
pub fn get_device_path(tab_file_path: &Path, path: &Path) -> Result<PathBuf, Error> {
    let findmnt_output_json = Dependency::Findmnt
        .cmd()
        .arg("--tab-file")
        .arg(tab_file_path)
        .arg("--json")
        .arg("--output")
        .arg("source,target,fstype,vfs-options,fs-options,freq,passno")
        .arg("--mountpoint")
        .arg(path)
        .output_and_check()
        .context(format!("Failed to find {path:?} in {tab_file_path:?}"))?;
    let map = parse_findmnt_output(findmnt_output_json.as_str())?;
    if map.len() != 1 {
        bail!(
            "Unexpected number of entries in the tab file matching the mount point '{}'",
            path.display()
        );
    }

    let device_path = map.get(path).context(format!(
        "Failed to find entry in the tab file matching the mount point '{}'",
        path.display()
    ))?;

    Ok(device_path.clone())
}

/// Parse the output of the `findmnt` utility into a map of mount points to device paths.
fn parse_findmnt_output(findmnt_output: &str) -> Result<HashMap<PathBuf, PathBuf>, Error> {
    let payload: Value = serde_json::from_str(findmnt_output)
        .context("Failed to deserialize output of tab file reader")?;

    let filesystems = payload["filesystems"].as_array().context(format!(
        "Unexpected formatting of the findmnt utility, missing 'filesystems' in {payload:?}"
    ))?;

    // returns the first error or the list of results
    filesystems.iter().map(parse_findmnt_entry).collect()
}

/// Parse a single entry from the `findmnt` utility output.
fn parse_findmnt_entry(entry: &Value) -> Result<(PathBuf, PathBuf), Error> {
    let device_path = entry["source"].as_str().context(format!(
        "Unexpected formatting of the findmnt utility, missing 'source' in {entry:?}"
    ))?;

    let mount_path = entry["target"].as_str().context(format!(
        "Unexpected formatting of the findmnt utility, missing 'target' in {entry:?}"
    ))?;

    Ok((PathBuf::from(mount_path), PathBuf::from(device_path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        collections::HashMap,
        io::Write,
        path::{Path, PathBuf},
    };

    use tempfile::NamedTempFile;

    use trident_api::constants::ESP_MOUNT_POINT_PATH;

    #[test]
    fn test_get_device_path() {
        let tab_file_contents = indoc::indoc! {r#"
                /dev/sda1 /boot/efi vfat defaults 0 0
                /dev/sda2 / ext4 errors=remount-ro 0 0
                /dev/sdb1 /random ext4 defaults 0 2
            "#}
        .to_owned();

        // Save that temporary file
        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile.write_all(tab_file_contents.as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        assert_eq!(
            get_device_path(tmpfile.path(), Path::new(ESP_MOUNT_POINT_PATH)).unwrap(),
            PathBuf::from("/dev/sda1")
        );

        assert_eq!(
            get_device_path(tmpfile.path(), Path::new(ROOT_MOUNT_POINT_PATH)).unwrap(),
            PathBuf::from("/dev/sda2")
        );

        assert_eq!(
            get_device_path(tmpfile.path(), Path::new("/random")).unwrap(),
            PathBuf::from("/dev/sdb1")
        );

        // non-existing mount point
        assert_eq!(
            get_device_path(tmpfile.path(), Path::new("/foobar"))
                .err()
                .unwrap()
                .to_string(),
            format!("Failed to find \"/foobar\" in {:?}", tmpfile.path())
        );

        // non-existing input file
        assert_eq!(
            get_device_path(Path::new("/does-not-exist"), Path::new("/foobar"))
                .err()
                .unwrap()
                .to_string(),
            "Failed to find \"/foobar\" in \"/does-not-exist\""
        );

        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile.write_all("malformed".as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        // malformed input file
        assert_eq!(
            get_device_path(tmpfile.path(), Path::new("/foobar"))
                .err()
                .unwrap()
                .to_string(),
            format!("Failed to find \"/foobar\" in {:?}", tmpfile.path())
        );

        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile
            .write_all((tab_file_contents + "\n/dev/sdb1q /random ext4 defaults 0 2").as_bytes())
            .unwrap();
        tmpfile.flush().unwrap();

        // pick the latter
        assert_eq!(
            get_device_path(tmpfile.path(), Path::new("/random")).unwrap(),
            PathBuf::from("/dev/sdb1q")
        );
    }

    #[test]
    fn test_parse_findmnt_entry() {
        let input_json = r#"{"source":"foo","target":"bar"}"#;
        let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap();

        assert_eq!(
            super::parse_findmnt_entry(&input).unwrap(),
            (PathBuf::from("bar"), PathBuf::from("foo"))
        );

        // missing target
        let input_json = r#"{"source":"foo"}"#;
        let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap();
        assert!(super::parse_findmnt_entry(&input).is_err());

        // missing source
        let input_json = r#"{"target":"foo"}"#;
        let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap();
        assert!(super::parse_findmnt_entry(&input).is_err());

        // missing target and source
        let input_json = r#"{"foo":"foo"}"#;
        let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap();
        assert!(super::parse_findmnt_entry(&input).is_err());
    }

    #[test]
    fn test_parse_findmnt_output() {
        let input = r#"{"filesystems": [{"source":"foo","target":"bar"}]}"#;
        let output: HashMap<PathBuf, PathBuf> = [(PathBuf::from("bar"), PathBuf::from("foo"))]
            .iter()
            .cloned()
            .collect();
        assert_eq!(super::parse_findmnt_output(input).unwrap(), output);

        // missing target
        let input = r#"{"filesystems": [{"source":"foo"}]}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        // missing source
        let input = r#"{"filesystems": [{"target":"foo"}]}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        // missing target and source
        let input = r#"{"filesystems": [{"foo":"foo"}]}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        let input = r#"{"filesystems": []}"#;
        assert!(super::parse_findmnt_output(input).unwrap().is_empty());

        let input = r#"{"filesystems": [{"source":"foo","target":"bar"},{"source":"foo2","target":"bar2"}]}"#;
        assert_eq!(super::parse_findmnt_output(input).unwrap().len(), 2);

        // no filesystems
        let input = r#"{"foo": []}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        // filesystems is not an array
        let input = r#"{"filesystems": {"foo": "bar"}}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        // one entry is malformed
        let input = r#"{"filesystems": [{"source":"foo","target":"bar"},{"sourcssse":"foo2","target":"bar"},{"source":"foo2","target":"bar"}]}"#;
        assert!(super::parse_findmnt_output(input).is_err());
    }
}
