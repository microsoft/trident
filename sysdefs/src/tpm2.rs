use anyhow::{bail, Error};
use enumflags2::bitflags;
use serde::{self, Deserialize, Serialize};
use strum_macros::{EnumString, IntoStaticStr};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

/// Represents the Platform Configuration Registers (PCRs) in the TPM. Each PCR is associated with
/// a digit number and a string name.
///
/// NOTE: In the current encryption logic, only the following PCRs are supported when sealing
/// encrypted volumes against the state of TPM 2.0 device:
/// - 4, or `boot-loader-code`
/// - 7, or `secure-boot-policy`
/// - 11, or `kernel-boot`.
#[bitflags]
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, IntoStaticStr, EnumString)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum Pcr {
    /// PCR 0, or `platform-code`.
    #[strum(serialize = "platform-code")]
    Pcr0 = 1 << 0,
    /// PCR 1, or `platform-config`.
    #[strum(serialize = "platform-config")]
    Pcr1 = 1 << 1,
    /// PCR 2, or `external-code`.
    #[strum(serialize = "external-code")]
    Pcr2 = 1 << 2,
    /// PCR 3, or `external-config`.
    #[strum(serialize = "external-config")]
    Pcr3 = 1 << 3,
    /// PCR 4, or `boot-loader-code`.
    #[strum(serialize = "boot-loader-code")]
    Pcr4 = 1 << 4,
    /// PCR 5, or `boot-loader-config`.
    #[strum(serialize = "boot-loader-config")]
    Pcr5 = 1 << 5,
    /// PCR 6, or `host-platform`.
    #[strum(serialize = "host-platform")]
    Pcr6 = 1 << 6,
    /// PCR 7, or `secure-boot-policy`.
    #[strum(serialize = "secure-boot-policy")]
    Pcr7 = 1 << 7,
    /// PCR 8.
    #[strum(serialize = "pcr8")]
    Pcr8 = 1 << 8,
    /// PCR 9, or `kernel-initrd`.
    #[strum(serialize = "kernel-initrd")]
    Pcr9 = 1 << 9,
    /// PCR 10, or `ima`.
    #[strum(serialize = "ima")]
    Pcr10 = 1 << 10,
    /// PCR 11, or `kernel-boot`.
    #[strum(serialize = "kernel-boot")]
    Pcr11 = 1 << 11,
    /// PCR 12, or `kernel-config`.
    #[strum(serialize = "kernel-config")]
    Pcr12 = 1 << 12,
    /// PCR 13, or `sysexts`.
    #[strum(serialize = "sysexts")]
    Pcr13 = 1 << 13,
    /// PCR 14, or `shim-policy`.
    #[strum(serialize = "shim-policy")]
    Pcr14 = 1 << 14,
    /// PCR 15, or `system-identity`.
    #[strum(serialize = "system-identity")]
    Pcr15 = 1 << 15,
    /// PCR 16, or `debug`.
    #[strum(serialize = "debug")]
    Pcr16 = 1 << 16,
    /// PCR 17.
    #[strum(serialize = "pcr17")]
    Pcr17 = 1 << 17,
    /// PCR 18.
    #[strum(serialize = "pcr18")]
    Pcr18 = 1 << 18,
    /// PCR 19.
    #[strum(serialize = "pcr19")]
    Pcr19 = 1 << 19,
    /// PCR 20.
    #[strum(serialize = "pcr20")]
    Pcr20 = 1 << 20,
    /// PCR 21.
    #[strum(serialize = "pcr21")]
    Pcr21 = 1 << 21,
    /// PCR 22.
    #[strum(serialize = "pcr22")]
    Pcr22 = 1 << 22,
    /// PCR 23, or `application-support`.
    #[strum(serialize = "application-support")]
    Pcr23 = 1 << 23,
}

impl Pcr {
    /// Returns the digit representation of the PCR number.
    pub fn to_num(&self) -> u32 {
        (*self as u32).trailing_zeros()
    }

    /// Returns the PCR for the given digit number. Needed for deserialization.
    pub fn from_num(num: u32) -> Result<Self, Error> {
        match num {
            0 => Ok(Pcr::Pcr0),
            1 => Ok(Pcr::Pcr1),
            2 => Ok(Pcr::Pcr2),
            3 => Ok(Pcr::Pcr3),
            4 => Ok(Pcr::Pcr4),
            5 => Ok(Pcr::Pcr5),
            6 => Ok(Pcr::Pcr6),
            7 => Ok(Pcr::Pcr7),
            8 => Ok(Pcr::Pcr8),
            9 => Ok(Pcr::Pcr9),
            10 => Ok(Pcr::Pcr10),
            11 => Ok(Pcr::Pcr11),
            12 => Ok(Pcr::Pcr12),
            13 => Ok(Pcr::Pcr13),
            14 => Ok(Pcr::Pcr14),
            15 => Ok(Pcr::Pcr15),
            16 => Ok(Pcr::Pcr16),
            17 => Ok(Pcr::Pcr17),
            18 => Ok(Pcr::Pcr18),
            19 => Ok(Pcr::Pcr19),
            20 => Ok(Pcr::Pcr20),
            21 => Ok(Pcr::Pcr21),
            22 => Ok(Pcr::Pcr22),
            23 => Ok(Pcr::Pcr23),
            _ => bail!("Failed to convert an invalid PCR number '{}' to a Pcr", num),
        }
    }

