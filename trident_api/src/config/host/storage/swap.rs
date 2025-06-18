use std::str::FromStr;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::BlockDeviceId;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Swap {
    /// The ID of the block device to use for this swap area.
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "crate::schema_helpers::block_device_id_schema")
    )]
    pub device_id: BlockDeviceId,
}

impl FromStr for Swap {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Swap {
            device_id: s.to_owned(),
        })
    }
}

#[cfg(feature = "schemars")]
impl crate::primitives::shortcuts::StringOrStructMetadata for Swap {
    fn shorthand_format() -> &'static str {
        crate::schema_helpers::BLOCK_DEVICE_ID_FORMAT
    }
}
