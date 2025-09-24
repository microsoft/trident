use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Services {
    /// List of services to enable.
    ///
    /// The services listed here will be set to automatically run on OS boot.
    #[serde(default)]
    pub enable: Vec<String>,

    /// List of services to disable.
    ///
    /// The services listed here will *not* be set to automatically run on OS boot.
    #[serde(default)]
    pub disable: Vec<String>,
}
