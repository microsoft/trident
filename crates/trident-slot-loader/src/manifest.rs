//! Versioned boot input, independent of the servicing database and runtime publication.
//!
//! Serde rejects duplicate known keys. Unknown fields deliberately remain forward-compatible.

use std::{collections::HashSet, path::Path};

use anyhow::{ensure, Context, Error};
use serde::Deserialize;

use crate::filesystem;

const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;
const MAX_NAME_BYTES: usize = 251;

/// Manifest order is retained: earlier confexts have higher overlay precedence.
#[derive(Debug, Deserialize)]
pub(super) struct Manifest {
    version: u64,
    #[serde(default)]
    pub(super) sysexts: Vec<Extension>,
    #[serde(default)]
    pub(super) confexts: Vec<Extension>,
}

#[derive(Debug, Deserialize)]
pub(super) struct Extension {
    pub(super) name: String,
}

impl Manifest {
    pub(super) fn read(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let data = filesystem::read_bounded(path, MAX_MANIFEST_BYTES)?;
        Self::parse(&data).with_context(|| format!("manifest {}", path.display()))
    }

    fn parse(data: &str) -> Result<Self, Error> {
        let manifest: Self = serde_json::from_str(data)?;
        ensure!(
            manifest.version == 1,
            "unsupported manifest version {}",
            manifest.version
        );
        for extensions in [&manifest.sysexts, &manifest.confexts] {
            let mut names = HashSet::new();
            for entry in extensions {
                ensure!(
                    valid_name(&entry.name),
                    "invalid image name {:?}",
                    entry.name
                );
                ensure!(
                    names.insert(&entry.name),
                    "duplicate image name {:?}",
                    entry.name
                );
            }
        }
        Ok(manifest)
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && !matches!(name, "." | "..")
        && !name.starts_with(".#")
        && !name
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\' | ':' | ','))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_unknown_fields_are_supported() {
        let manifest = Manifest::parse(r#"{"version":1,"future":true}"#).unwrap();
        assert!(manifest.sysexts.is_empty() && manifest.confexts.is_empty());
        Manifest::parse(
            r#"{"version":1,"sysexts":[{"name":"runtime","future":1}],"confexts":[{"name":"runtime"}]}"#,
        ).unwrap();
    }

    #[test]
    fn malformed_or_ambiguous_manifests_fail() {
        for invalid in [
            r#"{}"#,
            r#"{"version":2}"#,
            r#"{"version":1,"version":1}"#,
            r#"{"version":1,"sysexts":null}"#,
            r#"{"version":1,"sysexts":[{}]}"#,
            r#"{"version":1,"sysexts":[{"name":"a","name":"b"}]}"#,
            r#"{"version":1,"sysexts":[{"name":"a"},{"name":"a","future":true}]}"#,
            r#"{"version":1} trailing"#,
        ] {
            Manifest::parse(invalid).unwrap_err();
        }
        for invalid in [
            "",
            ".",
            "..",
            ".#hidden",
            "../escape",
            "a/b",
            "a\\b",
            "a:b",
            "a,b",
            "a\nb",
        ] {
            assert!(!valid_name(invalid), "{invalid:?}");
        }
    }
}
