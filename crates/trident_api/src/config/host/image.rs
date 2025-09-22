#[cfg(feature = "schemars")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::primitives::hash::Sha384Hash;
#[cfg(feature = "schemars")]
use crate::schema_helpers::unit_enum_with_untagged_variant;

/// Data about the image to deploy on the host, including sourcing and integrity information.
///
/// Currently, the only format supported by Trident is Composable OS Image (COSI). COSI files can be generated with PRISM.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct OsImage {
    /// The path to the COSI file.
    ///
    /// URLs may have one of the following four schemes: `http://`, `https://`, `file://`, or
    /// `oci://`. COSI files stored as an OCI image must allow for anonymous pulls.
    pub url: Url,

    /// The Sha384 of the entire COSI file.
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "unit_enum_with_untagged_variant::<ImageSha384, Sha384Hash>")
    )]
    pub sha384: ImageSha384,
}

/// Image SHA384 checksum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum ImageSha384 {
    /// # Ignored
    ///
    /// You can pass `ignored` to skip the checksum verification.
    Ignored,

    /// # Checksum
    ///
    /// The SHA384 checksum of the image.
    #[serde(untagged)]
    Checksum(Sha384Hash),
}

impl std::fmt::Display for ImageSha384 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageSha384::Ignored => write!(f, "ignored"),
            ImageSha384::Checksum(hash) => write!(f, "{hash}"),
        }
    }
}
