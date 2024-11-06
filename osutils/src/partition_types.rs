use std::str::FromStr;

use anyhow::{bail, Error};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

#[cfg(test)]
use strum_macros::EnumIter;

/// Partition types supported by `systemd-repart`.
///
/// Note: Secondary arch partition types are intentionally ignored.
#[derive(Debug, Clone, PartialEq, Eq, Copy, Hash)]
#[cfg_attr(test, derive(EnumIter))]
pub enum DiscoverablePartitionType {
    /// EFI System Partition
    Esp,

    /// Extended Boot Loader Partition
    Xbootldr,

    /// Swap partition
    Swap,

    /// Home (/home/) partition
    Home,

    /// Server data (/srv/) partition
    Srv,

    /// Variable data (/var/) partition
    Var,

    /// Temporary data (/var/tmp/) partition
    Tmp,

    /// Generic Linux file system partition
    LinuxGeneric,

    /// Root file system partition type appropriate for the local architecture
    ///
    /// This is an alias that gets resolved to the appropriate architecture
    /// specific partition type. E.g. on x86_64 this resolves to `RootAmd64`.
    Root,

    /// Verity data for the root file system partition for the local architecture
    ///
    /// This is an alias that gets resolved to the appropriate architecture
    /// specific partition type. E.g. on x86_64 this resolves to `RootVerityAmd64`.
    RootVerity,

    /// Verity signature data for the root file system partition for the local architecture
    ///
    /// This is an alias that gets resolved to the appropriate architecture
    /// specific partition type. E.g. on x86_64 this resolves to `RootVeritySigAmd64`.
    RootVeritySig,

    /// /usr/ file system partition type appropriate for the local architecture
    ///
    /// This is an alias that gets resolved to the appropriate architecture
    /// specific partition type. E.g. on x86_64 this resolves to `UsrAmd64`.
    Usr,

    /// Verity data for the /usr/ file system partition for the local architecture
    ///
    /// This is an alias that gets resolved to the appropriate architecture
    /// specific partition type. E.g. on x86_64 this resolves to `UsrVerityAmd64`.
    UsrVerity,

    /// Verity signature data for the /usr/ file system partition for the local architecture
    ///
    /// This is an alias that gets resolved to the appropriate architecture
    /// specific partition type. E.g. on x86_64 this resolves to `UsrVeritySigAmd64`.
    UsrVeritySig,

    // Arch specific

    // x86_64 / amd64
    /// Root file system partition in AMD64
    RootAmd64,
    /// Verity data for the root file system partition in AMD64
    RootAmd64Verity,
    /// Verity signature data for the root file system partition in AMD64
    RootAmd64VeritySig,
    /// /usr/ file system partition in AMD64
    UsrAmd64,
    /// Verity data for the /usr/ file system partition in AMD64
    UsrAmd64Verity,
    /// Verity signature data for the /usr/ file system partition in AMD64
    UsrAmd64VeritySig,

    // arm64 / aarch64
    /// Root file system partition in ARM64
    RootArm64,
    /// Verity data for the root file system partition in ARM64
    RootArm64Verity,
    /// Verity signature data for the root file system partition in ARM64
    RootArm64VeritySig,
    /// /usr/ file system partition in ARM64
    UsrArm64,
    /// Verity data for the /usr/ file system partition in ARM64
    UsrArm64Verity,
    /// Verity signature data for the /usr/ file system partition in ARM64
    UsrArm64VeritySig,

    /// Unknown type not contained in the Discoverable Partition Specification
    Unknown(Uuid),
}

impl DiscoverablePartitionType {
    /// Resolves aliases into the real partition type matching the current architecture.
    pub fn resolve(&self) -> Self {
        #[cfg(target_arch = "x86_64")]
        match self {
            Self::Root => Self::RootAmd64,
            Self::RootVerity => Self::RootAmd64Verity,
            Self::RootVeritySig => Self::RootAmd64VeritySig,
            Self::Usr => Self::UsrAmd64,
            Self::UsrVerity => Self::UsrAmd64Verity,
            Self::UsrVeritySig => Self::UsrAmd64VeritySig,
            _ => *self,
        }

        #[cfg(target_arch = "aarch64")]
        match self {
            Self::Root => Self::RootArm64,
            Self::RootVerity => Self::RootArm64Verity,
            Self::RootVeritySig => Self::RootArm64VeritySig,
            Self::Usr => Self::UsrArm64,
            Self::UsrVerity => Self::UsrArm64Verity,
            Self::UsrVeritySig => Self::UsrArm64VeritySig,
            _ => *self,
        }
    }

