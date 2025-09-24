use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::BlockDeviceId;

#[cfg(feature = "schemars")]
use crate::schema_helpers::block_device_id_schema;

/// A/B update configuration. Carries information about the A/B volume pairs that are used to
/// perform A/B updates.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct AbUpdate {
    /// A list of A/B volume pairs that will be used for A/B update.
    pub volume_pairs: Vec<AbVolumePair>,
}

/// Per A/B volume pair configuration. Points to the underlying block devices in the A/B volume
/// volume pair.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct AbVolumePair {
    /// A unique identifier for the A/B volume pair.
    ///
    /// This is a user-defined string that links the A/B volume pair to the results in the Host
    /// Status and to the `filesystems` config. The identifier needs to be unique across devices of
    /// all types, not just A/B volume pairs.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The id of the device that will be used as the A volume.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub volume_a_id: BlockDeviceId,

    /// The id of the device that will be used as the B volume.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub volume_b_id: BlockDeviceId,
}
