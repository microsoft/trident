use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{primitives::hash::Sha256Hash, BlockDeviceId};

#[cfg(feature = "schemars")]
use crate::schema_helpers::{block_device_id_schema, unit_enum_with_untagged_variant};

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
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "unit_enum_with_untagged_variant::<ImageSha256, Sha256Hash>")
    )]
    pub sha256: ImageSha256,

    /// The format of the image.
    pub format: ImageFormat,
}

/// Image SHA256 checksum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum ImageSha256 {
    /// # Ignored
    ///
    /// You can pass `ignored` to skip the checksum verification.
    Ignored,

    /// # Checksum
    ///
    /// The SHA256 checksum of the image.
    #[serde(untagged)]
    Checksum(Sha256Hash),
}

impl std::fmt::Display for ImageSha256 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImageSha256::Ignored => write!(f, "ignored"),
            ImageSha256::Checksum(hash) => write!(f, "{}", hash),
        }
    }
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
