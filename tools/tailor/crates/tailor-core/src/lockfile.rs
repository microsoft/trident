//! The lockfile (`tailor.lock`) — a flat, deduplicated set of resolved registry references to
//! immutable digests (`meta/docs/2026-06-22-design.md` §9.1). It pins only re-fetchable inputs: toolchain/janitor
//! container digests and registry (`oci`/`azureLinux`) base digests. Local inputs live in build
//! stamps, never here.

use std::{collections::BTreeMap, fs, io::ErrorKind, path::Path};

use serde::{Deserialize, Serialize};

use crate::error::CoreError;

/// The current lockfile schema version.
pub const SCHEMA_VERSION: u32 = 1;

/// The parsed `tailor.lock`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Lockfile {
    pub schema_version: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub toolchains: BTreeMap<String, LockedContainer>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools_dirs: BTreeMap<String, LockedContainer>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<LockedRuntime>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bases: Vec<LockedBase>,
}

impl Default for Lockfile {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            toolchains: BTreeMap::new(),
            tools_dirs: BTreeMap::new(),
            runtime: None,
            bases: Vec::new(),
        }
    }
}

/// A digest-pinned container image (toolchain or janitor).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedContainer {
    pub container: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    pub digest: String,
}

/// Runtime-scoped locked images.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LockedRuntime {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub janitor_image: Option<LockedContainer>,
}

/// A digest-pinned registry base image, keyed by reference + platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedBase {
    pub reference: String,
    pub platform: String,
    pub digest: String,
}

impl Lockfile {
    /// The pinned digest of toolchain `id`, if locked.
    pub fn toolchain_digest(&self, id: &str) -> Option<&str> {
        self.toolchains.get(id).map(|c| c.digest.as_str())
    }

    /// The pinned digest of tools-dir source `id`, if locked.
    pub fn tools_dir_digest(&self, id: &str) -> Option<&str> {
        self.tools_dirs.get(id).map(|c| c.digest.as_str())
    }

    /// The pinned digest of a registry base for `reference` + `platform`, if locked.
    pub fn base_digest(&self, reference: &str, platform: &str) -> Option<&str> {
        self.bases
            .iter()
            .find(|b| b.reference == reference && b.platform == platform)
            .map(|b| b.digest.as_str())
    }

    /// Insert/replace a registry base entry, keeping `bases` sorted and deduplicated.
    pub fn upsert_base(&mut self, base: LockedBase) {
        self.bases
            .retain(|b| !(b.reference == base.reference && b.platform == base.platform));
        self.bases.push(base);
        self.bases
            .sort_by(|a, b| (&a.reference, &a.platform).cmp(&(&b.reference, &b.platform)));
    }

    /// Read a lockfile, returning the default (empty) lock if the file does not exist.
    pub fn read(path: &Path) -> Result<Self, CoreError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(source) if source.kind() == ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(CoreError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        serde_yaml_ng::from_str(&text).map_err(|source| CoreError::Serde {
            path: path.to_path_buf(),
            source,
        })
    }

    /// Write the lockfile as YAML.
    pub fn write(&self, path: &Path) -> Result<(), CoreError> {
        let text = serde_yaml_ng::to_string(self).map_err(|source| CoreError::Serde {
            path: path.to_path_buf(),
            source,
        })?;
        fs::write(path, text).map_err(|source| CoreError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn round_trips_through_yaml() {
        let mut lock = Lockfile::default();
        lock.toolchains.insert(
            "ic-1.3".to_owned(),
            LockedContainer {
                container: "mcr.microsoft.com/azurelinux/imagecustomizer".to_owned(),
                version: Some("1.3.0".to_owned()),
                tag: Some("1.3.0".to_owned()),
                digest: "sha256:abcd".to_owned(),
            },
        );
        lock.tools_dirs.insert(
            "acl".to_owned(),
            LockedContainer {
                container: "mcr.microsoft.com/azurelinux/base/core".to_owned(),
                version: None,
                tag: Some("3.0".to_owned()),
                digest: "sha256:tools".to_owned(),
            },
        );
        lock.upsert_base(LockedBase {
            reference: "mcr.microsoft.com/azurelinux/3.0/image/minimal-os".to_owned(),
            platform: "linux/amd64".to_owned(),
            digest: "sha256:9a9a".to_owned(),
        });

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("tailor.lock");
        lock.write(&path).unwrap();
        let reloaded = Lockfile::read(&path).unwrap();

        assert_eq!(reloaded.toolchain_digest("ic-1.3"), Some("sha256:abcd"));
        assert_eq!(reloaded.tools_dir_digest("acl"), Some("sha256:tools"));
        assert_eq!(
            reloaded.base_digest(
                "mcr.microsoft.com/azurelinux/3.0/image/minimal-os",
                "linux/amd64"
            ),
            Some("sha256:9a9a")
        );
    }

    #[test]
    fn missing_file_reads_as_empty() {
        let dir = TempDir::new().unwrap();
        let lock = Lockfile::read(&dir.path().join("absent.lock")).unwrap();
        assert!(lock.toolchains.is_empty() && lock.tools_dirs.is_empty() && lock.bases.is_empty());
    }

    #[test]
    fn upsert_replaces_same_reference_platform() {
        let mut lock = Lockfile::default();
        for digest in ["sha256:one", "sha256:two"] {
            lock.upsert_base(LockedBase {
                reference: "ref".to_owned(),
                platform: "linux/amd64".to_owned(),
                digest: digest.to_owned(),
            });
        }
        assert_eq!(lock.bases.len(), 1);
        assert_eq!(lock.base_digest("ref", "linux/amd64"), Some("sha256:two"));
    }
}
