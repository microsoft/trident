use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::BlockDeviceId;

#[cfg(feature = "schemars")]
use crate::schema_helpers::block_device_id_schema;

/// Per image configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Image {
    /// The URL of the image.
    ///
    /// Supported schemes are: `file`, `http`, and `https`.
    pub url: String,

    /// The SHA256 checksum of the compressed image.
    ///
    /// The hash is computed over the compressed contents of the image, not the uncompressed output
    /// that will be written to the block device. This value is used to verify the integrity of the
    /// image.
    ///
    /// Accepted values:
    ///
    /// - 64-character hexadecimal string (case insensitive)
    ///
    /// - `ignored` to skip the checksum verification
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub sha256: ImageSha256,

    /// The format of the image.
    pub format: ImageFormat,

    /// The ID of the partition that will be used to store the image.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub target_id: BlockDeviceId,
}

/// Image SHA256 checksum.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum ImageSha256 {
    /// The SHA256 checksum of the image.
    ///
    /// This is used to verify the integrity of the image.
    /// The checksum is a 64 character hexadecimal string.
    Checksum(String),

    /// You can pass `ignored` to skip the checksum verification.
    Ignored,
}

/// Image format.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum ImageFormat {
    /// # Raw Zstd Compressed
    ///
    /// Raw filesystem image with zstd compression.
    RawZst,

    /// # Raw Lzma Compressed
    ///
    /// Raw filesystem image with lzma compression, required by
    /// systemd-sysupdate.
    #[cfg(feature = "sysupdate")]
    RawLzma,
}

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
    /// needs to be unique across all types of devices, not just AB Volume Pairs.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The ID of the partition that will be used as the A volume.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub volume_a_id: BlockDeviceId,

    /// The ID of the partition that will be used as the B volume.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub volume_b_id: BlockDeviceId,
}
