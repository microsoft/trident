use std::{
    fs::{self, File, Permissions},
    io::{Read, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::Mutex,
};

use anyhow::{Context, Error};
use enumflags2::BitFlags;
use log::debug;
use once_cell::sync::Lazy;
use tempfile::NamedTempFile;

use sysdefs::tpm2::Pcr;
use trident_api::constants::LUKS_HEADER_SIZE_IN_MIB;

use crate::{dependencies::Dependency, pcrlock::PCRLOCK_POLICY_JSON_PATH};

/// Cipher specification string for the LUKS2 data segment.
pub const CIPHER: &str = "aes-xts-plain64";

/// Path to the special file that serves as a source of cryptographically secure random numbers. It
/// is used for generating keys for LUKS2 encryption.
pub const DEV_RANDOM_PATH: &str = "/dev/random";

/// Key size in bits, limited by the cipher specification.
pub const KEY_SIZE: &str = "512";

/// Size of the temporary recovery key file in bytes.
const TMP_RECOVERY_KEY_SIZE: usize = 64;

/// Randomly generated key passphrase used for encryption and protected by a mutex. This passphrase
/// is used to re-enroll the TPM 2.0 device using a pcrlock policy.
///
/// TODO: In systemd v256, `--unlock-tpm2-device` is added, which allows to use a TPM 2.0 device,
/// instead of a key file, to unlock the volume. Once systemd v256 is available in AZL 4.0, remove
/// ENCRYPTION_PASSPHRASE and use `--unlock-tpm2-device` instead. Related ADO task:
/// https://dev.azure.com/mariner-org/polar/_workitems/edit/13057/.
pub static ENCRYPTION_PASSPHRASE: Lazy<Mutex<Vec<u8>>> = Lazy::new(Default::default);

/// Runs `systemd-cryptenroll` to enroll a TPM 2.0 device for the given device of a LUKS2 encrypted
/// volume.
///
/// Takes in the key file to unlock the TPM 2.0 device, the path to the device, and a set of PCRs
/// to bind the enrollment to. If a key file is not provided, it means that the device has already
/// been bound to TPM 2.0 and we're re-enrolling it with a pcrlock policy.
pub fn systemd_cryptenroll(
    key_file: Option<impl AsRef<Path>>,
    device_path: impl AsRef<Path>,
    pcrs: BitFlags<Pcr>,
) -> Result<(), Error> {
    debug!(
        "Enrolling TPM 2.0 device for underlying encrypted volume '{}'",
        device_path.as_ref().display()
    );

    let mut cmd = Dependency::SystemdCryptenroll.cmd();
    cmd.arg(device_path.as_ref().as_os_str())
        .arg("--tpm2-device=auto")
        .arg("--wipe-slot=tpm2");

    // If a key file is provided, use it to unlock the TPM 2.0 device; if a key file is not
    // provided, it means that the device has already been bound to TPM 2.0 and we're re-enrolling
    // it with a pcrlock policy. So we use ENCRYPTION_PASSPHRASE to unlock the device.
    let mut _tmp_file;
    if let Some(path) = key_file {
        cmd.arg(format!("--unlock-key-file={}", path.as_ref().display()))
            .arg(to_tpm2_pcrs_arg(pcrs));
    } else {
        let key = {
            ENCRYPTION_PASSPHRASE
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock encryption passphrase in memory"))?
        };

        _tmp_file = NamedTempFile::new()
            .context("Failed to create temporary file for the encryption passphrase")?;
        // Set permissions required for a key file; only owner has read and write permission
        fs::set_permissions(_tmp_file.path(), Permissions::from_mode(0o600))
            .context("Failed to set permissions for temporary file with encryption passphrase")?;
        _tmp_file
            .write_all(&key)
            .context("Failed to write the encryption passphrase to a temporary file")?;

        cmd.arg(format!("--unlock-key-file={}", _tmp_file.path().display()))
            .arg(format!("--tpm2-pcrlock={PCRLOCK_POLICY_JSON_PATH}"));
    }

    cmd.run_and_check().context(format!(
        "Failed to enroll TPM 2.0 device for underlying device '{}'",
        device_path.as_ref().display()
    ))
}

#[derive(Debug, Clone)]
pub enum KeySlotType {
    Password,
    Index(u8),
}

impl std::fmt::Display for KeySlotType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            KeySlotType::Password => write!(f, "password"),
            KeySlotType::Index(idx) => write!(f, "{idx}"),
        }
    }
}

