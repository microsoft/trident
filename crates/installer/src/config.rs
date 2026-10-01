use std::{
    fs,
    io::ErrorKind,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};
use serde::{Deserialize, Serialize};
use toml::Value as TomlValue;

pub(super) const DEFAULT_CONFIG: &str = "/etc/trident/installer.toml";
pub(super) const DEFAULT_HOST_CONFIGURATION: &str = "/etc/trident/config.yaml";
pub(super) const MEDIA_CONFIG: &str = "installer/installer.toml";
pub(super) const MEDIA_MOUNT: &str = "/run/trident/installer-media";
pub(super) const DEFAULT_MEDIA_LABEL: &str = "TRIDENT_INSTALL";

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Config {
    pub mode: Mode,
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

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Media {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cdrom_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mount_path: Option<PathBuf>,
    #[serde(default = "default_cosi_directory")]
    pub cosi_directory: PathBuf,
}

impl Default for Media {
    fn default() -> Self {
        Self {
            cdrom_label: None,
            mount_path: None,
            cosi_directory: default_cosi_directory(),
        }
    }
}

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
}

impl Default for Autorun {
    fn default() -> Self {
        Self {
            host_configuration: None,
            reboot: default_reboot(),
        }
    }
}

fn default_reboot() -> bool {
    true
}

impl Config {
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

    pub(super) fn parse(text: &str) -> Result<Self, Error> {
        let config: Self = toml::from_str(text).context("Invalid installer TOML configuration")?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), Error> {
        ensure!(
            self.media.cdrom_label.is_none() || self.media.mount_path.is_none(),
            "media.cdromLabel and media.mountPath are mutually exclusive"
        );
        if let Some(label) = &self.media.cdrom_label {
            ensure!(
                !label.trim().is_empty(),
                "media.cdromLabel must not be empty"
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

    pub(super) fn require_autorun(&self) -> Result<(), Error> {
        ensure!(
            self.mode == Mode::Autorun,
            "Interactive installation is not implemented yet; choose mode = \"autorun\""
        );
        Ok(())
    }

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
        if let Some(media) = overlay.get("media") {
            ensure!(
                media.get("cdromLabel").is_none() && media.get("mountPath").is_none(),
                "Media configuration cannot change bootstrap cdromLabel or mountPath"
            );
        }
        let mut base = TomlValue::try_from(self)?;
        merge(&mut base, overlay);
        let config: Self = base.try_into()?;
        config.validate()?;
        Ok(config)
    }
}

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
    fn explicit_mode_and_strict_fields() {
        Config::parse("").unwrap_err();
        Config::parse("mode = 'autorun'\nunknown = true").unwrap_err();
        let config = Config::parse("mode = 'autorun'").unwrap();
        assert!(config.autorun.reboot);
        assert_eq!(config.media.cosi_directory, Path::new("cosi"));
        Config::parse("mode = 'interactive'")
            .unwrap()
            .require_autorun()
            .unwrap_err();
    }

    #[test]
    fn packaged_configuration_matches_the_runtime_schema() {
        let config = Config::parse(include_str!("../../../packaging/installer.toml")).unwrap();
        config.require_autorun().unwrap();
        assert_eq!(
            config.media.cdrom_label.as_deref(),
            Some(DEFAULT_MEDIA_LABEL)
        );
        assert!(config.autorun.reboot);
    }

    #[test]
    fn media_selector_and_path_validation() {
        Config::parse(indoc! {"
            mode = 'autorun'
            [media]
            cdromLabel = 'TEST'
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
    fn media_overlay_preserves_defaults_and_bootstrap() {
        let root = TempDir::new().unwrap();
        fs::create_dir(root.path().join("installer")).unwrap();
        let path = root.path().join(MEDIA_CONFIG);
        fs::write(&path, "[autorun]\nreboot = false").unwrap();
        let config = Config::parse("mode = 'autorun'")
            .unwrap()
            .overlay(root.path())
            .unwrap();
        assert!(!config.autorun.reboot);
        assert_eq!(config.media.cosi_directory, Path::new("cosi"));
        fs::write(&path, "[media]\ncdromLabel = 'OTHER'").unwrap();
        config.overlay(root.path()).unwrap_err();
    }
}
