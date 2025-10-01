use std::path::PathBuf;

#[cfg(feature = "schemars")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::primitives::hash::Sha384Hash;

/// Data about an extension image (sysext or confext) to merge onto the target OS.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Extension {
    /// The path to the extension image file.
    ///
    /// URLs may have one of the following four schemes: `http://`, `https://`, `file://`, or
    /// `oci://`. Extension image files stored in OCI registries must allow for
    /// anonymous pulls.
    pub url: Url,

    /// The Sha384 of the entire extension image file.
    pub sha384: Sha384Hash,

    /// The path of the extension image in the target OS.
    pub location: Option<PathBuf>,
}