/// Runs `systemd-cryptenroll <encrypted_device_path> --wipe-slot={}` to wipe the desired key slot
/// for the given encrypted device.
pub fn systemd_cryptenroll_wipe_slot(
    device_path: impl AsRef<Path>,
    key_slot: KeySlotType,
) -> Result<(), Error> {
    Dependency::SystemdCryptenroll
        .cmd()
        .arg(device_path.as_ref().as_os_str())
        .arg(format!("--wipe-slot={key_slot}"))
        .run_and_check()
        .context(format!(
            "Failed to wipe key slot '{}' for underlying device '{}'",
            key_slot,
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
        .arg(format!("{LUKS_HEADER_SIZE_IN_MIB}M"))
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
        .arg(format!("{LUKS_HEADER_SIZE_IN_MIB}M"))
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
pub fn cryptsetup_close(device_name: &str) -> Result<(), Error> {
    Dependency::Cryptsetup
        .cmd()
        .arg("luksClose")
        .arg(device_name)
        .run_and_check()
        .context(format!(
            "Failed to close pre-existing encrypted volume '{device_name}'"
        ))
}

/// Converts the provided PCR bitflags into the `--tpm2-pcrs` argument for `systemd-cryptenroll`.
/// Returns a string with the PCR indices separated by `+`.
fn to_tpm2_pcrs_arg(pcrs: BitFlags<Pcr>) -> String {
    format!(
        "--tpm2-pcrs={}",
        pcrs.iter()
            .map(|flag| flag.to_num().to_string())
            .collect::<Vec<_>>()
            .join("+")
    )
}

/// This function creates a file at the specified path and fills it with cryptographically secure
/// random bytes sourced from `/dev/random`. It is intended for generating a recovery key file with
/// a specified size `TMP_RECOVERY_KEY_SIZE`. The function returns the random bytes that were
/// written to the file.
///
/// `path` specifies the location and name of the file to be created, and must be accessible and
/// writable by the process.
///
/// This function can return an error if opening or reading `/dev/random` fails. It can also error
/// when writing to the specified file path fails, which could be due to permission issues,
/// non-existent directories in the path, or other filesystem-related errors.
pub fn generate_recovery_key_file(path: &Path) -> Result<Vec<u8>, Error> {
    let mut random_file =
        File::open(DEV_RANDOM_PATH).context("Failed to open '{DEV_RANDOM_PATH}'")?;

    let mut random_buffer = vec![0u8; TMP_RECOVERY_KEY_SIZE];
    random_file
        .read_exact(&mut random_buffer)
        .context("Failed to read from '{DEV_RANDOM_PATH}'")?;

    fs::write(path, &random_buffer).context(format!(
        "Failed to write random data to recovery key file '{}'",
        path.display()
    ))?;

    Ok(random_buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use enumflags2::make_bitflags;
    use tempfile::{NamedTempFile, TempDir};

    #[test]
    fn test_to_tpm2_pcrs_arg() {
        let pcrs = make_bitflags!(Pcr::{Pcr1 | Pcr4});
        assert_eq!(to_tpm2_pcrs_arg(pcrs), "--tpm2-pcrs=1+4".to_string());

        let single_pcr = make_bitflags!(Pcr::{Pcr7});
        assert_eq!(to_tpm2_pcrs_arg(single_pcr), "--tpm2-pcrs=7".to_string());

        let all_pcrs = BitFlags::<Pcr>::all();
        assert_eq!(
            to_tpm2_pcrs_arg(all_pcrs),
            "--tpm2-pcrs=0+1+2+3+4+5+6+7+8+9+10+11+12+13+14+15+16+17+18+19+20+21+22+23".to_string()
        );
    }

    #[test]
    fn test_generate_recovery_key_file() {
        // Create a temporary file for testing
        let temp_file = NamedTempFile::new().expect("Failed to create temporary file");
        let temp_path = temp_file.path();

        // Call the function to generate the recovery key file
        generate_recovery_key_file(temp_path).expect("Failed to generate recovery key file");

        // Validate the generated file
        let mut generated_file = File::open(temp_path).expect("Failed to open generated file");

        // Check the size of the file
        let metadata = generated_file
            .metadata()
            .expect("Failed to get file metadata");
        assert_eq!(
            metadata.len() as usize,
            TMP_RECOVERY_KEY_SIZE,
            "File size does not match TMP_RECOVERY_KEY_SIZE"
        );

        // Read the file content
        let mut content = vec![0u8; TMP_RECOVERY_KEY_SIZE];
        generated_file
            .read_exact(&mut content)
            .expect("Failed to read file content");

        // Ensure that the file contains non-zero bytes, i.e. some random data
        assert!(
            content.iter().any(|&byte| byte != 0),
            "Generated file contains only zeros, expected random data"
        );
    }

    #[test]
    fn test_generate_recovery_key_file_invalid_path() {
        // Create a temporary directory
        let temp_dir = TempDir::new().expect("Failed to create temporary directory");

        // Construct an invalid file path
        let mut invalid_path = PathBuf::from(temp_dir.path());
        invalid_path.push("non_existent_directory/recovery_key");

        // Attempt to generate the recovery key file at the invalid path
        let result = generate_recovery_key_file(&invalid_path);

        // Ensure the function returns the correct error
        let error_string = result.unwrap_err().root_cause().to_string();
        assert!(
            error_string.contains("No such file or directory (os error 2)"),
            "Unexpected output: {error_string}"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::{
        fs::{OpenOptions, Permissions},
        io::{Read, Seek, SeekFrom, Write},
        os::unix::fs::PermissionsExt,
    };

    use tempfile::NamedTempFile;

    use pytest_gen::functional_test;
    use sysdefs::partition_types::DiscoverablePartitionType;

    use crate::{
        filesystems::MkfsFileSystemType,
        mkfs,
        repart::{RepartEmptyMode, RepartPartitionEntry, SystemdRepartInvoker},
        testutils::repart::{self, TEST_DISK_DEVICE_PATH},
        udevadm,
    };

    const ENCRYPTED_VOLUME_NAME: &str = "encrypted_volume";
    const ENCRYPTED_VOLUME_PATH: &str = "/dev/mapper/encrypted_volume";

    /// Wipes the /dev/sdb device and ensures the /mnt directory exists.
    fn setup_test() {
        // Just zero-out the metadata so this is a fast operation.
        repart::clear_disk(Path::new(TEST_DISK_DEVICE_PATH)).unwrap();
        if !Path::new("/mnt").exists() {
            Dependency::Mkdir.cmd().arg("/mnt").run_and_check().unwrap();
        }
    }

    #[functional_test(feature = "helpers")]
    fn test_cryptsetup_luksformat() {
        // Setup test environment
        setup_test();

        // Create a partition for testing
        let repart = SystemdRepartInvoker::new(TEST_DISK_DEVICE_PATH, RepartEmptyMode::Force)
            .with_partition_entries(vec![RepartPartitionEntry {
                id: "1".to_string(),
                partition_type: DiscoverablePartitionType::Root,
                label: Some("encrypted_partition".to_string()),
                size_min_bytes: Some(50 * 1048576),
                size_max_bytes: Some(100 * 1048576),
            }]);

        let partition1 = &repart.execute().unwrap()[0];

        // Wait for udev to process pending events
        udevadm::settle().unwrap();

        // Create a temporary file to store the recovery key file
        let key_file_tmp = NamedTempFile::new().unwrap();
        let key_file_path = key_file_tmp.path();
        fs::set_permissions(key_file_path, Permissions::from_mode(0o600)).unwrap();
        generate_recovery_key_file(key_file_path).unwrap();

        // Run `cryptsetup-luksFormat` on the partition
        cryptsetup_luksformat(key_file_path, &partition1.node).unwrap();

        // Run `systemd-cryptenroll` on the partition
        systemd_cryptenroll(
            Some(key_file_path),
            &partition1.node,
            BitFlags::from(Pcr::Pcr7),
        )
        .unwrap();

        // Open the encrypted volume, to make the block device available
        cryptsetup_open(key_file_path, &partition1.node, ENCRYPTED_VOLUME_NAME).unwrap();

        // Format the unlocked volume with ext4
        mkfs::run(Path::new(ENCRYPTED_VOLUME_PATH), MkfsFileSystemType::Ext4).unwrap();

        // Mount the encrypted volume
        Dependency::Mount
            .cmd()
            .arg(ENCRYPTED_VOLUME_PATH)
            .arg("/mnt")
            .run_and_check()
            .unwrap();

        // Write a file `test.txt` to the mounted volume
        const TEST_FILE_PATH: &str = "/mnt/test.txt";
        const TEST_FILE_CONTENT: &str = "Hello, world!";
        fs::write(TEST_FILE_PATH, TEST_FILE_CONTENT).unwrap();

        // Verify the file exists
        let test_file_path = Path::new(TEST_FILE_PATH);
        assert!(
            test_file_path.exists(),
            "File `test.txt` should exist on the encrypted volume"
        );

        // Validate the file contents
        let mut file = File::open(TEST_FILE_PATH).expect("Failed to open the test file");
        let mut file_content = String::new();
        file.read_to_string(&mut file_content)
            .expect("Failed to read the test file");
        assert_eq!(
            file_content, TEST_FILE_CONTENT,
            "File contents do not match expected value"
        );

        // Close the file
        drop(file);

        // Unmount the encrypted volume
        Dependency::Umount
            .cmd()
            .arg("/mnt")
            .run_and_check()
            .unwrap();

        // Close the encrypted volume
        cryptsetup_close(ENCRYPTED_VOLUME_NAME).unwrap();

        // Re-open the encrypted volume
        cryptsetup_open(key_file_path, &partition1.node, ENCRYPTED_VOLUME_NAME).unwrap();

        // Re-mount the encrypted volume
        Dependency::Mount
            .cmd()
            .arg(ENCRYPTED_VOLUME_PATH)
            .arg("/mnt")
            .run_and_check()
            .unwrap();

        // Verify that the file still exists
        assert!(
            test_file_path.exists(),
            "File '{TEST_FILE_PATH}' should still exist on the encrypted volume after re-mounting"
        );

        // Validate the file contents
        let mut file = File::open(TEST_FILE_PATH).expect("Failed to open the test file");
        let mut file_content = String::new();
        file.read_to_string(&mut file_content)
            .expect("Failed to read the test file");
        assert_eq!(
            file_content, TEST_FILE_CONTENT,
            "File contents do not match expected value"
        );

        drop(file);

        // Cleanup: Unmount and close the volume
        Dependency::Umount
            .cmd()
            .arg("/mnt")
            .run_and_check()
            .unwrap();

        cryptsetup_close(ENCRYPTED_VOLUME_NAME).unwrap();
    }

    #[functional_test(feature = "helpers")]
    fn test_cryptsetup_reencrypt() {
        // Setup test environment
        setup_test();

        // Create a small partition for testing
        let repart = SystemdRepartInvoker::new(TEST_DISK_DEVICE_PATH, RepartEmptyMode::Force)
            .with_partition_entries(vec![RepartPartitionEntry {
                id: "1".to_string(),
                partition_type: DiscoverablePartitionType::Root,
                label: Some("encrypted_partition".to_string()),
                size_min_bytes: Some(50 * 1048576),  // 50 MiB
                size_max_bytes: Some(100 * 1048576), // 100 MiB
            }]);

        let partition1 = &repart.execute().unwrap()[0];

        // Wait for udev to process pending events
        udevadm::settle().unwrap();

        // Open the partition as a raw block device
        let mut device = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&partition1.node)
            .unwrap();

        // Write known data to a fixed offset
        const TEST_OFFSET: u64 = 4096;
        const TEST_DATA: &[u8] = b"This is test data for `cryptsetup-reencrypt`";
        device.seek(SeekFrom::Start(TEST_OFFSET)).unwrap();
        device.write_all(TEST_DATA).unwrap();

        // Sync data to ensure it's written to the device
        device.sync_all().unwrap();

        // Create a temporary file to store the recovery key file
        let key_file_tmp = NamedTempFile::new().unwrap();
        let key_file_path = key_file_tmp.path();
        fs::set_permissions(key_file_path, Permissions::from_mode(0o600)).unwrap();
        generate_recovery_key_file(key_file_path).unwrap();

        // Re-encrypt the filesystem
        cryptsetup_reencrypt(key_file_path, &partition1.node).unwrap();

        // Run `systemd-cryptenroll` on the partition
        systemd_cryptenroll(
            Some(key_file_path),
            &partition1.node,
            BitFlags::from(Pcr::Pcr7),
        )
        .unwrap();

        // Open the encrypted volume, to make the block device available
        cryptsetup_open(key_file_path, &partition1.node, ENCRYPTED_VOLUME_NAME).unwrap();

        // Verify the test data exists at the expected offset
        let mut decrypted_device = OpenOptions::new()
            .read(true)
            .write(false)
            .open(ENCRYPTED_VOLUME_PATH)
            .unwrap();
        let mut read_data = vec![0u8; TEST_DATA.len()];
        decrypted_device.seek(SeekFrom::Start(TEST_OFFSET)).unwrap();
        decrypted_device.read_exact(&mut read_data).unwrap();

        assert_eq!(
            read_data, TEST_DATA,
            "Decrypted data does not match original data"
        );

        // Close the file descriptor explicitly
        drop(decrypted_device);

        // Close the encrypted volume
        cryptsetup_close(ENCRYPTED_VOLUME_NAME).unwrap();
    }
}
