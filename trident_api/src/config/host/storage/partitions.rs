use std::{fmt::Display, num::ParseIntError, str::FromStr};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{constants::PARTITION_SIZE_GROW, primitives::bytes::ByteCount, BlockDeviceId};

#[cfg(feature = "schemars")]
use crate::schema_helpers::{block_device_id_schema, unit_enum_with_untagged_variant};

/// Per partition configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Partition {
    /// A unique identifier for the partition.
    ///
    /// This is a user defined string that allows to link the partition to the
    /// mount points and also to results in the Host Status. The identifier
    /// needs to be unique across all types of devices, not just partitions.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The type of the partition.
    ///
    /// As defined by the [Discoverable Partitions Specification](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/).
    #[serde(rename = "type")]
    pub partition_type: PartitionType,

    /// Size of the partition.
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "unit_enum_with_untagged_variant::<PartitionSize, ByteCount>")
    )]
    pub size: PartitionSize,
}

/// Settings to adopt a pre-existing partition.
///
/// Only ONE match criteria should be provided.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct AdoptedPartition {
    /// A unique identifier for the partition.
    ///
    /// This is a user defined string that allows to link the partition to the
    /// mount points and also to results in the Host Status. The identifier
    /// needs to be unique across all types of devices, not just partitions.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// Partition label to look for when matching against the existing partitions.
    pub match_label: Option<String>,

    /// Partition UUID to look for when matching against the existing partitions.
    pub match_uuid: Option<Uuid>,
}

/// Partition types as defined by The Discoverable Partitions Specification (<https://uapi-group.org/specifications/specs/discoverable_partitions_specification/>).
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
#[cfg_attr(feature = "documentation", derive(strum_macros::EnumIter))]
pub enum PartitionType {
    /// # EFI System Partition
    ///
    /// `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`
    ///
    /// If ESP is not on `raid1`, Trident will use the first partition of this type found in the
    /// Host Configuration.
    Esp,

    /// # Root partition
    ///
    /// x64: `4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709`
    Root,

    /// # Swap partition
    ///
    /// `0657fd6d-a4ab-43c4-84e5-0933c84b4f4f`
    Swap,

    /// # Root partition with dm-verity enabled
    ///
    /// x64: `2c7357ed-ebd2-46d9-aec1-23d437ec2bf5`
    RootVerity,

    /// # Home partition
    ///
    /// `933ac7e1-2eb4-4f13-b844-0e14e2aef915`
    Home,

    /// # Var partition
    ///
    /// `4d21b016-b534-45c2-a9fb-5c16e091fd2d`
    Var,

    /// # Usr partition
    ///
    /// x64: `8484680c-9521-48c6-9c11-b0720656f69e`
    Usr,

    /// # Tmp partition
    ///
    /// `7ec6f557-3bc5-4aca-b293-16ef5df639d1`
    Tmp,

    /// # Generic Linux partition
    ///
    /// `0fc63daf-8483-4772-8e79-3d69d8477de4`
    LinuxGeneric,

    /// # Server Data partition
    ///
    /// `3b8f8425-20e0-4f3b-907f-1a25a76f98e8`
    ///
    /// To use this partition type on the disk with the root volume, make sure
    /// to not have `/srv` symlink present in your root volume filesystem. If
    /// you do, remove it before running Trident (e.g. by using MIC).
    Srv,

    /// # Extended Boot Loader Partition
    ///
    /// `bc13c2ff-59e6-4262-a352-b275fd6f7172`
    Xbootldr,
}

impl PartitionType {
    /// Helper function that returns PartititionType as a string. Return values
    /// are based on GPT partition type identifiers, as defined in the Type
    /// section of systemd repart.d manual:
    /// <https://www.man7.org/linux/man-pages/man5/repart.d.5.html>.
    pub fn to_sdrepart_part_type(&self) -> &str {
        match self {
            PartitionType::Esp => "esp",
            PartitionType::Root => "root",
            PartitionType::Swap => "swap",
            PartitionType::RootVerity => "root-verity",
            PartitionType::Home => "home",
            PartitionType::Var => "var",
            PartitionType::Usr => "usr",
            PartitionType::Tmp => "tmp",
            PartitionType::LinuxGeneric => "linux-generic",
            PartitionType::Srv => "srv",
            PartitionType::Xbootldr => "xbootldr",
        }
    }

    /// Returns the corresponding verity partition type for a given partition type.
    pub fn to_verity(&self) -> Option<Self> {
        match self {
            Self::Root => Some(PartitionType::RootVerity),
            Self::RootVerity
            | Self::Esp
            | Self::Swap
            | Self::Home
            | Self::Var
            | Self::Usr
            | Self::Tmp
            | Self::LinuxGeneric
            | Self::Srv
            | Self::Xbootldr => None,
        }
    }
}

impl Display for PartitionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.to_sdrepart_part_type())
    }
}

/// Partition size enum.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Copy)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum PartitionSize {
    /// # Grow
    ///
    /// Grow a partition to use all available space.
    Grow,

    /// # Fixed
    ///
    /// Fixed size in bytes. Must be a non-zero multiple of 4096 bytes.
    #[serde(untagged)]
    Fixed(ByteCount),
}

impl FromStr for PartitionSize {
    type Err = ParseIntError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        Ok(if s == PARTITION_SIZE_GROW {
            PartitionSize::Grow
        } else {
            PartitionSize::Fixed(ByteCount::from_human_readable(s)?)
        })
    }
}

impl Display for PartitionSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PartitionSize::Fixed(n) => write!(f, "{}", n.to_human_readable()),
            PartitionSize::Grow => write!(f, "{}", PARTITION_SIZE_GROW),
        }
    }
}

impl From<u64> for PartitionSize {
    fn from(v: u64) -> Self {
        PartitionSize::Fixed(v.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialization_roundtrip() {
        #[derive(Debug, serde::Deserialize, serde::Serialize, PartialEq, Eq)]
        struct TestStruct {
            size: PartitionSize,
        }

        impl TestStruct {
            fn fixed(v: u64) -> Self {
                Self {
                    size: PartitionSize::Fixed(v.into()),
                }
            }

            fn grow() -> Self {
                Self {
                    size: PartitionSize::Grow,
                }
            }
        }

        // Define test cases
        let test_cases = [
            ("size: grow", TestStruct::grow(), "size: grow"),
            ("size: 1", TestStruct::fixed(1), "size: 1"),
            ("size: 512", TestStruct::fixed(512), "size: 512"),
            ("size: 1K", TestStruct::fixed(1024), "size: 1K"),
            ("size: 1024", TestStruct::fixed(1024), "size: 1K"),
            ("size: 1M", TestStruct::fixed(1048576), "size: 1M"),
            ("size: 1048576", TestStruct::fixed(1048576), "size: 1M"),
            ("size: 1G", TestStruct::fixed(1073741824), "size: 1G"),
            (
                "size: 1073741824",
                TestStruct::fixed(1073741824),
                "size: 1G",
            ),
            ("size: 1024M", TestStruct::fixed(1073741824), "size: 1G"),
        ];

        // Test (de)serialization
        for (input_yaml, expected_struct, expected_yaml) in test_cases.iter() {
            let actual: TestStruct = serde_yaml::from_str(input_yaml).unwrap();
            assert_eq!(
                actual, *expected_struct,
                "failed to deserialize '{input_yaml}'"
            );

            let actual = serde_yaml::to_string(&actual).unwrap();
            assert_eq!(
                actual.trim(),
                *expected_yaml,
                "failed to serialize '{expected_struct:?}'"
            );
        }
    }
}
