//! Base-image acquisition adapter: pull a catalogue slot's artifact for a platform and write the
//! image file to its path (`meta/docs/2026-06-29-base-image-catalogue.md` §5.2). Reuses `oci_client` (already a
//! dep for digest resolution) to stay self-contained — no shelling out to `oras`.
//!
//! The Azure Linux image artifact is an OCI image index; its per-`linux/<arch>` manifest carries the
//! disk image as a **raw** layer (title-annotated `*.vhdx`, no tar/compression — the layer bytes are
//! the file), plus SPDX/signature metadata layers. We pick the image layer and stream it to the slot
//! path, so the written file's hash is exactly the layer digest.

use std::path::Path;

use oci_client::{manifest::OciDescriptor, secrets::RegistryAuth};
use tokio::{
    fs::File,
    io::{AsyncWriteExt, BufWriter},
};

use tailor_config::{Arch, BaseImageSource};
use tailor_core::{BaseImageFetcher, FetchedBase, ResolveError};

use crate::{azure_linux, oci};

const TITLE_ANNOTATION: &str = "org.opencontainers.image.title";
/// Disk-image extensions tailor recognises as a slot's payload layer (the rest of an Azure Linux image
/// artifact is SPDX/signature metadata).
const IMAGE_EXTENSIONS: [&str; 5] = ["vhdx", "vhd", "qcow2", "raw", "img"];

/// Pulls catalogue slot files via `oci_client`. Stateless; the seam every base-image `download` uses.
#[derive(Debug, Default, Clone, Copy)]
pub struct OciFetcher;

impl OciFetcher {
    pub fn new() -> Self {
        Self
    }
}

impl BaseImageFetcher for OciFetcher {
    async fn fetch(
        &self,
        source: &BaseImageSource,
        arch: Arch,
        dest: &Path,
    ) -> Result<FetchedBase, ResolveError> {
        let (reference, platform) = source_reference(source, arch);
        let image = oci::parse_reference(&reference)?;
        // The platform-aware client resolves an image index to the `linux/<arch>` manifest.
        let client = oci::platform_client(&platform)?;
        let (manifest, source_digest) = client
            .pull_image_manifest(&image, &RegistryAuth::Anonymous)
            .await
            .map_err(|err| registry_error(&reference, err.to_string()))?;

        let layer = select_image_layer(&manifest.layers, dest)
            .ok_or_else(|| registry_error(&reference, "manifest has no image layer".to_owned()))?;

        let file = File::create(dest)
            .await
            .map_err(|source| ResolveError::LocalRead {
                path: dest.to_path_buf(),
                source,
            })?;
        let mut writer = BufWriter::new(file);
        // `pull_blob` streams the layer and verifies it against `layer.digest`; the raw layer bytes are
        // the disk image, so they land at `dest` directly.
        client
            .pull_blob(&image, layer, &mut writer)
            .await
            .map_err(|err| registry_error(&reference, err.to_string()))?;
        writer
            .flush()
            .await
            .map_err(|source| ResolveError::LocalRead {
                path: dest.to_path_buf(),
                source,
            })?;

        let sha256 = parse_sha256(&layer.digest).ok_or_else(|| {
            registry_error(
                &reference,
                format!("unexpected layer digest `{}`", layer.digest),
            )
        })?;
        Ok(FetchedBase {
            source_digest,
            sha256,
            size: u64::try_from(layer.size).unwrap_or(0),
        })
    }
}

/// The registry reference + `linux/<arch>` platform for a slot source.
fn source_reference(source: &BaseImageSource, arch: Arch) -> (String, String) {
    let platform = oci::platform_for_arch(arch);
    let reference = match source {
        BaseImageSource::Oci { oci } => oci.uri.clone(),
        BaseImageSource::AzureLinux { azure_linux } => azure_linux::reference(azure_linux),
    };
    (reference, platform)
}

