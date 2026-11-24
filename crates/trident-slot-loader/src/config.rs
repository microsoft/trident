//! Environment parsing and path derivation. This module performs no filesystem I/O.

use std::{
    env,
    ffi::OsString,
    fmt,
    path::{Component, Path, PathBuf},
};

use anyhow::{bail, ensure, Error};
use log::LevelFilter;

const DEFAULT_STORE: &str = "/var/lib/aclslot";
const SYSROOT: &str = "/sysroot";
const RUNTIME_ROOT: &str = "/run";

/// Confext activation belongs either to our writable-overlay preparation or to systemd, never both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ConfextMode {
    /// Retain read-only mounts and publish their /etc directories for Bootengine.
    Overlay,
    /// Publish native confext links; the loader must not mount these images.
    Systemd,
}

impl ConfextMode {
    fn parse(value: &str) -> Result<Self, Error> {
        match value {
            "overlay" => Ok(Self::Overlay),
            "systemd" => Ok(Self::Systemd),
            _ => bail!("unsupported CONFEXT_MODE {value:?}; expected overlay or systemd"),
        }
    }
}

impl fmt::Display for ConfextMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Overlay => "overlay",
            Self::Systemd => "systemd",
        })
    }
}

/// Keeps initrd access paths distinct from the host paths stored in native symlinks.
pub(super) struct Paths {
    sysroot: PathBuf,
    runtime: PathBuf,
    host_store: PathBuf,
    store: PathBuf,
}

impl Paths {
    pub(super) fn from_environment() -> Result<Self, Error> {
        let store = env::var_os("TRIDEN_SLOT_STORE").unwrap_or_else(|| DEFAULT_STORE.into());
        Self::new(SYSROOT.into(), RUNTIME_ROOT.into(), store.into())
    }

    pub(super) fn new(
        sysroot: PathBuf,
        runtime: PathBuf,
        host_store: PathBuf,
    ) -> Result<Self, Error> {
        for path in [&sysroot, &runtime, &host_store] {
            ensure!(
                path.is_absolute()
                    && path != Path::new("/")
                    && path
                        .components()
                        .skip(1)
                        .all(|part| matches!(part, Component::Normal(_))),
                "expected a non-root absolute path without parent traversal: {}",
                path.display()
            );
        }
        let store = sysroot.join(host_store.strip_prefix("/")?);
        Ok(Self {
            sysroot,
            runtime,
            host_store,
            store,
        })
    }

    pub(super) fn store(&self) -> &Path {
        &self.store
    }

    pub(super) fn host_store(&self) -> &Path {
        &self.host_store
    }

    pub(super) fn runtime(&self) -> &Path {
        &self.runtime
    }

    pub(super) fn sysroot(&self) -> &Path {
        &self.sysroot
    }

    pub(super) fn lowerdirs(&self) -> PathBuf {
        self.runtime.join("trident-slot-loader/etc-lowerdirs")
    }
}

pub(super) fn activation_from_environment() -> Result<ConfextMode, Error> {
    activation(&|name| env::var_os(name))
}

pub(super) fn recovery_from_environment() -> Result<bool, Error> {
    recovery(&|name| env::var_os(name))
}

pub(super) fn log_level_from_environment() -> Result<LevelFilter, Error> {
    setting(
        &|name| env::var_os(name),
        "TRIDENT_SLOT_LOADER_LOG_LEVEL",
        "info",
    )?
    .parse()
    .map_err(|error| anyhow::anyhow!("invalid TRIDENT_SLOT_LOADER_LOG_LEVEL: {error}"))
}

fn activation(environment: &impl Fn(&str) -> Option<OsString>) -> Result<ConfextMode, Error> {
    let mode = setting(environment, "LOADER_MODE", "acl")?;
    ensure!(
        mode == "acl",
        "unsupported LOADER_MODE {mode:?}; expected acl"
    );
    ConfextMode::parse(&setting(environment, "CONFEXT_MODE", "overlay")?)
}

fn recovery(environment: &impl Fn(&str) -> Option<OsString>) -> Result<bool, Error> {
    match setting(environment, "REBOOT_ON_UPDATE_LOADER_ERROR", "true")?.as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        other => bail!("invalid REBOOT_ON_UPDATE_LOADER_ERROR {other:?}; expected true or false"),
    }
}

fn setting(
    environment: &impl Fn(&str) -> Option<OsString>,
    name: &str,
    default: &str,
) -> Result<String, Error> {
    environment(name).map_or_else(
        || Ok(default.to_string()),
        |value| {
            value
                .into_string()
                .map_err(|value| anyhow::anyhow!("{name} is not UTF-8: {value:?}"))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{collections::HashMap, os::unix::ffi::OsStringExt};

    #[test]
    fn modes_are_typed_and_invalid_values_do_not_fall_back() {
        assert_eq!(activation(&|_| None).unwrap(), ConfextMode::Overlay);
        assert!(recovery(&|_| None).unwrap());
        let values = HashMap::from([
            ("CONFEXT_MODE", "systemd"),
            ("REBOOT_ON_UPDATE_LOADER_ERROR", "false"),
        ]);
        let environment = |name: &str| values.get(name).map(OsString::from);
        assert_eq!(activation(&environment).unwrap(), ConfextMode::Systemd);
        assert!(!recovery(&environment).unwrap());
        activation(&|_| Some("invalid".into())).unwrap_err();
        recovery(&|_| Some("invalid".into())).unwrap_err();
        activation(&|_| Some(OsString::from_vec(vec![0xff]))).unwrap_err();
    }

    #[test]
    fn recovery_does_not_depend_on_activation_settings() {
        assert!(recovery(&|name| { (name == "CONFEXT_MODE").then(|| "invalid".into()) }).unwrap());
    }

    #[test]
    fn one_store_path_produces_distinct_initrd_and_host_views() {
        let paths = Paths::new("/sysroot".into(), "/run".into(), DEFAULT_STORE.into()).unwrap();
        assert_eq!(paths.store(), Path::new("/sysroot/var/lib/aclslot"));
        assert_eq!(paths.host_store(), Path::new(DEFAULT_STORE));
        for invalid in ["relative", "/", "/var/../etc"] {
            assert!(Paths::new("/sysroot".into(), "/run".into(), invalid.into()).is_err());
        }
    }
}