    pub fn to_str(&self) -> &'static str {
        match self {
            DiscoverablePartitionType::Esp => "esp",
            DiscoverablePartitionType::Xbootldr => "xbootldr",
            DiscoverablePartitionType::Swap => "swap",
            DiscoverablePartitionType::Home => "home",
            DiscoverablePartitionType::Srv => "srv",
            DiscoverablePartitionType::Var => "var",
            DiscoverablePartitionType::Tmp => "tmp",
            DiscoverablePartitionType::LinuxGeneric => "linux-generic",
            DiscoverablePartitionType::Root => "root",
            DiscoverablePartitionType::RootVerity => "root-verity",
            DiscoverablePartitionType::RootVeritySig => "root-verity-sig",
            DiscoverablePartitionType::Usr => "usr",
            DiscoverablePartitionType::UsrVerity => "usr-verity",
            DiscoverablePartitionType::UsrVeritySig => "usr-verity-sig",

            // Arch specific

            // x86_64 / amd64
            DiscoverablePartitionType::RootAmd64 => "root-x86-64",
            DiscoverablePartitionType::RootAmd64Verity => "root-x86-64-verity",
            DiscoverablePartitionType::RootAmd64VeritySig => "root-x86-64-verity-sig",
            DiscoverablePartitionType::UsrAmd64 => "usr-86-64",
            DiscoverablePartitionType::UsrAmd64Verity => "usr-x86-64-verity",
            DiscoverablePartitionType::UsrAmd64VeritySig => "usr-x86-64-verity-sig",

            // arm64 / aarch64
            DiscoverablePartitionType::RootArm64 => "root-arm64",
            DiscoverablePartitionType::RootArm64Verity => "root-arm64-verity",
            DiscoverablePartitionType::RootArm64VeritySig => "root-arm64-verity-sig",
            DiscoverablePartitionType::UsrArm64 => "usr-arm64",
            DiscoverablePartitionType::UsrArm64Verity => "usr-arm64-verity",
            DiscoverablePartitionType::UsrArm64VeritySig => "usr-arm64-verity-sig",

            // Unknown type
            DiscoverablePartitionType::Unknown(_) => "unknown",
        }
    }

    pub fn try_from_str(val: &str) -> Result<Self, Error> {
        Ok(match val {
            "esp" => DiscoverablePartitionType::Esp,
            "xbootldr" => DiscoverablePartitionType::Xbootldr,
            "swap" => DiscoverablePartitionType::Swap,
            "home" => DiscoverablePartitionType::Home,
            "srv" => DiscoverablePartitionType::Srv,
            "var" => DiscoverablePartitionType::Var,
            "tmp" => DiscoverablePartitionType::Tmp,
            "linux-generic" => DiscoverablePartitionType::LinuxGeneric,
            "root" => DiscoverablePartitionType::Root,
            "root-verity" => DiscoverablePartitionType::RootVerity,
            "root-verity-sig" => DiscoverablePartitionType::RootVeritySig,
            "usr" => DiscoverablePartitionType::Usr,
            "usr-verity" => DiscoverablePartitionType::UsrVerity,
            "usr-verity-sig" => DiscoverablePartitionType::UsrVeritySig,

            // Arch specific

            // x86_64 / amd64
            "root-x86-64" => DiscoverablePartitionType::RootAmd64,
            "root-x86-64-verity" => DiscoverablePartitionType::RootAmd64Verity,
            "root-x86-64-verity-sig" => DiscoverablePartitionType::RootAmd64VeritySig,
            "usr-86-64" => DiscoverablePartitionType::UsrAmd64,
            "usr-x86-64-verity" => DiscoverablePartitionType::UsrAmd64Verity,
            "usr-x86-64-verity-sig" => DiscoverablePartitionType::UsrAmd64VeritySig,

            // arm64 / aarch64
            "root-arm64" => DiscoverablePartitionType::RootArm64,
            "root-arm64-verity" => DiscoverablePartitionType::RootArm64Verity,
            "root-arm64-verity-sig" => DiscoverablePartitionType::RootArm64VeritySig,
            "usr-arm64" => DiscoverablePartitionType::UsrArm64,
            "usr-arm64-verity" => DiscoverablePartitionType::UsrArm64Verity,
            "usr-arm64-verity-sig" => DiscoverablePartitionType::UsrArm64VeritySig,
            _ => bail!("Unknown partition type: {}", val),
        })
    }

    pub fn to_uuid(&self) -> Uuid {
        Uuid::from_u128(match self.resolve() {
            DiscoverablePartitionType::Esp => 0xc12a7328_f81f_11d2_ba4b_00a0c93ec93bu128,
            DiscoverablePartitionType::Xbootldr => 0xbc13c2ff_59e6_4262_a352_b275fd6f7172u128,
            DiscoverablePartitionType::Swap => 0x0657fd6d_a4ab_43c4_84e5_0933c84b4f4fu128,
            DiscoverablePartitionType::Home => 0x933ac7e1_2eb4_4f13_b844_0e14e2aef915u128,
            DiscoverablePartitionType::Srv => 0x3b8f8425_20e0_4f3b_907f_1a25a76f98e8u128,
            DiscoverablePartitionType::Var => 0x4d21b016_b534_45c2_a9fb_5c16e091fd2du128,
            DiscoverablePartitionType::Tmp => 0x7ec6f557_3bc5_4aca_b293_16ef5df639d1u128,
            DiscoverablePartitionType::LinuxGeneric => 0x0fc63daf_8483_4772_8e79_3d69d8477de4u128,

            // Arch specific

            // x86_64 / amd64
            DiscoverablePartitionType::RootAmd64 => 0x4f68bce3_e8cd_4db1_96e7_fbcaf984b709u128,
            DiscoverablePartitionType::RootAmd64Verity => {
                0x2c7357ed_ebd2_46d9_aec1_23d437ec2bf5u128
            }
            DiscoverablePartitionType::RootAmd64VeritySig => {
                0x41092b05_9fc8_4523_994f_2def0408b176u128
            }
            DiscoverablePartitionType::UsrAmd64 => 0x8484680c_9521_48c6_9c11_b0720656f69eu128,
            DiscoverablePartitionType::UsrAmd64Verity => 0x77ff5f63_e7b6_4633_acf4_1565b864c0e6u128,
            DiscoverablePartitionType::UsrAmd64VeritySig => {
                0xe7bb33fb_06cf_4e81_8273_e543b413e2e2u128
            }

            // arm64 / aarch64
            DiscoverablePartitionType::RootArm64 => 0xb921b045_1df0_41c3_af44_4c6f280d3faeu128,
            DiscoverablePartitionType::RootArm64Verity => {
                0xdf3300ce_d69f_4c92_978c_9bfb0f38d820u128
            }
            DiscoverablePartitionType::RootArm64VeritySig => {
                0x6db69de6_29f4_4758_a7a5_962190f00ce3u128
            }
            DiscoverablePartitionType::UsrArm64 => 0xb0e01050_ee5f_4390_949a_9101b17104e9u128,
            DiscoverablePartitionType::UsrArm64Verity => 0x6e11a4e7_fbca_4ded_b9e9_e1a512bb664eu128,
            DiscoverablePartitionType::UsrArm64VeritySig => {
                0xc23ce4ff_44bd_4b00_b2d4_b41b3419e02au128
            }

            // These are aliases for the current architecture, by calling resolve() above we
            // should never get here.
            DiscoverablePartitionType::Root => unreachable!(),
            DiscoverablePartitionType::RootVerity => unreachable!(),
            DiscoverablePartitionType::RootVeritySig => unreachable!(),
            DiscoverablePartitionType::Usr => unreachable!(),
            DiscoverablePartitionType::UsrVerity => unreachable!(),
            DiscoverablePartitionType::UsrVeritySig => unreachable!(),

            // Unknown type
            DiscoverablePartitionType::Unknown(uuid) => return uuid,
        })
    }

    pub fn from_uuid(val: &Uuid) -> Self {
        match val.as_u128() {
            0xc12a7328_f81f_11d2_ba4b_00a0c93ec93bu128 => DiscoverablePartitionType::Esp,
            0xbc13c2ff_59e6_4262_a352_b275fd6f7172u128 => DiscoverablePartitionType::Xbootldr,
            0x0657fd6d_a4ab_43c4_84e5_0933c84b4f4fu128 => DiscoverablePartitionType::Swap,
            0x933ac7e1_2eb4_4f13_b844_0e14e2aef915u128 => DiscoverablePartitionType::Home,
            0x3b8f8425_20e0_4f3b_907f_1a25a76f98e8u128 => DiscoverablePartitionType::Srv,
            0x4d21b016_b534_45c2_a9fb_5c16e091fd2du128 => DiscoverablePartitionType::Var,
            0x7ec6f557_3bc5_4aca_b293_16ef5df639d1u128 => DiscoverablePartitionType::Tmp,
            0x0fc63daf_8483_4772_8e79_3d69d8477de4u128 => DiscoverablePartitionType::LinuxGeneric,

            // Arch specific

            // x86_64 / amd64
            0x4f68bce3_e8cd_4db1_96e7_fbcaf984b709u128 => DiscoverablePartitionType::RootAmd64,
            0x2c7357ed_ebd2_46d9_aec1_23d437ec2bf5u128 => {
                DiscoverablePartitionType::RootAmd64Verity
            }
            0x41092b05_9fc8_4523_994f_2def0408b176u128 => {
                DiscoverablePartitionType::RootAmd64VeritySig
            }
            0x8484680c_9521_48c6_9c11_b0720656f69eu128 => DiscoverablePartitionType::UsrAmd64,
            0x77ff5f63_e7b6_4633_acf4_1565b864c0e6u128 => DiscoverablePartitionType::UsrAmd64Verity,
            0xe7bb33fb_06cf_4e81_8273_e543b413e2e2u128 => {
                DiscoverablePartitionType::UsrAmd64VeritySig
            }

            // arm64 / aarch64
            0xb921b045_1df0_41c3_af44_4c6f280d3faeu128 => DiscoverablePartitionType::RootArm64,
            0xdf3300ce_d69f_4c92_978c_9bfb0f38d820u128 => {
                DiscoverablePartitionType::RootArm64Verity
            }
            0x6db69de6_29f4_4758_a7a5_962190f00ce3u128 => {
                DiscoverablePartitionType::RootArm64VeritySig
            }
            0xb0e01050_ee5f_4390_949a_9101b17104e9u128 => DiscoverablePartitionType::UsrArm64,
            0x6e11a4e7_fbca_4ded_b9e9_e1a512bb664eu128 => DiscoverablePartitionType::UsrArm64Verity,
            0xc23ce4ff_44bd_4b00_b2d4_b41b3419e02au128 => {
                DiscoverablePartitionType::UsrArm64VeritySig
            }

            _ => DiscoverablePartitionType::Unknown(*val),
        }
    }

    pub fn is_unknown(&self) -> bool {
        matches!(self, Self::Unknown(_))
    }
}

