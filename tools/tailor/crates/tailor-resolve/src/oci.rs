use oci_client::{
    Client, ParseError, Reference, client::ClientConfig, manifest::ImageIndexEntry,
    secrets::RegistryAuth,
};

use tailor_config::{Arch, OciBase};
use tailor_core::{ResolveError, ResolvedBase};

const LINUX_OS: &str = "linux";
const PLATFORM_SEPARATOR: char = '/';

pub(crate) async fn resolve(oci: &OciBase, arch: Arch) -> Result<ResolvedBase, ResolveError> {
    let platform = oci
        .platform
        .clone()
        .unwrap_or_else(|| platform_for_arch(arch));
    let digest = resolve_digest(&oci.uri, &platform).await?;

    Ok(ResolvedBase::Oci {
        reference: oci.uri.clone(),
        platform,
        digest,
    })
}

pub(crate) async fn resolve_digest(
    reference: impl AsRef<str>,
    platform: impl AsRef<str>,
) -> Result<String, ResolveError> {
    let reference = reference.as_ref();
    let platform = platform.as_ref();
    let image = parse_reference(reference)?;
    let client = platform_client(platform)?;
    let (_manifest, digest) = client
        .pull_image_manifest(&image, &RegistryAuth::Anonymous)
        .await
        .map_err(|err| ResolveError::Registry {
            reference: reference.to_owned(),
            detail: err.to_string(),
        })?;

    Ok(digest)
}

pub(crate) async fn resolve_reference_digest(
    reference: impl AsRef<str>,
) -> Result<String, ResolveError> {
    let reference = reference.as_ref();
    let image = parse_reference(reference)?;
    let client = Client::new(ClientConfig::default());

    client
        .fetch_manifest_digest(&image, &RegistryAuth::Anonymous)
        .await
        .map_err(|err| ResolveError::Registry {
            reference: reference.to_owned(),
            detail: err.to_string(),
        })
}

pub(crate) fn platform_for_arch(arch: Arch) -> String {
    format!("{LINUX_OS}/{arch}")
}

pub(crate) fn parse_reference(reference: &str) -> Result<Reference, ResolveError> {
    reference
        .parse()
        .map_err(|err: ParseError| ResolveError::Registry {
            reference: reference.to_owned(),
            detail: err.to_string(),
        })
}

pub(crate) fn platform_client(platform: &str) -> Result<Client, ResolveError> {
    let (os, architecture) = parse_platform(platform)?;
    let config = ClientConfig {
        platform_resolver: Some(Box::new(move |manifests| {
            resolve_manifest_for_platform(manifests, &os, &architecture)
        })),
        ..ClientConfig::default()
    };

    Ok(Client::new(config))
}

fn parse_platform(platform: &str) -> Result<(String, String), ResolveError> {
    let mut parts = platform.split(PLATFORM_SEPARATOR);
    match (parts.next(), parts.next(), parts.next()) {
        (Some(os), Some(architecture), None) if !os.is_empty() && !architecture.is_empty() => {
            Ok((os.to_owned(), architecture.to_owned()))
        }
        _ => Err(ResolveError::Registry {
            reference: platform.to_owned(),
            detail: "platform must have form os/architecture".to_owned(),
        }),
    }
}

fn resolve_manifest_for_platform(
    manifests: &[ImageIndexEntry],
    os: &str,
    architecture: &str,
) -> Option<String> {
    manifests
        .iter()
        .find(|entry| {
            entry
                .platform
                .as_ref()
                .is_some_and(|platform| platform.os == os && platform.architecture == architecture)
        })
        .map(|entry| entry.digest.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_linux_platform_from_arch() {
        assert_eq!(platform_for_arch(Arch::Amd64), "linux/amd64");
        assert_eq!(platform_for_arch(Arch::Arm64), "linux/arm64");
    }

    #[test]
    fn rejects_malformed_platforms() {
        let Err(err) = platform_client("linux") else {
            unreachable!("malformed platform should fail");
        };

        assert!(matches!(err, ResolveError::Registry { .. }), "got {err:?}");
    }

    #[tokio::test]
    #[ignore = "live registry test; requires network access and public registry availability"]
    async fn resolves_public_image_digest() {
        let digest = resolve_digest("busybox:latest", "linux/amd64")
            .await
            .unwrap();

        assert!(digest.starts_with("sha256:"));
    }
}