    /// Returns a human-readable string representation of the PCR. The strings are based on the
    /// `systemd-cryptenroll` documentation published here:
    /// https://www.man7.org/linux/man-pages/man1/systemd-cryptenroll.1.html.
    pub fn as_str(&self) -> &'static str {
        // Use strum's IntoStaticStr trait
        self.into()
    }

    /// Returns the PCR for the given string name.
    pub fn from_str_name(s: &str) -> Result<Self, Error> {
        // Use the strum-generated FromStr implementation
        use std::str::FromStr;
        Self::from_str(s).map_err(|_| anyhow::anyhow!("Failed to convert string '{}' to a Pcr", s))
    }
}

impl Serialize for Pcr {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize as string representation for better readability
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for Pcr {
    fn deserialize<D>(deserializer: D) -> Result<Pcr, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct PcrVisitor;

        impl serde::de::Visitor<'_> for PcrVisitor {
            type Value = Pcr;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a PCR number (0-23) or string name (e.g., 'boot-loader-code')")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Pcr::from_num(value as u32)
                    .map_err(|_| E::custom(format!("Invalid PCR number: {value}")))
            }

            fn visit_u32<E>(self, value: u32) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Pcr::from_num(value).map_err(|_| E::custom(format!("Invalid PCR number: {value}")))
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                Pcr::from_str_name(value)
                    .map_err(|_| E::custom(format!("Invalid PCR string: '{value}'")))
            }
        }

        deserializer.deserialize_any(PcrVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_num() {
        assert_eq!(Pcr::Pcr0.to_num(), 0);
        assert_eq!(Pcr::Pcr1.to_num(), 1);
        assert_eq!(Pcr::Pcr2.to_num(), 2);
        assert_eq!(Pcr::Pcr3.to_num(), 3);
        assert_eq!(Pcr::Pcr4.to_num(), 4);
        assert_eq!(Pcr::Pcr5.to_num(), 5);
        assert_eq!(Pcr::Pcr6.to_num(), 6);
        assert_eq!(Pcr::Pcr7.to_num(), 7);
        assert_eq!(Pcr::Pcr8.to_num(), 8);
        assert_eq!(Pcr::Pcr9.to_num(), 9);
        assert_eq!(Pcr::Pcr10.to_num(), 10);
        assert_eq!(Pcr::Pcr11.to_num(), 11);
        assert_eq!(Pcr::Pcr12.to_num(), 12);
        assert_eq!(Pcr::Pcr13.to_num(), 13);
        assert_eq!(Pcr::Pcr14.to_num(), 14);
        assert_eq!(Pcr::Pcr15.to_num(), 15);
        assert_eq!(Pcr::Pcr16.to_num(), 16);
        assert_eq!(Pcr::Pcr17.to_num(), 17);
        assert_eq!(Pcr::Pcr18.to_num(), 18);
        assert_eq!(Pcr::Pcr19.to_num(), 19);
        assert_eq!(Pcr::Pcr20.to_num(), 20);
        assert_eq!(Pcr::Pcr21.to_num(), 21);
        assert_eq!(Pcr::Pcr22.to_num(), 22);
        assert_eq!(Pcr::Pcr23.to_num(), 23);
    }

    #[test]
    fn test_from_num() {
        // Test case #0: Convert a valid value to a PCR.
        assert_eq!(Pcr::from_num(0).unwrap(), Pcr::Pcr0);
        assert_eq!(Pcr::from_num(1).unwrap(), Pcr::Pcr1);
        assert_eq!(Pcr::from_num(2).unwrap(), Pcr::Pcr2);
        assert_eq!(Pcr::from_num(23).unwrap(), Pcr::Pcr23);

        // Test case #1: Convert an invalid value to a PCR.
        assert_eq!(
            Pcr::from_num(31).unwrap_err().root_cause().to_string(),
            "Failed to convert an invalid PCR number '31' to a Pcr"
        );
    }

    #[test]
    fn test_from_str_name() {
        // Test valid string representations
        assert_eq!(Pcr::from_str_name("platform-code").unwrap(), Pcr::Pcr0);
        assert_eq!(Pcr::from_str_name("boot-loader-code").unwrap(), Pcr::Pcr4);
        assert_eq!(Pcr::from_str_name("secure-boot-policy").unwrap(), Pcr::Pcr7);
        assert_eq!(Pcr::from_str_name("kernel-boot").unwrap(), Pcr::Pcr11);
        assert_eq!(
            Pcr::from_str_name("application-support").unwrap(),
            Pcr::Pcr23
        );

        // Test invalid string
        assert!(Pcr::from_str_name("invalid-pcr").is_err());
    }

    #[test]
    fn test_serialize_deserialize() {
        use serde_json;

        // Test serialization (should serialize as string)
        let pcr4 = Pcr::Pcr4;
        let serialized = serde_json::to_string(&pcr4).unwrap();
        assert_eq!(serialized, "\"boot-loader-code\"");

        // Test deserialization from string
        let deserialized: Pcr = serde_json::from_str("\"boot-loader-code\"").unwrap();
        assert_eq!(deserialized, Pcr::Pcr4);

        // Test deserialization from number
        let deserialized: Pcr = serde_json::from_str("4").unwrap();
        assert_eq!(deserialized, Pcr::Pcr4);

        // Test array of mixed types
        let pcrs: Vec<Pcr> = serde_json::from_str("[4, \"secure-boot-policy\", 11]").unwrap();
        assert_eq!(pcrs, vec![Pcr::Pcr4, Pcr::Pcr7, Pcr::Pcr11]);
    }
}
