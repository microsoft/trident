use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};
use log::LevelFilter;
use serde::{Deserialize, Serialize};
use toml::Value as TomlValue;

pub(super) const DEFAULT_CONFIG: &str = "/etc/trident/installer.toml";
pub(super) const DEFAULT_HOST_CONFIGURATION: &str = "/etc/trident/config.yaml";
pub(super) const MEDIA_CONFIG: &str = "installer/installer.toml";
pub(super) const MEDIA_MOUNT: &str = "/run/trident/installer-media";
pub(super) const DEFAULT_MEDIA_LABEL: &str = "TRD_INSTALL";

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Config {
    pub mode: Mode,
    #[serde(default)]
    pub serial_mode: SerialMode,
    #[serde(default)]
    pub serial_verbosity: SerialVerbosity,
    #[serde(default)]
    pub media: Media,
    #[serde(default)]
    pub autorun: Autorun,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum Mode {
    #[default]
    Autorun,
    Interactive,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum SerialMode {
    #[default]
    Logs,
    Ui,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub(super) enum SerialVerbosity {
    Off,
    Error,
    Warn,
    Info,
    #[default]
    Debug,
    Trace,
}

impl SerialVerbosity {
    /// Converts the configured serial level into the log filter used when mirroring records.
    pub(super) fn filter(self) -> LevelFilter {
        match self {
            Self::Off => LevelFilter::Off,
            Self::Error => LevelFilter::Error,
            Self::Warn => LevelFilter::Warn,
            Self::Info => LevelFilter::Info,
            Self::Debug => LevelFilter::Debug,
            Self::Trace => LevelFilter::Trace,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Media {
    #[serde(default, alias = "cdromLabel", skip_serializing_if = "Option::is_none")]
    pub filesystem_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_path: Option<PathBuf>,
    #[serde(default = "default_cosi_directory")]
    pub cosi_directory: PathBuf,
}

impl Default for Media {
    /// Leaves media discovery on the default label and searches the `cosi/` directory.
    fn default() -> Self {
        Self {
            filesystem_label: None,
            mount_path: None,
            cosi_directory: default_cosi_directory(),
        }
    }
}

/// Returns the relative COSI directory used when the media config omits it.
fn default_cosi_directory() -> PathBuf {
    "cosi".into()
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Autorun {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_configuration: Option<String>,
    #[serde(default = "default_reboot")]
    pub reboot: bool,
    #[serde(default)]
    pub force: bool,
}

impl Default for Autorun {
    /// Enables reboot after a successful install but never forces a repeat stream.
    fn default() -> Self {
        Self {
            host_configuration: None,
            reboot: default_reboot(),
            force: false,
        }
    }
}

/// Enables reboot when the autorun configuration does not specify a preference.
fn default_reboot() -> bool {
    true
}

impl Config {
    /// Reads and validates the bootstrap installer configuration from disk.
    pub(super) fn read(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let text = fs::read_to_string(path).with_context(|| {
            format!(
                "Failed to read installer configuration '{}'",
                path.display()
            )
        })?;
        Self::parse(&text)
    }

    /// Parses installer TOML, rejecting unknown fields and unsupported settings.
    pub(super) fn parse(text: &str) -> Result<Self, Error> {
        let config: Self = toml::from_str(text).context("Invalid installer TOML configuration")?;
        config.validate()?;
        Ok(config)
    }

    /// Checks media paths, source fields, and the reserved serial UI mode.
    fn validate(&self) -> Result<(), Error> {
        ensure!(
            self.serial_mode == SerialMode::Logs,
            "serialMode = \"ui\" is not implemented yet; use \"logs\""
        );
        ensure!(
            self.media.filesystem_label.is_none() || self.media.mount_path.is_none(),
            "media.filesystemLabel and media.mountPath are mutually exclusive"
        );
        if let Some(label) = &self.media.filesystem_label {
            ensure!(
                !label.trim().is_empty(),
                "media.filesystemLabel must not be empty"
            );
        }
        if let Some(path) = &self.media.mount_path {
            ensure!(path.is_absolute(), "media.mountPath must be absolute");
        }
        ensure!(
            !self.media.cosi_directory.as_os_str().is_empty()
                && self.media.cosi_directory.components().all(|component| {
                    matches!(component, Component::Normal(_) | Component::CurDir)
                }),
            "media.cosiDirectory must be a nonempty relative path beneath the media root"
        );
        if let Some(source) = &self.autorun.host_configuration {
            ensure!(
                !source.trim().is_empty(),
                "autorun.hostConfiguration must not be empty"
            );
        }
        Ok(())
    }

    /// Rejects interactive mode until interactive source selection is implemented.
    pub(super) fn require_autorun(&self) -> Result<(), Error> {
        ensure!(
            self.mode == Mode::Autorun,
            "Interactive installation is not implemented yet; choose mode = \"autorun\""
        );
        Ok(())
    }

    /// Applies an optional media-local config without allowing bootstrap media or serial changes.
    pub(super) fn overlay(&self, root: impl AsRef<Path>) -> Result<Self, Error> {
        let path = root.as_ref().join(MEDIA_CONFIG);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(self.clone()),
            Err(error) => {
                return Err(error).with_context(|| format!("Failed to read '{}'", path.display()))
            }
        };
        let overlay: TomlValue = toml::from_str(&text)
            .with_context(|| format!("Invalid media configuration '{}'", path.display()))?;
        ensure!(
            overlay.get("serialMode").is_none(),
            "Media configuration cannot change bootstrap serialMode"
        );
        if let Some(media) = overlay.get("media") {
            ensure!(
                media.get("filesystemLabel").is_none()
                    && media.get("cdromLabel").is_none()
                    && media.get("mountPath").is_none(),
                "Media configuration cannot change bootstrap filesystemLabel or mountPath"
            );
        }
        let mut base = TomlValue::try_from(self)?;
        merge(&mut base, overlay);
        let config: Self = base.try_into()?;
        config.validate()?;
        Ok(config)
    }
}

/// Recursively overlays TOML tables; non-table values replace their base values.
fn merge(base: &mut TomlValue, overlay: TomlValue) {
    match (base, overlay) {
        (TomlValue::Table(base), TomlValue::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(base_value) => merge(base_value, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

/// Lists regular `.cosi` files directly beneath the media directory in name order.
///
/// Rejects directories that resolve outside the media root; an absent directory yields no images.
pub(super) fn discover_images(
    root: impl AsRef<Path>,
    directory: impl AsRef<Path>,
) -> Result<Vec<PathBuf>, Error> {
    let root = root
        .as_ref()
        .canonicalize()
        .context("Failed to resolve media root")?;
    let directory = root.join(directory);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("Failed to scan '{}'", directory.display()))
        }
    };
    ensure!(
        directory.canonicalize()?.starts_with(&root),
        "COSI directory escapes the media root"
    );
    let mut images = Vec::new();
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_file()
            && path
                .extension()
                .is_some_and(|extension| extension == "cosi")
        {
            images.push(path);
        }
    }
    images.sort();
    Ok(images)
}

/// Selects the sole media device, or fails rather than guessing between duplicate labels.
pub(super) fn choose_media(devices: &[PathBuf]) -> Result<Option<&Path>, Error> {
    match devices {
        [] => Ok(None),
        [device] => Ok(Some(device.as_path())),
        _ => bail!("Multiple devices match the installer media label: {devices:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs as unix_fs;

    use indoc::indoc;
    use tempfile::TempDir;

    #[test]
    /// Ensures mode is required and omitted settings keep their safe defaults.
    fn explicit_mode_and_strict_fields() {
        Config::parse("").unwrap_err();
        Config::parse("mode = 'autorun'\nunknown = true").unwrap_err();
        let config = Config::parse("mode = 'autorun'").unwrap();
        assert_eq!(config.serial_mode, SerialMode::Logs);
        assert_eq!(config.serial_verbosity, SerialVerbosity::Debug);
        assert!(config.autorun.reboot);
        assert!(!config.autorun.force);
        assert_eq!(config.media.cosi_directory, Path::new("cosi"));
        assert_eq!(DEFAULT_MEDIA_LABEL.len(), 11);
        Config::parse("mode = 'interactive'")
            .unwrap()
            .require_autorun()
            .unwrap_err();
    }

    #[test]
    /// Ensures force is false by default and accepts only a boolean override.
    fn autorun_force_is_opt_in() {
        let config = Config::parse("mode = 'autorun'\n[autorun]\nforce = true").unwrap();
        assert!(config.autorun.force);
        let config = Config::parse("mode = 'autorun'\n[autorun]\nforce = false").unwrap();
        assert!(!config.autorun.force);
        Config::parse("mode = 'autorun'\n[autorun]\nforce = 'yes'").unwrap_err();
    }

    #[test]
    /// Ensures the reserved serial UI mode fails validation instead of being enabled.
    fn serial_ui_mode_is_reserved_not_silently_enabled() {
        Config::parse("mode = 'autorun'\nserialMode = 'logs'").unwrap();
        let error = Config::parse("mode = 'autorun'\nserialMode = 'ui'").unwrap_err();
        assert!(error.to_string().contains("not implemented"), "{error:#}");
    }

    #[test]
    /// Ensures serial verbosity defaults to debug and invalid levels are rejected.
    fn serial_verbosity_defaults_to_debug_and_rejects_invalid_levels() {
        let config = Config::parse("mode = 'autorun'\nserialVerbosity = 'trace'").unwrap();
        assert_eq!(config.serial_verbosity.filter(), LevelFilter::Trace);
        let config = Config::parse("mode = 'autorun'\nserialVerbosity = 'off'").unwrap();
        assert_eq!(config.serial_verbosity.filter(), LevelFilter::Off);
        Config::parse("mode = 'autorun'\nserialVerbosity = 'verbose'").unwrap_err();
    }

    #[test]
    /// Ensures the shipped TOML stays compatible with the runtime schema.
    fn packaged_configuration_matches_the_runtime_schema() {
        let config = Config::parse(include_str!("../../../packaging/etc/installer.toml")).unwrap();
        config.require_autorun().unwrap();
        assert_eq!(
            config.media.filesystem_label.as_deref(),
            Some(DEFAULT_MEDIA_LABEL)
        );
        assert!(config.autorun.reboot);
    }

    #[test]
    /// Ensures media selectors are exclusive and the image directory stays relative.
    fn media_selector_and_path_validation() {
        Config::parse(indoc! {"
            mode = 'autorun'
            [media]
            filesystemLabel = 'TEST'
            mountPath = '/media'
        "})
        .unwrap_err();
        for directory in ["/cosi", "../cosi", "cosi/../../outside", ""] {
            Config::parse(&format!(
                "mode = 'autorun'\n[media]\ncosiDirectory = '{directory}'"
            ))
            .unwrap_err();
        }
        Config::parse("mode = 'autorun'\n[media]\nmountPath = '/media'").unwrap();
        choose_media(&[PathBuf::from("/dev/sr0"), PathBuf::from("/dev/sr1")]).unwrap_err();
    }

    #[test]
    /// Ensures image discovery is sorted, nonrecursive, and excludes symlinks.
    fn catalog_is_sorted_nonrecursive_and_ignores_symlinks() {
        let root = TempDir::new().unwrap();
        let directory = root.path().join("cosi");
        fs::create_dir(&directory).unwrap();
        for filename in ["z.cosi", "a.cosi", "readme.txt"] {
            fs::write(directory.join(filename), "").unwrap();
        }
        fs::create_dir(directory.join("nested")).unwrap();
        fs::write(directory.join("nested/hidden.cosi"), "").unwrap();
        unix_fs::symlink(directory.join("z.cosi"), directory.join("0.cosi")).unwrap();
        assert_eq!(
            discover_images(root.path(), "cosi").unwrap(),
            [directory.join("a.cosi"), directory.join("z.cosi")]
        );
    }

    #[test]
    /// Ensures media overlays cannot change bootstrap settings or discard defaults.
    fn media_overlay_preserves_defaults_and_bootstrap() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("installer")).unwrap();
        let path = root.path().join(MEDIA_CONFIG);
        fs::write(&path, "[autorun]\nreboot = false\nforce = true").unwrap();
        let config = Config::parse("mode = 'autorun'")
            .unwrap()
            .overlay(root.path())
            .unwrap();
        assert!(!config.autorun.reboot);
        assert!(config.autorun.force);
        assert_eq!(config.media.cosi_directory, Path::new("cosi"));
        fs::write(&path, "[media]\nfilesystemLabel = 'OTHER'").unwrap();
        config.overlay(root.path()).unwrap_err();
        fs::write(&path, "[media]\ncdromLabel = 'OTHER'").unwrap();
        config.overlay(root.path()).unwrap_err();
        fs::write(&path, "serialMode = 'ui'").unwrap();
        config.overlay(root.path()).unwrap_err();
    }

    #[test]
    /// Ensures the former CD-ROM key maps to the filesystem-label setting.
    fn legacy_cdrom_label_alias_still_mounts_the_same_filesystem() {
        let legacy = Config::parse("mode = 'autorun'\n[media]\ncdromLabel = 'CDROM'").unwrap();
        let generic =
            Config::parse("mode = 'autorun'\n[media]\nfilesystemLabel = 'CDROM'").unwrap();
        assert_eq!(legacy, generic);
        Config::parse(
            "mode = 'autorun'\n[media]\ncdromLabel = 'CDROM'\nfilesystemLabel = 'TRD_INSTALL'",
        )
        .unwrap_err();
    }
}
