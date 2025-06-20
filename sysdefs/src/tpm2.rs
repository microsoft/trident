use anyhow::{bail, Error};
use enumflags2::bitflags;
use serde::{self, Deserialize};

/// Represents the Platform Configuration Registers (PCRs) in the TPM. Each PCR is associated with
/// a digit number and a string name.
#[bitflags]
#[repr(u32)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Pcr {
    /// PCR 0, or `platform-code`.
    Pcr0 = 1 << 0,
    /// PCR 1, or `platform-config`.
    Pcr1 = 1 << 1,
    /// PCR 2, or `external-code`.
    Pcr2 = 1 << 2,
    /// PCR 3, or `external-config`.
    Pcr3 = 1 << 3,
    /// PCR 4, or `boot-loader-code`.
    Pcr4 = 1 << 4,
    /// PCR 5, or `boot-loader-config`.
    Pcr5 = 1 << 5,
    /// PCR 6, or `host-platform`.
    Pcr6 = 1 << 6,
    /// PCR 7, or `secure-boot-policy`.
    Pcr7 = 1 << 7,
    /// PCR 8.
    Pcr8 = 1 << 8,
    /// PCR 9, or `kernel-initrd`.
    Pcr9 = 1 << 9,
    /// PCR 10, or `ima`.
    Pcr10 = 1 << 10,
    /// PCR 11, or `kernel-boot`.
    Pcr11 = 1 << 11,
    /// PCR 12, or `kernel-config`.
    Pcr12 = 1 << 12,
    /// PCR 13, or `sysexts`.
    Pcr13 = 1 << 13,
    /// PCR 14, or `shim-policy`.
    Pcr14 = 1 << 14,
    /// PCR 15, or `system-identity`.
    Pcr15 = 1 << 15,
    /// PCR 16, or `debug`.
    Pcr16 = 1 << 16,
    /// PCR 17.
    Pcr17 = 1 << 17,
    /// PCR 18.
    Pcr18 = 1 << 18,
    /// PCR 19.
    Pcr19 = 1 << 19,
    /// PCR 20.
    Pcr20 = 1 << 20,
    /// PCR 21.
    Pcr21 = 1 << 21,
    /// PCR 22.
    Pcr22 = 1 << 22,
    /// PCR 23, or `application-support`.
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
}

impl<'de> Deserialize<'de> for Pcr {
    fn deserialize<D>(deserializer: D) -> Result<Pcr, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let val = u32::deserialize(deserializer)?;
        Pcr::from_num(val)
            .map_err(|_| serde::de::Error::custom(format!("Failed to deserialize PCR: {val}")))
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
}
