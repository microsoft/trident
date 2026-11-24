//! Resolves boot identity and builds a read-only activation plan before any runtime changes.

use std::{fmt, path::PathBuf};

use anyhow::{bail, ensure, Context, Error};

use crate::{
    config::{ConfextMode, Paths},
    filesystem,
    manifest::Manifest,
};

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

impl fmt::Display for Slot {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// A native activation output: target is absolute in the real host root, not the initrd.
#[derive(Debug)]
pub(super) struct NativeLink {
    pub(super) destination: PathBuf,
    pub(super) target: PathBuf,
}

/// A confext requiring dissection, validation and a retained read-only mount.
#[derive(Debug)]
pub(super) struct OverlayImage {
    pub(super) name: String,
    pub(super) source: PathBuf,
    pub(super) mountpoint: PathBuf,
}

pub(super) struct Plan {
    pub(super) links: Vec<NativeLink>,
    pub(super) mounts: Vec<OverlayImage>,
}

#[derive(Clone, Copy)]
enum Kind {
    Sysext,
    Confext,
}

impl Kind {
    fn directory(self) -> &'static str {
        match self {
            Self::Sysext => "sysexts",
            Self::Confext => "confexts",
        }
    }

    fn native_directory(self) -> &'static str {
        match self {
            Self::Sysext => "extensions",
            Self::Confext => "confexts",
        }
    }
}

impl Plan {
    pub(super) fn build(
        paths: &Paths,
        slot: Slot,
        manifest: &Manifest,
        mode: ConfextMode,
    ) -> Result<Self, Error> {
        let mut plan = Self {
            links: Vec::new(),
            mounts: Vec::new(),
        };
        for (kind, entries) in [
            (Kind::Sysext, &manifest.sysexts),
            (Kind::Confext, &manifest.confexts),
        ] {
            for entry in entries {
                let filename = format!("{}.raw", entry.name);
                let relative = PathBuf::from(slot.name())
                    .join(kind.directory())
                    .join(&filename);
                let source = paths.store().join(&relative);
                filesystem::regular_file(&source).with_context(|| {
                    format!("selected image slot={slot} path={}", source.display())
                })?;
                if matches!(kind, Kind::Confext) && mode == ConfextMode::Overlay {
                    plan.mounts.push(OverlayImage {
                        name: entry.name.clone(),
                        source,
                        mountpoint: paths
                            .runtime()
                            .join("trident-slot-loader/etc")
                            .join(slot.name())
                            .join(&entry.name),
                    });
                } else {
                    plan.links.push(NativeLink {
                        destination: paths.runtime().join(kind.native_directory()).join(filename),
                        target: paths.host_store().join(relative),
                    });
                }
            }
        }
        Ok(plan)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.links.is_empty() && self.mounts.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
