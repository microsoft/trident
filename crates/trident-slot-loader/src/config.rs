use std::{
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};

const DEFAULT_STORE: &str = "/var/lib/aclslot";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Slot {
    A,
    B,
}

impl Slot {
    pub(super) fn parse(cmdline: &str) -> Result<Self, Error> {
        let mut values = cmdline
            .split_ascii_whitespace()
            .filter_map(|word| word.strip_prefix("acl.slot="));
        let slot = match values.next() {
            Some("a") => Self::A,
            Some("b") => Self::B,
            other => bail!("missing or invalid acl.slot: {other:?}"),
        };
        ensure!(values.next().is_none(), "duplicate acl.slot");
        Ok(slot)
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfextMode {
    Overlay,
    Systemd,
}

impl ConfextMode {
    pub(super) fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "overlay" => Ok(Self::Overlay),
            "systemd" => Ok(Self::Systemd),
            _ => bail!("unsupported CONFEXT_MODE {value:?}"),
        }
    }
}

pub(super) struct Config {
    pub(super) root: PathBuf,
    pub(super) run: PathBuf,
    pub(super) host_store: PathBuf,
}

impl Config {
    pub(super) fn new(root: PathBuf, run: PathBuf, store: Option<OsString>) -> Result<Self, Error> {
        let host_store = store
            .map(PathBuf::from)
            .unwrap_or_else(|| DEFAULT_STORE.into());
        ensure!(
            host_store.is_absolute()
                && host_store != Path::new("/")
                && host_store
                    .components()
                    .skip(1)
                    .all(|part| matches!(part, Component::Normal(_))),
            "TRIDEN_SLOT_STORE must be a non-root absolute path without . or .."
        );
        Ok(Self {
            root,
            run,
            host_store,
        })
    }

    pub(super) fn store(&self) -> PathBuf {
        self.root.join(
            self.host_store
                .strip_prefix("/")
                .expect("invariant: absolute store"),
        )
    }

    pub(super) fn store_present(&self) -> Result<bool, Error> {
        let path = self.store();
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                directory(&path)?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error).with_context(|| format!("inspecting {}", path.display())),
        }
    }

    pub(super) fn slot_dir(&self, slot: Slot) -> Result<PathBuf, Error> {
        let path = self.store().join(slot.name());
        directory(&path)?;
        Ok(path)
    }
}

pub(super) fn directory(path: &Path) -> Result<(), Error> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        let metadata = fs::symlink_metadata(&current)
            .with_context(|| format!("inspecting directory {}", current.display()))?;
        ensure!(
            metadata.is_dir(),
            "{} is not a real directory",
            current.display()
        );
    }
    Ok(())
}

pub(super) fn regular_file(path: &Path) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        directory(parent)?;
    }
    ensure!(
        fs::symlink_metadata(path)
            .with_context(|| format!("inspecting {}", path.display()))?
            .is_file(),
        "{} is not a regular file",
        path.display()
    );
    Ok(())
}

pub(super) fn create_directory(path: &Path) -> Result<(), Error> {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => ensure!(
                metadata.is_dir(),
                "{} is not a real directory",
                current.display()
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .with_context(|| format!("creating {}", current.display()))?;
            }
            Err(error) => {
                return Err(error).with_context(|| format!("inspecting {}", current.display()))
            }
        }
    }
    Ok(())
}

pub(super) fn reboot_enabled(value: Option<&str>) -> Result<bool, Error> {
    match value {
        None | Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(other) => bail!("invalid REBOOT_ON_UPDATE_LOADER_ERROR {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs::symlink;

    #[test]
    fn slot_is_explicit_and_unambiguous() {
        assert_eq!(Slot::parse("quiet acl.slot=a").unwrap(), Slot::A);
        assert_eq!(Slot::parse("acl.slot=b console=tty0").unwrap(), Slot::B);
        for invalid in [
            "",
            "acl.slot=A",
            "acl.slot=c",
            "acl.slot=a acl.slot=a",
            "acl.slot=a acl.slot=b",
        ] {
            Slot::parse(invalid).unwrap_err();
        }
    }

    #[test]
    fn store_uses_one_host_path_and_rejects_symlink_ancestors() {
        let temporary = tempfile::tempdir().unwrap();
        let config =
            Config::new(temporary.path().into(), temporary.path().join("run"), None).unwrap();
        assert_eq!(config.store(), temporary.path().join("var/lib/aclslot"));
        assert!(!config.store_present().unwrap());
        fs::create_dir_all(config.store()).unwrap();
        assert!(config.store_present().unwrap());
        symlink("aclslot", temporary.path().join("var/lib/alias")).unwrap();
        let config = Config::new(
            temporary.path().into(),
            temporary.path().join("run"),
            Some("/var/lib/alias".into()),
        )
        .unwrap();
        config.store_present().unwrap_err();
        for path in ["relative", "/", "/var/../etc"] {
            assert!(Config::new("/sysroot".into(), "/run".into(), Some(path.into())).is_err());
        }
    }
}
