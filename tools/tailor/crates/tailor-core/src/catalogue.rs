//! Base-image catalogue orchestration: decide which slots to download, materialise them through the
//! [`BaseImageFetcher`] port, and assert referenced slots are present (`meta/docs/2026-06-29-base-image-catalogue.md`
//! §5, §8). No I/O of its own beyond presence checks — the fetch is the adapter's.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use tailor_config::{Arch, BaseImageCatalogue, BaseImageSlot, BaseImageSource};

use crate::{error::CoreError, ports::BaseImageFetcher};

/// The default slot architecture when a slot declares none — the pull platform is `linux/<arch>`
/// (`meta/docs/2026-06-29-arch-and-platform.md` §3).
const DEFAULT_SLOT_ARCH: Arch = Arch::Amd64;

/// What `tailor bases download` did to one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotOutcome {
    /// Pulled from the slot's source and written to its path.
    Downloaded { source_digest: String, size: u64 },
    /// Skipped: the file is already present (no `--force`).
    Present,
    /// Skipped: the slot has no `source` (default run only — filled out-of-band, e.g. a CI feed).
    NoSource,
}

impl SlotOutcome {
    /// A cargo-style status verb for the slot's result, for `tailor bases download` output.
    pub fn verb(&self) -> &'static str {
        match self {
            SlotOutcome::Downloaded { .. } => "Downloaded",
            SlotOutcome::Present => "Present",
            SlotOutcome::NoSource => "Skipped",
        }
    }
}

/// One slot's `download` result, keyed by name and its absolute path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotReport {
    pub name: String,
    pub path: PathBuf,
    pub outcome: SlotOutcome,
}

/// How a slot is materialised, for `tailor bases list` display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotSource {
    /// Pulled from an OCI registry reference.
    Oci(String),
    /// Pulled from the Azure Linux MCR sugar (`azurelinux/<version>/image/<variant>`).
    AzureLinux { version: String, variant: String },
    /// No `source`: filled out-of-band (e.g. a CI feed); `download` skips it, `verify` checks it.
    OutOfBand,
}

/// One slot's catalogue summary for `tailor bases list`: its resolved fields and on-disk presence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotSummary {
    pub name: String,
    pub path: PathBuf,
    pub arch: Arch,
    pub source: SlotSource,
    pub present: bool,
}

/// Materialise catalogue slots from their `source`. With `names` empty, every slot that has a source
/// and a missing file; otherwise exactly those slots (naming a sourceless slot is an error). `--force`
/// re-pulls present files. Idempotent, so it is safe before every local build (§5).
pub async fn download(
    catalogue: &BaseImageCatalogue,
    root: impl AsRef<Path>,
    fetcher: &impl BaseImageFetcher,
    names: &[String],
    force: bool,
) -> Result<Vec<SlotReport>, CoreError> {
    let root = root.as_ref();
    let selected = select_slots(catalogue, names)?;
    let mut reports = Vec::new();
    for slot in selected {
        let path = tailor_config::absolutize(&slot.path, root);
        let outcome = match &slot.source {
            None => SlotOutcome::NoSource,
            Some(_) if path.exists() && !force => SlotOutcome::Present,
            Some(source) => {
                let arch = slot.arch.unwrap_or(DEFAULT_SLOT_ARCH);
                let pulled = fetcher.fetch(source, arch, &path).await?;
                SlotOutcome::Downloaded {
                    source_digest: pulled.source_digest,
                    size: pulled.size,
                }
            }
        };
        reports.push(SlotReport {
            name: slot.name.clone(),
            path,
            outcome,
        });
    }
    Ok(reports)
}

/// Assert every named slot's file exists on disk, returning the missing names+paths. Empty `names`
/// checks the whole catalogue. This is `tailor bases verify`'s presence gate (§5).
pub fn verify(
    catalogue: &BaseImageCatalogue,
    root: impl AsRef<Path>,
    names: &BTreeSet<String>,
) -> Result<(), CoreError> {
    let root = root.as_ref();
    for slot in catalogue.iter() {
        if !names.is_empty() && !names.contains(&slot.name) {
            continue;
        }
        let path = tailor_config::absolutize(&slot.path, root);
        if !path.exists() {
            return Err(CoreError::BaseImageMissing {
                name: slot.name.clone(),
                path,
            });
        }
    }
    Ok(())
}

/// Summarize every catalogue slot (in catalogue order) for `tailor bases list`: the resolved pull arch
/// (default amd64), the source kind, and whether the slot file exists on disk.
pub fn summarize(catalogue: &BaseImageCatalogue, root: impl AsRef<Path>) -> Vec<SlotSummary> {
    let root = root.as_ref();
    catalogue
        .iter()
        .map(|slot| {
            let path = tailor_config::absolutize(&slot.path, root);
            let present = path.exists();
            let source = match &slot.source {
                None => SlotSource::OutOfBand,
                Some(BaseImageSource::Oci { oci }) => SlotSource::Oci(oci.uri.clone()),
                Some(BaseImageSource::AzureLinux { azure_linux }) => SlotSource::AzureLinux {
                    version: azure_linux.version.clone(),
                    variant: azure_linux.variant.clone(),
                },
            };
            SlotSummary {
                name: slot.name.clone(),
                path,
                arch: slot.arch.unwrap_or(DEFAULT_SLOT_ARCH),
                source,
                present,
            }
        })
        .collect()
}

