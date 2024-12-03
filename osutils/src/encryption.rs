use std::path::Path;

use anyhow::{Context, Error};
use enumflags2::BitFlags;

use crate::{dependencies::Dependency, lsblk::BlockDevice, pcr::Pcr};

/// Cipher specification string for the LUKS2 data segment.
pub const CIPHER: &str = "aes-xts-plain64";

/// Key size in bits, limited by the cipher specification.
pub const KEY_SIZE: &str = "512";

/// Reduction in data device size when LUKS2 encryption is initialized.
const LUKS_HEADER_SIZE_IN_MIB: usize = 16;

/// Runs `systemd-cryptenroll` to enroll a TPM 2.0 device for the given device of a LUKS2 encrypted
/// volume.
///
/// Takes in the key file to unlock the TPM 2.0 device, the path to the device, and a set of PCRs
/// to bind the enrollment to. By default, the enrollment is binded to PCR 7 only.
pub fn systemd_cryptenroll(
    key_file: impl AsRef<Path>,
    device_path: impl AsRef<Path>,
    pcrs: BitFlags<Pcr>,
) -> Result<(), Error> {
    Dependency::SystemdCryptenroll
        .cmd()
        .arg("--tpm2-device=auto")
        .arg(to_tpm2_pcrs_arg(pcrs))
        .arg("--unlock-key-file")
        .arg(key_file.as_ref().as_os_str())
        .arg("--wipe-slot=tpm2")
        .arg(device_path.as_ref().as_os_str())
        .run_and_check()
        .context(format!(
            "Failed to enroll TPM 2.0 device for underlying device '{}'",
            device_path.as_ref().display()
        ))
}

/// Runs `cryptsetup-luksFormat` to initialize a LUKS2 encrypted volume for the given underlying
/// device.
///
/// This function is used on a clean install by default.
pub fn cryptsetup_luksformat(
    key_file: impl AsRef<Path>,
    device_path: impl AsRef<Path>,
) -> Result<(), Error> {
    Dependency::Cryptsetup
        .cmd()
        .arg("luksFormat")
        .arg("--cipher")
        .arg(CIPHER)
        .arg("--force-password")
        .arg("--hash")
        .arg("sha512")
        .arg("--iter-time")
        .arg("0")
        .arg("--key-file")
        .arg(key_file.as_ref().as_os_str())
        .arg("--key-size")
        .arg(KEY_SIZE)
        .arg("--key-slot")
        .arg("0")
        .arg("--pbkdf")
        .arg("pbkdf2")
        .arg("--reduce-device-size")
        .arg(format!("{}M", LUKS_HEADER_SIZE_IN_MIB))
        .arg("--type")
        .arg("luks2")
        .arg(device_path.as_ref().as_os_str())
        .run_and_check()
        .context(format!(
            "Failed to encrypt underlying device '{}'",
            device_path.as_ref().display()
        ))
}

/// Runs `cryptsetup-reencrypt` to re-encrypt the LUKS2 encrypted volume for the given underlying
/// device in-place.
///
/// While by default, `cryptsetup-luksFormat` will be used on a clean install, an internal
/// parameter `REENCRYPT_ON_CLEAN_INSTALL` can be set, to instead re-encrypt the volumes.
pub fn cryptsetup_reencrypt(
    key_file: impl AsRef<Path>,
    device_path: impl AsRef<Path>,
) -> Result<(), Error> {
    Dependency::Cryptsetup
        .cmd()
        .arg("reencrypt")
        .arg("--encrypt")
        .arg("--batch-mode")
        .arg("--cipher")
        .arg(CIPHER)
        .arg("--force-password")
        .arg("--hash")
        .arg("sha512")
        .arg("--iter-time")
        .arg("0")
        .arg("--key-file")
        .arg(key_file.as_ref().as_os_str())
        .arg("--key-size")
        .arg(KEY_SIZE)
        .arg("--key-slot")
        .arg("0")
        .arg("--pbkdf")
        .arg("pbkdf2")
        .arg("--reduce-device-size")
        .arg(format!("{}M", LUKS_HEADER_SIZE_IN_MIB))
        .arg("--type")
        .arg("luks2")
        .arg(device_path.as_ref().as_os_str())
        .run_and_check()
        .context(format!(
            "Failed to encrypt underlying device '{}'",
            device_path.as_ref().display()
        ))
}

/// Runs `cryptsetup luksOpen` to open the given LUKS2 device with the provided key file.
pub fn cryptsetup_open(
    key_file: impl AsRef<Path>,
    device_path: impl AsRef<Path>,
    device_name: &str,
) -> Result<(), Error> {
    Dependency::Cryptsetup
        .cmd()
        .arg("luksOpen")
        .arg("--key-file")
        .arg(key_file.as_ref().as_os_str())
        .arg(device_path.as_ref().as_os_str())
        .arg(device_name)
        .run_and_check()
        .context(format!(
            "Failed to open underlying encrypted device '{}' as '{}'",
            device_path.as_ref().display(),
            device_name
        ))
}

/// Runs `cryptsetup luksClose` to close the given LUKS2 device.
pub fn cryptsetup_close(crypt_block_device: &BlockDevice) -> Result<(), Error> {
    Dependency::Cryptsetup
        .cmd()
        .arg("luksClose")
        .arg(crypt_block_device.name.as_str())
        .run_and_check()
        .context(format!(
            "Failed to close pre-existing encrypted volume '{}'",
            crypt_block_device.name
        ))
}

/// Converts the provided PCR bitflags into the `--tpm2-pcrs` argument for `systemd-cryptenroll`.
/// Returns a string with the PCR indices separated by `+`.
fn to_tpm2_pcrs_arg(pcrs: BitFlags<Pcr>) -> String {
    format!(
        "--tpm2-pcrs={}",
        pcrs.iter()
            .map(|flag| flag.to_value().to_string())
            .collect::<Vec<_>>()
            .join("+")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use enumflags2::make_bitflags;

    #[test]
    fn test_to_tpm2_pcrs_arg() {
        let pcrs = make_bitflags!(Pcr::{Pcr1 | Pcr4});
        assert_eq!(to_tpm2_pcrs_arg(pcrs), "--tpm2-pcrs=1+4".to_string());

        let single_pcr = make_bitflags!(Pcr::{Pcr7});
        assert_eq!(to_tpm2_pcrs_arg(single_pcr), "--tpm2-pcrs=7".to_string());

        let all_pcrs = BitFlags::<Pcr>::all();
        assert_eq!(
            to_tpm2_pcrs_arg(all_pcrs),
            "--tpm2-pcrs=0+1+2+3+4+5+7+9+10+11+12+13+14+15+16+23".to_string()
        );
    }
}
