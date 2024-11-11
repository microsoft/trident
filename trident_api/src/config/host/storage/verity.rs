#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use serde::{Deserialize, Serialize};

use crate::BlockDeviceId;

/// Verity device configuration.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct VerityDevice {
    /// Block device id of the verity device.
    pub id: BlockDeviceId,

    /// Name of the verity device, used for the device mapper name.
    ///
    /// The value must be "root" for root partition "/".
    pub name: String,

    /// The ID of the partition to use as the verity data partition.
    pub data_device_id: BlockDeviceId,

    /// The ID of the partition to use as the verity hash partition.
    pub hash_device_id: BlockDeviceId,

    // Specifies how a mismatch between the hash and the data partition is handled.
    #[serde(default)]
    pub corruption_option: VerityCorruptionOption,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
/// Corruption option for verity.
pub enum VerityCorruptionOption {
    /// # IO-Error
    ///
    /// Fails the I/O operation with an I/O error.
    #[default]
    IoError,

    /// # Ignore
    ///
    /// Ignores the corruption and continues operation.
    Ignore,

    /// # Panic
    ///
    /// Causes the system to panic (print errors) and then try restarting.
    Panic,

    /// # Restart
    ///
    /// Attempts to restart the system.
    Restart,
}