/// The slots `download` should touch: every sourced slot when `names` is empty, else exactly the
/// named ones — an unknown name or a sourceless named slot is an error (§5).
fn select_slots<'a>(
    catalogue: &'a BaseImageCatalogue,
    names: &[String],
) -> Result<Vec<&'a BaseImageSlot>, CoreError> {
    if names.is_empty() {
        return Ok(catalogue
            .iter()
            .filter(|slot| slot.source.is_some())
            .collect());
    }
    let mut selected = Vec::new();
    for name in names {
        let slot = catalogue
            .get(name)
            .ok_or_else(|| CoreError::UnknownBaseImage {
                image: "image".to_owned(),
                name: name.clone(),
                known: catalogue
                    .iter()
                    .map(|slot| slot.name.clone())
                    .collect::<Vec<_>>()
                    .join(", "),
            })?;
        if slot.source.is_none() {
            return Err(CoreError::BaseImageMissing {
                name: name.clone(),
                path: slot.path.clone(),
            });
        }
        selected.push(slot);
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs,
        path::{Path, PathBuf},
        sync::Mutex,
    };

    use tempfile::TempDir;

    use tailor_config::{Arch, BaseImageSlot, BaseImageSource, OciBase};

    use crate::error::ResolveError;
    use crate::ports::FetchedBase;

    /// A fetcher that writes a stub file and records the destinations it filled.
    #[derive(Default)]
    struct StubFetcher {
        filled: Mutex<Vec<PathBuf>>,
    }

    impl BaseImageFetcher for StubFetcher {
        async fn fetch(
            &self,
            _source: &BaseImageSource,
            _arch: Arch,
            dest: &Path,
        ) -> Result<FetchedBase, ResolveError> {
            fs::write(dest, b"stub").unwrap();
            self.filled.lock().unwrap().push(dest.to_path_buf());
            Ok(FetchedBase {
                source_digest: "sha256:stub".to_owned(),
                sha256: [0; 32],
                size: 4,
            })
        }
    }

    fn sourced_slot(name: &str, path: &str) -> BaseImageSlot {
        BaseImageSlot {
            name: name.to_owned(),
            path: path.into(),
            arch: Some(Arch::Amd64),
            source: Some(BaseImageSource::Oci {
                oci: OciBase {
                    uri: "mcr.example/base:latest".to_owned(),
                    platform: None,
                },
            }),
        }
    }

    fn feed_slot(name: &str, path: &str) -> BaseImageSlot {
        BaseImageSlot {
            name: name.to_owned(),
            path: path.into(),
            arch: Some(Arch::Amd64),
            source: None,
        }
    }

    fn catalogue() -> BaseImageCatalogue {
        BaseImageCatalogue::from(vec![
            sourced_slot("baremetal", "baremetal.vhdx"),
            feed_slot("qemu", "qemu.vhdx"),
        ])
    }

    #[tokio::test]
    async fn download_default_pulls_only_sourced_missing_slots() {
        let dir = TempDir::new().unwrap();
        let reports = download(
            &catalogue(),
            dir.path(),
            &StubFetcher::default(),
            &[],
            false,
        )
        .await
        .unwrap();
        let baremetal = reports.iter().find(|r| r.name == "baremetal").unwrap();
        assert!(matches!(baremetal.outcome, SlotOutcome::Downloaded { .. }));
        assert!(dir.path().join("baremetal.vhdx").exists());
        // The feed-only slot has no source, so it is absent from a default run entirely.
        assert!(!reports.iter().any(|r| r.name == "qemu"));
    }

    #[tokio::test]
    async fn download_skips_present_unless_forced() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("baremetal.vhdx"), b"old").unwrap();
        let reports = download(
            &catalogue(),
            dir.path(),
            &StubFetcher::default(),
            &["baremetal".to_owned()],
            false,
        )
        .await
        .unwrap();
        assert_eq!(reports[0].outcome, SlotOutcome::Present);
        let forced = download(
            &catalogue(),
            dir.path(),
            &StubFetcher::default(),
            &["baremetal".to_owned()],
            true,
        )
        .await
        .unwrap();
        assert!(matches!(forced[0].outcome, SlotOutcome::Downloaded { .. }));
    }

    #[tokio::test]
    async fn naming_a_sourceless_slot_errors() {
        let dir = TempDir::new().unwrap();
        let err = download(
            &catalogue(),
            dir.path(),
            &StubFetcher::default(),
            &["qemu".to_owned()],
            false,
        )
        .await
        .unwrap_err();
        assert!(
            matches!(err, CoreError::BaseImageMissing { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn verify_fails_on_missing_referenced_slot() {
        let dir = TempDir::new().unwrap();
        let names = BTreeSet::from(["baremetal".to_owned()]);
        let err = verify(&catalogue(), dir.path(), &names).unwrap_err();
        assert!(
            matches!(err, CoreError::BaseImageMissing { .. }),
            "got {err:?}"
        );
        fs::write(dir.path().join("baremetal.vhdx"), b"x").unwrap();
        verify(&catalogue(), dir.path(), &names).unwrap();
    }

    #[test]
    fn summarize_reports_arch_source_and_presence() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("baremetal.vhdx"), b"x").unwrap();
        let summary = summarize(&catalogue(), dir.path());
        assert_eq!(summary.len(), 2);
        let baremetal = &summary[0];
        assert_eq!(baremetal.name, "baremetal");
        assert_eq!(baremetal.arch, Arch::Amd64);
        assert!(baremetal.present);
        assert!(
            matches!(baremetal.source, SlotSource::Oci(_)),
            "got {:?}",
            baremetal.source
        );
        let qemu = &summary[1];
        assert_eq!(qemu.name, "qemu");
        assert!(!qemu.present);
        assert_eq!(qemu.source, SlotSource::OutOfBand);
    }
}
