//! Per-cell build stamps (`meta/docs/2026-06-22-design.md` §9.2, §12). Each artifact has a sidecar JSON at
//! `<output-dir>/.tailor/stamps/<cell-slug>.json` recording the canonical fingerprint, so rebuild
//! decisions compare against the last *built* inputs (immune to a just-refreshed lock).

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{atomic, domain::Fingerprint, error::CoreError};

const STAMP_DIR: &str = ".tailor/stamps";

/// The recorded fingerprint of a previously built cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStamp {
    pub slug: String,
    pub fingerprint: String,
    pub tailor_version: String,
}

/// The stamp path for a cell slug under `output_dir`.
pub fn stamp_path(output_dir: &Path, slug: &str) -> PathBuf {
    output_dir.join(STAMP_DIR).join(format!("{slug}.json"))
}

/// Read a cell's stamp, returning `None` if absent or unparseable (treated as "rebuild").
pub fn read(output_dir: &Path, slug: &str) -> Option<BuildStamp> {
    let text = fs::read_to_string(stamp_path(output_dir, slug)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write a cell's stamp, creating the stamp directory if needed.
pub fn write(output_dir: &Path, slug: &str, fingerprint: Fingerprint) -> Result<(), CoreError> {
    let path = stamp_path(output_dir, slug);
    let stamp = BuildStamp {
        slug: slug.to_owned(),
        fingerprint: fingerprint.to_hex(),
        tailor_version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    let text = serde_json::to_string_pretty(&stamp).unwrap_or_default();
    // The stamp is written last, after the artifact is fully published, and atomically — so a
    // reader (a later run's up-to-date check) sees a matching stamp only when the artifact is
    // complete, never a torn stamp left by an interrupted build.
    atomic::write(&path, text.as_bytes()).map_err(|source| CoreError::Io { path, source })
}

/// Whether a cell is up to date: its artifact exists and the stamp records the same fingerprint.
pub fn is_up_to_date(
    output_dir: &Path,
    slug: &str,
    fingerprint: Fingerprint,
    artifact: &Path,
) -> bool {
    artifact.exists()
        && read(output_dir, slug).is_some_and(|stamp| stamp.fingerprint == fingerprint.to_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn write_then_read_round_trips() {
        let dir = TempDir::new().unwrap();
        let fp = Fingerprint([7; 32]);
        write(dir.path(), "img_grub_amd64_3.0_base_cosi", fp).unwrap();
        let stamp = read(dir.path(), "img_grub_amd64_3.0_base_cosi").unwrap();
        assert_eq!(stamp.fingerprint, fp.to_hex());
    }

    #[test]
    fn up_to_date_requires_matching_fingerprint_and_artifact() {
        let dir = TempDir::new().unwrap();
        let artifact = dir.path().join("out.cosi");
        fs::write(&artifact, b"x").unwrap();
        let fp = Fingerprint([1; 32]);
        write(dir.path(), "cell", fp).unwrap();

        assert!(is_up_to_date(dir.path(), "cell", fp, &artifact));
        assert!(!is_up_to_date(
            dir.path(),
            "cell",
            Fingerprint([2; 32]),
            &artifact
        ));
        assert!(!is_up_to_date(
            dir.path(),
            "cell",
            fp,
            &dir.path().join("missing.cosi")
        ));
    }
}
