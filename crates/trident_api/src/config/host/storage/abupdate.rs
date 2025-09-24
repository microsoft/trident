use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::BlockDeviceId;

#[cfg(feature = "schemars")]
use crate::schema_helpers::block_device_id_schema;

/// A/B update configuration. Carries information about the A/B update volume
/// pairs that are used to perform A/B updates.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct AbUpdate {
    /// A list of volume pairs that will be used for A/B Update.
    ///
    /// You can target the A/B Update volume pair from the `images` and
    /// `mount-points` and Trident will pick the right volume to use based on
    /// the A/B Update state of the host.
    pub volume_pairs: Vec<AbVolumePair>,
}

/// Per A/B update volume pair configuration. Points to the underlying block
/// devices used for the A/B update.
///
/// **Under development, initial logic for illustration purposes only.**
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct AbVolumePair {
    /// A unique identifier for the volume pair.
    ///
    /// This is a user defined string that allows to link the volume pair
    /// to the results in the Host Status and to the mount points. The identifier
    /// needs to be unique across all types of devices, not just A/B Volume Pairs.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The ID of the partition that will be used as the A volume.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub volume_a_id: BlockDeviceId,

    /// The ID of the partition that will be used as the B volume.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub volume_b_id: BlockDeviceId,
}
