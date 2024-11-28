use enumflags2::bitflags;

/// Represents the Platform Configuration Registers (PCRs) in the TPM.
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
    /// PCR 7, or `secure-boot-policy`.
    Pcr7 = 1 << 7,
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
    /// PCR 23, or `application-support`.
    Pcr23 = 1 << 23,
}

impl Pcr {
    /// Returns the digit value of the PCR.
    pub fn to_value(&self) -> u32 {
        (*self as u32).trailing_zeros()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_value() {
        assert_eq!(Pcr::Pcr0.to_value(), 0);
        assert_eq!(Pcr::Pcr1.to_value(), 1);
        assert_eq!(Pcr::Pcr2.to_value(), 2);
        assert_eq!(Pcr::Pcr3.to_value(), 3);
        assert_eq!(Pcr::Pcr4.to_value(), 4);
        assert_eq!(Pcr::Pcr5.to_value(), 5);
        assert_eq!(Pcr::Pcr7.to_value(), 7);
        assert_eq!(Pcr::Pcr9.to_value(), 9);
        assert_eq!(Pcr::Pcr10.to_value(), 10);
        assert_eq!(Pcr::Pcr11.to_value(), 11);
        assert_eq!(Pcr::Pcr12.to_value(), 12);
        assert_eq!(Pcr::Pcr13.to_value(), 13);
        assert_eq!(Pcr::Pcr14.to_value(), 14);
        assert_eq!(Pcr::Pcr15.to_value(), 15);
        assert_eq!(Pcr::Pcr16.to_value(), 16);
        assert_eq!(Pcr::Pcr23.to_value(), 23);
    }
}
