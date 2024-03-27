#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use serde::{Deserialize, Serialize};

use crate::BlockDeviceId;

/// Verity configuration for a volume.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct VerityDevice {
    /// Block device id of the verity device
    pub id: BlockDeviceId,

    /// Name of the verity device, used for the device mapper name
    pub device_name: String,

    /// Block device id of the data block device
    pub data_target_id: BlockDeviceId,

    /// Block device id of the hash block device
    pub hash_target_id: BlockDeviceId,
}
