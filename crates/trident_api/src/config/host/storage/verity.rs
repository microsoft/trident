use std::path::{Path, PathBuf};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use serde::{Deserialize, Serialize};

use crate::{constants::DEV_MAPPER_PATH, BlockDeviceId};

#[cfg(feature = "schemars")]
use crate::schema::block_device_id_schema;

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
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub data_device_id: BlockDeviceId,

    /// The ID of the partition to use as the verity hash partition.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub hash_device_id: BlockDeviceId,

    /// The ID of the partition holding the dm-verity root hash signature, if any.
    ///
    /// When set, Trident reads the PKCS#7/DER root hash signature directly
    /// from this partition and uses it to open the verity device with
    /// `veritysetup open --root-hash-signature=...`, enabling kernel-enforced
    /// signature verification of the verity root hash. The certificate
    /// matching the signature must exist in the kernel keyring.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub hash_signature_device_id: Option<BlockDeviceId>,

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

impl VerityDevice {
    /// Returns the path where this verity device will be mounted at runtime.
    pub fn device_path(&self) -> PathBuf {
        Path::new(DEV_MAPPER_PATH).join(&self.name)
    }

    /// Returns the path where this verity device will be mounted while staging an update.
    ///
    /// This path must be different from where the device will be mounted at runtime because the
    /// verity device_name is shared between the A and B devices.
    pub fn temporary_device_path(&self) -> PathBuf {
        Path::new(DEV_MAPPER_PATH).join(format!("{}_new", self.name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_verity_device() -> VerityDevice {
        VerityDevice {
            id: "verity".into(),
            name: "root".into(),
            data_device_id: "data".into(),
            hash_device_id: "hash".into(),
            hash_signature_device_id: None,
            corruption_option: VerityCorruptionOption::default(),
        }
    }

    #[test]
    fn test_verity_device_serde_roundtrip_without_signature() {
        let device = base_verity_device();

        let serialized = serde_json::to_string(&device).unwrap();
        assert!(!serialized.contains("hashSignatureDeviceId"));

        let deserialized: VerityDevice = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, device);
        assert_eq!(deserialized.hash_signature_device_id, None);
    }

    #[test]
    fn test_verity_device_serde_roundtrip_with_signature() {
        let mut device = base_verity_device();
        device.hash_signature_device_id = Some("hash-signature".into());

        let serialized = serde_json::to_string(&device).unwrap();
        assert!(serialized.contains("\"hashSignatureDeviceId\":\"hash-signature\""));

        let deserialized: VerityDevice = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized, device);
        assert_eq!(
            deserialized.hash_signature_device_id,
            Some("hash-signature".into())
        );
    }

    #[test]
    fn test_verity_device_deserialize_signature_from_json() {
        let json = serde_json::json!({
            "id": "verity",
            "name": "root",
            "dataDeviceId": "data",
            "hashDeviceId": "hash",
            "hashSignatureDeviceId": "hash-signature",
        });

        let device: VerityDevice = serde_json::from_value(json).unwrap();
        assert_eq!(
            device.hash_signature_device_id,
            Some("hash-signature".into())
        );
    }
}