impl<'de> Deserialize<'de> for DiscoverablePartitionType {
    fn deserialize<D>(deserializer: D) -> Result<DiscoverablePartitionType, D::Error>
    where
        D: Deserializer<'de>,
    {
        // Get the value as a string
        let value = String::deserialize(deserializer)?;

        // Attempt to parse the string as a UUID
        if let Ok(parsed_uuid) = Uuid::from_str(&value) {
            // If we succeed, try to convert the UUID to a partition type
            Ok(DiscoverablePartitionType::from_uuid(&parsed_uuid))
        } else {
            // Otherwise, try to parse the string as a partition type name
            DiscoverablePartitionType::try_from_str(&value).map_err(serde::de::Error::custom)
        }
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;
    #[test]
    fn test_repart_types() {
        // Iterate over all partition types and check that the round-trip is ok
        for partition_type in DiscoverablePartitionType::iter().filter(|pt| !pt.is_unknown()) {
            let uuid = partition_type.to_uuid();
            let partition_type_from_uuid = DiscoverablePartitionType::from_uuid(&uuid);
            assert_eq!(
                partition_type.resolve(), // We need to resolve aliases
                partition_type_from_uuid,
                "Round-trip failed for partition type {}",
                partition_type.to_str()
            );
        }
    }

    #[test]
    fn test_bad_uuid() {
        // Check that known bad UUID will map to Unknown type
        let bad_uuid = Uuid::from_u128(0x00000000_0000_0000_0000_000000000000u128);
        let partition_type_from_uuid = DiscoverablePartitionType::from_uuid(&bad_uuid);
        let expected_partition_type = DiscoverablePartitionType::Unknown(bad_uuid);
        assert_eq!(
            expected_partition_type, partition_type_from_uuid,
            "Round-trip failed for bad UUID"
        );
    }

    #[test]
    fn test_name_roundtrip() {
        for partition_type in DiscoverablePartitionType::iter().filter(|pt| !pt.is_unknown()) {
            let name = partition_type.to_str();
            let partition_type_from_name = DiscoverablePartitionType::try_from_str(name).unwrap();
            assert_eq!(
                partition_type, // We need to resolve aliases
                partition_type_from_name,
                "Round-trip failed for partition type {}",
                partition_type.to_str()
            );
        }
    }
}