/// Pick the slot's payload layer: prefer a layer whose title matches `dest`'s extension, else any
/// disk-image layer, else the largest layer (the disk image dwarfs the SPDX/signature blobs).
fn select_image_layer<'a>(layers: &'a [OciDescriptor], dest: &Path) -> Option<&'a OciDescriptor> {
    let title = |layer: &OciDescriptor| {
        layer
            .annotations
            .as_ref()
            .and_then(|a| a.get(TITLE_ANNOTATION))
            .cloned()
    };
    let has_ext = |layer: &OciDescriptor, ext: &str| {
        title(layer).is_some_and(|t| t.ends_with(&format!(".{ext}")))
    };

    let by_ext = dest
        .extension()
        .and_then(|e| e.to_str())
        .and_then(|ext| layers.iter().find(|layer| has_ext(layer, ext)));
    if let Some(layer) = by_ext {
        return Some(layer);
    }
    layers
        .iter()
        .filter(|layer| IMAGE_EXTENSIONS.iter().any(|ext| has_ext(layer, ext)))
        .max_by_key(|layer| layer.size)
        .or_else(|| layers.iter().max_by_key(|layer| layer.size))
}

fn parse_sha256(digest: &str) -> Option<[u8; 32]> {
    let hex = digest.strip_prefix("sha256:")?;
    hex::decode(hex).ok()?.try_into().ok()
}

fn registry_error(reference: &str, detail: String) -> ResolveError {
    ResolveError::Registry {
        reference: reference.to_owned(),
        detail,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{collections::BTreeMap, fs};

    use tailor_config::{AzureLinuxBase, OciBase};

    fn layer(title: Option<&str>, size: i64) -> OciDescriptor {
        OciDescriptor {
            media_type: "application/vnd.oci.image.layer.v1.tar".to_owned(),
            digest: format!("sha256:{size:064x}"),
            size,
            urls: None,
            annotations: title.map(|t| {
                let mut a = BTreeMap::new();
                a.insert(TITLE_ANNOTATION.to_owned(), t.to_owned());
                a
            }),
        }
    }

    #[test]
    fn selects_the_layer_matching_the_slot_extension() {
        let layers = [
            layer(Some("image.vhdx"), 742_000_000),
            layer(Some("image.vhdx.spdx.json"), 142_000),
            layer(Some("image.vhdx.spdx.json.sig"), 10_000),
        ];
        let picked = select_image_layer(&layers, Path::new("bases/baremetal.vhdx")).unwrap();
        assert_eq!(picked.size, 742_000_000);
    }

    #[test]
    fn falls_back_to_the_largest_layer_without_titles() {
        let layers = [layer(None, 10), layer(None, 999), layer(None, 50)];
        let picked = select_image_layer(&layers, Path::new("x.vhdx")).unwrap();
        assert_eq!(picked.size, 999);
    }

    #[test]
    fn parses_a_sha256_digest_and_rejects_others() {
        let digest = format!("sha256:{}", "ab".repeat(32));
        assert_eq!(parse_sha256(&digest).unwrap(), [0xab; 32]);
        assert!(parse_sha256("sha512:dead").is_none());
        assert!(parse_sha256("sha256:zz").is_none());
    }

    #[test]
    fn azure_linux_source_resolves_to_platform_and_reference() {
        let source = BaseImageSource::AzureLinux {
            azure_linux: AzureLinuxBase {
                version: "3.0".to_owned(),
                variant: "core".to_owned(),
            },
        };
        let (reference, platform) = source_reference(&source, Arch::Arm64);
        assert_eq!(platform, "linux/arm64");
        assert!(reference.contains("azurelinux/3.0/image/core"));
    }

    #[tokio::test]
    #[ignore = "live registry test; pulls a small public artifact's layer over the network"]
    async fn fetches_a_public_blob_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("layer.bin");
        let source = BaseImageSource::Oci {
            oci: OciBase {
                uri: "docker.io/library/busybox:latest".to_owned(),
                platform: None,
            },
        };
        let fetched = OciFetcher::new()
            .fetch(&source, Arch::Amd64, &dest)
            .await
            .unwrap();
        assert!(dest.exists());
        assert_eq!(fetched.size, fs::metadata(&dest).unwrap().len());
    }
}
