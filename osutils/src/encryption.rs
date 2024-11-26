use std::path::Path;

use anyhow::{Context, Error};

use crate::{dependencies::Dependency, lsblk::BlockDevice};

/// Cipher specification string for the LUKS2 data segment.
pub const CIPHER: &str = "aes-xts-plain64";

/// Key size in bits, limited by the cipher specification.
pub const KEY_SIZE: &str = "512";

/// Reduction in data device size when LUKS2 encryption is initialized.
const LUKS_HEADER_SIZE_IN_MIB: usize = 16;

/// Runs `systemd-cryptenroll` to enroll a TPM 2.0 device for the given device of a LUKS2 encrypted
/// volume.
pub fn systemd_cryptenroll(
    key_file: impl AsRef<Path>,
    device_path: impl AsRef<Path>,
) -> Result<(), Error> {
    Dependency::SystemdCryptenroll
        .cmd()
        .arg("--tpm2-device=auto")
        .arg("--tpm2-pcrs=7")
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

/// Runs `cryptsetup-reencrypt` to re-encrypt the given device with LUKS2 encryption.
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
