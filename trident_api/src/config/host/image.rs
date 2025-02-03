use serde::{Deserialize, Serialize};
use url::Url;

// TODO: Enable JsonSchema when this is officially added to the API.
// #[cfg(feature = "schemars")]
// use schemars::JsonSchema;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type", deny_unknown_fields)]
// #[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum OsImage {
    /// Composable OS Image (COSI)
    Cosi(CosiFile),
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
// #[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct CosiFile {
    /// The path to the COSI file.
    pub url: Url,
}

impl OsImage {
    /// Returns the URL of the OsImage file.
    pub fn url(&self) -> &Url {
        match self {
            OsImage::Cosi(cosi) => &cosi.url,
        }
    }
}
