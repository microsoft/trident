use tailor_config::{Arch, AzureLinuxBase, OciBase};
use tailor_core::{ResolveError, ResolvedBase};

use crate::oci;

const AZURE_LINUX_REGISTRY: &str = "mcr.microsoft.com";
const AZURE_LINUX_NAMESPACE: &str = "azurelinux";
const IMAGE_SEGMENT: &str = "image";
const LATEST_TAG: &str = "latest";

pub(crate) async fn resolve(
    azure_linux: &AzureLinuxBase,
    arch: Arch,
) -> Result<ResolvedBase, ResolveError> {
    let uri = reference(azure_linux);
    let oci_base = OciBase {
        uri,
        platform: None,
    };

    oci::resolve(&oci_base, arch).await
}

pub(crate) fn reference(azure_linux: &AzureLinuxBase) -> String {
    let version = &azure_linux.version;
    let variant = &azure_linux.variant;
    format!(
        "{AZURE_LINUX_REGISTRY}/{AZURE_LINUX_NAMESPACE}/{version}/{IMAGE_SEGMENT}/{variant}:{LATEST_TAG}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_mcr_reference() {
        let base = AzureLinuxBase {
            version: "3.0".to_owned(),
            variant: "core".to_owned(),
        };

        assert_eq!(
            reference(&base),
            "mcr.microsoft.com/azurelinux/3.0/image/core:latest"
        );
    }

    #[tokio::test]
    #[ignore = "live registry test; requires network access and MCR image availability"]
    async fn resolves_azure_linux_digest() {
        let base = AzureLinuxBase {
            version: "3.0".to_owned(),
            variant: "core".to_owned(),
        };
        let resolved = resolve(&base, Arch::Amd64).await.unwrap();

        assert!(matches!(
            resolved,
            ResolvedBase::Oci { digest, .. } if digest.starts_with("sha256:")
        ));
    }
}
