use std::{
    collections::HashMap,
    ffi::OsString,
    fmt::Display,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::{error, trace};
use openssl::{
    pkcs7::{Pkcs7, Pkcs7Flags},
    stack::Stack,
    x509::{X509NameEntries, X509},
};

use trident_api::constants::DEV_MAPPER_PATH;

use crate::{
    dependencies::{Dependency, DependencyError},
    lsblk,
};

/// String representing the value expected for the state of a verity device
/// after opening it.
const EXPECTED_VERITY_DEVICE_STATUS: &str = "verified";

/// String representing the value expected for the state of a signed verity
/// device after opening it.
const EXPECTED_VERITY_DEVICE_STATUS_SIGNED: &str = "verified (with signature)";

/// Represents a verity device
/// This struct wraps the open and close behavior of a verity device.
#[derive(Debug, Clone)]
pub struct VerityDevice {
    device_name: String,
    data_device_path: PathBuf,
    hash_device_path: PathBuf,
    root_hash: String,
}

impl VerityDevice {
    /// Create a new VerityDevice instance. The device name is the name of the
    /// verity device that will be created in the /dev/mapper directory.
    pub fn new(
        name: impl Into<String>,
        data_device_path: impl Into<PathBuf>,
        hash_device_path: impl Into<PathBuf>,
        root_hash: impl Into<String>,
    ) -> Self {
        Self {
            device_name: name.into(),
            data_device_path: data_device_path.into(),
            hash_device_path: hash_device_path.into(),
            root_hash: root_hash.into(),
        }
    }

    /// Will attempt to open the device with a signature file and verify it.
    pub fn open_with_signature(&self, signature_file: impl AsRef<Path>) -> Result<(), Error> {
        open_with_signature(
            &self.device_name,
            &self.data_device_path,
            &self.hash_device_path,
            &self.root_hash,
            signature_file,
        )?;

        self.validate_or_close(EXPECTED_VERITY_DEVICE_STATUS_SIGNED)
    }

    /// Will attempt to open the device and verify it.
    pub fn open(&self) -> Result<(), Error> {
        open(
            &self.device_name,
            &self.data_device_path,
            &self.hash_device_path,
            &self.root_hash,
        )?;

        self.validate_or_close(EXPECTED_VERITY_DEVICE_STATUS)
    }

    /// Validates the device status after opening it.
    fn validate_or_close(&self, expected_status: &str) -> Result<(), Error> {
        let dev_status = match self.status() {
            Err(e) => {
                close(&self.device_name)?;
                return Err(e);
            }
            Ok(VerityDeviceStatus::Inactive) => {
                bail!("Verity device '{}' is inactive", self.device_name)
            }
            Ok(VerityDeviceStatus::Active(status)) => *status,
        };

        if dev_status.status != expected_status {
            // The device is not verified, so we need to close it
            // and return an error.
            let mut msg = format!(
                "Failed to activate verity device '{}', status: '{}', expected: '{}'",
                self.device_name, dev_status.status, expected_status
            );

            // Try to close the device, attach the error to the message if it fails.
            if let Err(err) = close(&self.device_name) {
                msg.push_str(&format!(". Also failed to close device: {err:#}"));
            }

            bail!(msg);
        }

        trace!(
            "Successfully opened verity device '{}', status: '{}'",
            self.device_name,
            dev_status.status
        );

        Ok(())
    }

    /// Opens the device and returns a guard that will automatically close the
    /// device when it goes out of scope.
    pub fn open_with_guard(&self) -> Result<VerityDeviceGuard, Error> {
        self.open()?;
        Ok(VerityDeviceGuard::new(self.device_name.clone()))
    }

    /// Retrieves the status of the device.
    pub fn status(&self) -> Result<VerityDeviceStatus, Error> {
        status(&self.device_name)
    }

    /// Returns the full path to the device.
    pub fn device_path(&self) -> PathBuf {
        device_path(&self.device_name)
    }

    /// Returns whether the device is active or not.
    pub fn is_active(&self) -> Result<bool, Error> {
        Ok(self
            .status()
            .context("Failed to determine if device is active")?
            .active()
            .is_some())
    }

    /// Closes the device ONLY if it is active.
    pub fn close(&self) -> Result<(), Error> {
        if !self.is_active()? {
            return Ok(());
        }

        close(&self.device_name)
    }
}

pub struct VerityDeviceGuard {
    device_name: String,
}

impl VerityDeviceGuard {
    pub fn new(device_name: impl Into<String>) -> Self {
        Self {
            device_name: device_name.into(),
        }
    }
}

impl Drop for VerityDeviceGuard {
    fn drop(&mut self) {
        if let Err(e) = close(&self.device_name) {
            error!(
                "Failed to close verity device '{}': {}",
                self.device_name, e
            );
        }
    }
}

/// Low level function to open a verity device.
/// This function will not check the status of the device after opening it.
/// It is the caller's responsibility to check the status of the device
/// after opening it.
/// This function will return an error if the device does not exist or
/// if the device is not a valid verity device.
/// The device name is the name of the verity device that will be created
/// in the /dev/mapper directory.
pub fn open(
    name: impl AsRef<str>,
    data_device_path: impl AsRef<Path>,
    hash_device_path: impl AsRef<Path>,
    root_hash: impl AsRef<str>,
) -> Result<(), Error> {
    open_inner(
        name,
        data_device_path,
        hash_device_path,
        root_hash,
        None::<&Path>,
    )
}

/// Same as open() but adds a parameter to pass a signature file.
fn open_with_signature(
    name: impl AsRef<str>,
    data_device_path: impl AsRef<Path>,
    hash_device_path: impl AsRef<Path>,
    root_hash: impl AsRef<str>,
    signature_file: impl AsRef<Path>,
) -> Result<(), Error> {
    open_inner(
        name,
        data_device_path,
        hash_device_path,
        root_hash,
        Some(signature_file),
    )
}

/// Inner implementation of open() and open_with_signature().
fn open_inner(
    name: impl AsRef<str>,
    data_device_path: impl AsRef<Path>,
    hash_device_path: impl AsRef<Path>,
    root_hash: impl AsRef<str>,
    signature_file: Option<impl AsRef<Path>>,
) -> Result<(), Error> {
    let mut cmd = Dependency::Veritysetup.cmd();
    cmd.arg("open")
        .arg(data_device_path.as_ref())
        .arg(name.as_ref())
        .arg(hash_device_path.as_ref())
        .arg(root_hash.as_ref())
        .arg("--verbose");

    // If a signature file is provided, add it to the command.
    if let Some(signature_file) = signature_file {
        let mut arg = OsString::from("--root-hash-signature=");
        arg.push(signature_file.as_ref());
        cmd.arg(arg);
    }

    cmd.run_and_check()
        .with_context(|| format!("Failed to open verity device '{}'", name.as_ref()))?;

    let dm_verity_root_path = Path::new(DEV_MAPPER_PATH).join(name.as_ref());
    if !dm_verity_root_path.exists() {
        bail!(
            "Verity device '{}' does not exist",
            dm_verity_root_path.display()
        );
    }

    Ok(())
}

/// Low level function to open a verity device and return a guard that will
/// automatically close the device when it goes out of scope.
/// This function is a convenience wrapper around the `open` function.
pub fn open_with_guard(
    name: impl AsRef<str>,
    data_device_path: impl AsRef<Path>,
    hash_device_path: impl AsRef<Path>,
    root_hash: impl AsRef<str>,
) -> Result<VerityDeviceGuard, Error> {
    let device_name = name.as_ref();
    open(device_name, data_device_path, hash_device_path, root_hash)?;
    Ok(VerityDeviceGuard::new(device_name.to_owned()))
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum VerityDeviceStatus {
    Inactive,
    Active(Box<VeritySetupStatus>),
}

impl VerityDeviceStatus {
    /// Unwraps the status if it is active, otherwise returns None.
    pub fn active(self) -> Option<VeritySetupStatus> {
        match self {
            VerityDeviceStatus::Active(status) => Some(*status),
            VerityDeviceStatus::Inactive => None,
        }
    }
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct VeritySetupStatus {
    pub type_: String,
    pub status: String,
    pub hash_type: u8,
    pub data_block_size: u64,
    pub hash_block_size: u64,
    pub hash_name: String,
    pub salt: String,
    pub data_device_path: PathBuf,
    pub size: String,
    pub mode: String,
    pub hash_device_path: PathBuf,
    pub hash_offset: String,
    pub root_hash: String,
    pub flags: Option<String>,
}

impl VeritySetupStatus {
    // Returns an iterator over the members of the verity device. The data
    // device is always the first member, and the hash device is always the
    // second member.
    pub fn members(&self) -> impl Iterator<Item = &Path> {
        [
            self.data_device_path.as_path(),
            self.hash_device_path.as_path(),
        ]
        .into_iter()
    }
}

fn parse_veritysetup_status_output(output: &str) -> Result<VeritySetupStatus, Error> {
    // Skip the first line, which has human readable status
    let lines = output.lines().skip(1);

    let key_values: HashMap<&str, &str> = lines
        .map(|line| {
            let mut parts = line.splitn(2, ':');
            let key = parts
                .next()
                .context(format!(
                    "Missing key in the output of veritysetup status on line '{line}'"
                ))
                .map(|k| k.trim());
            let value = parts
                .next()
                .context(format!(
                    "Missing value in the output of veritysetup status on line '{line}'"
                ))
                .map(|v| v.trim());

            key.and_then(|key| value.map(|value| (key, value)))
        })
        .collect::<Result<HashMap<_, _>, Error>>()?;

    let verity_setup_status = VeritySetupStatus {
        type_: key_values
            .get("type")
            .context("Missing 'type' in the output of veritysetup status")?
            .to_string(),
        status: key_values
            .get("status")
            .context("Missing 'status' in the output of veritysetup status")?
            .to_string(),
        hash_type: key_values
            .get("hash type")
            .context("Missing 'hash type' in the output of veritysetup status")?
            .parse()
            .context(format!(
                "Unable to parse 'hash type' value '{}'",
                key_values
                    .get("hash type")
                    .context("Missing 'hash type' in the output of veritysetup status")?
            ))?,
        data_block_size: key_values
            .get("data block")
            .context("Missing 'data block' in the output of veritysetup status")?
            .parse()
            .context(format!(
                "Unable to parse 'data block' value '{}'",
                key_values
                    .get("data block")
                    .context("Missing 'data block' in the output of veritysetup status")?
            ))?,
        hash_block_size: key_values
            .get("hash block")
            .context("Missing 'hash block' in the output of veritysetup status")?
            .parse()
            .context(format!(
                "Unable to parse 'hash block' value '{}'",
                key_values
                    .get("hash block")
                    .context("Missing 'hash block' in the output of veritysetup status")?
            ))?,
        hash_name: key_values
            .get("hash name")
            .context("Missing 'hash name' in the output of veritysetup status")?
            .to_string(),
        salt: key_values
            .get("salt")
            .context("Missing 'salt' in the output of veritysetup status")?
            .to_string(),
        data_device_path: PathBuf::from(
            key_values
                .get("data device")
                .context("Missing 'data device' in the output of veritysetup status")?,
        ),
        size: key_values
            .get("size")
            .context("Missing 'size' in the output of veritysetup status")?
            .to_string(),
        mode: key_values
            .get("mode")
            .context("Missing 'mode' in the output of veritysetup status")?
            .to_string(),
        hash_device_path: PathBuf::from(
            key_values
                .get("hash device")
                .context("Missing 'hash device' in the output of veritysetup status")?,
        ),
        hash_offset: key_values
            .get("hash offset")
            .context("Missing 'hash offset' in the output of veritysetup status")?
            .to_string(),
        root_hash: key_values
            .get("root hash")
            .context("Missing 'root hash' in the output of veritysetup status")?
            .to_string(),
        flags: key_values.get("flags").map(|s| s.to_string()),
    };

    Ok(verity_setup_status)
}

pub fn status(device_name: impl AsRef<str>) -> Result<VerityDeviceStatus, Error> {
    trace!("Getting status of verity device '{}'", device_name.as_ref());

    // First check if the device is active at all.
    let device_path = device_path(device_name.as_ref());
    if !device_path.exists() {
        trace!(
            "Verity device '{}' does not exist, returning inactive status",
            device_path.display()
        );

        return Ok(VerityDeviceStatus::Inactive);
    }

    let output = Dependency::Veritysetup
        .cmd()
        .arg("status")
        .arg(device_name.as_ref())
        .output_and_check();

    let err = match output {
        Ok(stdout) => {
            return Ok(VerityDeviceStatus::Active(Box::new(
                parse_veritysetup_status_output(&stdout)?,
            )));
        }
        Err(err) => err,
    };

    // Catch the scenario where the device is not active.
    if let DependencyError::ExecutionFailed { code, stdout, .. } = err.as_ref() {
        if code == &Some(4) && stdout.contains("inactive") {
            trace!(
                "Verity device '{}' exists, but verity setup reported it as inactive",
                device_path.display()
            );

            // Device is not active
            return Ok(VerityDeviceStatus::Inactive);
        }
    }

    // If we reach here, some other error occurred.
    Err(err).context(format!(
        "Failed to get status of verity device '{}'",
        device_name.as_ref()
    ))
}

pub fn close(device_name: &str) -> Result<(), Error> {
    let res = Dependency::Veritysetup
        .cmd()
        .arg("close")
        .arg(device_name)
        .arg("--verbose")
        .run_and_check()
        .context(format!("Failed to close verity device '{device_name}'"));

    if let Err(e) = res {
        // If close returns an error, do best effort to log what is holding the
        // block device
        let block_device = lsblk::get(Path::new(DEV_MAPPER_PATH).join(device_name));
        if let Ok(block_device) = block_device {
            error!(
                "Failed to close '{}': active children: {:?}, active mount points: {:?}",
                device_name, block_device.children, block_device.mountpoints
            );
        }

        // Propagate the original unmount error
        return Err(e.context(format!("Failed to close verity device '{device_name}'")));
    }

    Ok(())
}

/// Returns the dev-mapper path for the given device name.
pub fn device_path(name: impl AsRef<Path>) -> PathBuf {
    Path::new(DEV_MAPPER_PATH).join(name)
}

pub struct VeritySignatureInfo(Vec<SignerInfo>);

struct SignerInfo {
    subject_name: String,
    issuer_name: String,
    authority_key_id: Option<Vec<u8>>,
}

impl VeritySignatureInfo {
    fn new(signers: Stack<X509>) -> Self {
        fn extract_data(mut entries: X509NameEntries, default: impl Into<String>) -> String {
            entries
                .next()
                .and_then(|e| e.data().as_utf8().ok())
                .map_or_else(|| default.into(), |s| s.to_string())
        }

        Self(
            signers
                .iter()
                .map(|signer| SignerInfo {
                    subject_name: extract_data(signer.subject_name().entries(), "Unknown"),
                    issuer_name: extract_data(signer.issuer_name().entries(), "Unknown"),
                    authority_key_id: signer.authority_key_id().map(|id| id.as_slice().to_vec()),
                })
                .collect(),
        )
    }
}

impl Display for VeritySignatureInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "VeritySignatureInfo with {} signer(s):", self.0.len())?;
        for signer in self.0.iter() {
            writeln!(
                f,
                "  Subject: {}, Issuer: {}, Authority Key ID: {}",
                signer.subject_name,
                signer.issuer_name,
                signer
                    .authority_key_id
                    .as_ref()
                    .map(hex::encode)
                    .unwrap_or_else(|| "None".to_string())
            )?;
        }

        Ok(())
    }
}

pub fn get_verity_signature_info(path: impl AsRef<Path>) -> Result<VeritySignatureInfo, Error> {
    let der = fs::read(path.as_ref()).context("Failed to read verity signature file")?;
    let pkcs7 = Pkcs7::from_der(&der).context("Failed to parse verity signature file as PKCS#7")?;
    let empty_stack = Stack::<X509>::new().context("Failed to create empty X509 stack")?;
    let signers = pkcs7
        .signers(&empty_stack, Pkcs7Flags::NOVERIFY)
        .context("Failed to get signers from verity signature file")?;

    Ok(VeritySignatureInfo::new(signers))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_verity_setup_status_members() {
        let status = VeritySetupStatus {
            type_: Default::default(),
            status: Default::default(),
            hash_type: Default::default(),
            data_block_size: Default::default(),
            hash_block_size: Default::default(),
            hash_name: Default::default(),
            salt: Default::default(),
            data_device_path: "/dev/sda3".into(),
            size: Default::default(),
            mode: Default::default(),
            hash_device_path: "/dev/sda4".into(),
            hash_offset: Default::default(),
            root_hash: Default::default(),
            flags: Default::default(),
        };

        let members = status.members().collect::<Vec<_>>();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0], Path::new("/dev/sda3"));
        assert_eq!(members[1], Path::new("/dev/sda4"));
    }

    #[test]
    fn test_parse_veritysetup_status_output() {
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        let status = parse_veritysetup_status_output(output).unwrap();

        assert_eq!(status.type_, "VERITY");
        assert_eq!(status.status, "verified");
        assert_eq!(status.hash_type, 1);
        assert_eq!(status.data_block_size, 4096);
        assert_eq!(status.hash_block_size, 4096);
        assert_eq!(status.hash_name, "sha256");
        assert_eq!(
            status.salt,
            "c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62"
        );
        assert_eq!(status.data_device_path, Path::new("/dev/sda3"));
        assert_eq!(status.size, "7567360 sectors");
        assert_eq!(status.mode, "readonly");
        assert_eq!(status.hash_device_path, Path::new("/dev/sda4"));
        assert_eq!(status.hash_offset, "8 sectors");
        assert_eq!(
            status.root_hash,
            "180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a"
        );
        assert_eq!(status.flags, Some("panic_on_corruption".to_string()));

        // fail on missing value
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Missing value in the output of veritysetup status on line '  type'"
        );

        // fail on missing key (though this also showcases as a missing value)
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.

            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Missing value in the output of veritysetup status on line ''"
        );

        // fail non-integer hash type
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1.5
                  data block:  4096
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "invalid digit found in string"
        );

        // fail non-integer data block
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096.5
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "invalid digit found in string"
        );

        // fail non-integer hash block
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096
                  hash block:  4096.5
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "invalid digit found in string"
        );

        // fail on a missing attribute (data device)
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  size:        7567360 sectors
                  mode:        readonly
                  hash device: /dev/sda4
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Missing 'data device' in the output of veritysetup status"
        );

        // fail on a missing attribute (hash device)
        let output = indoc::indoc!(
            r#"
                /dev/mapper/root is active and is in use.
                  type:        VERITY
                  status:      verified
                  hash type:   1
                  data block:  4096
                  hash block:  4096
                  hash name:   sha256
                  salt:        c6ce7430a3ff75757aa1c3367a482e8267c7227d03d77bffc875957da7320b62
                  data device: /dev/sda3
                  size:        7567360 sectors
                  mode:        readonly
                  hash offset: 8 sectors
                  root hash:   180731f6fd8dbab8042c6d4c2a61b4d7de405f2a405ce9a6cd43cef6819aab7a
                  flags:       panic_on_corruption
            "#,
        );

        assert_eq!(
            parse_veritysetup_status_output(output)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Missing 'hash device' in the output of veritysetup status"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;
    use trident_api::constants::MOUNT_OPTION_READ_ONLY;

    use crate::{
        files,
        filesystems::MountFileSystemType,
        mount::{self, MountGuard},
        testutils::{repart::OS_DISK_DEVICE_PATH, verity},
    };

    #[functional_test(feature = "helpers")]
    fn test_open_and_close() {
        let verity_vol = verity::setup_verity_volumes();
        let verity_dev = verity_vol.verity_device("verity-test");

        // bad hash
        {
            let mut bad_hash_dev = verity_dev.clone();
            bad_hash_dev.root_hash = "foobar".to_string();
            let err = bad_hash_dev.open().unwrap_err();
            assert!(
                err.root_cause().to_string().contains("stdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nInvalid root hash string specified.\n\n")
            );
        }

        // Incorrect hash
        {
            let mut bad_hash_dev = verity_dev.clone();
            bad_hash_dev.root_hash =
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef".to_string();
            assert_ne!(
                verity_dev.root_hash, bad_hash_dev.root_hash,
                "Root hash should not match"
            );

            assert_eq!(
                bad_hash_dev.open().unwrap_err().to_string(),
                "Failed to activate verity device 'verity-test', status: 'corrupted', expected: 'verified'"
            );
        }

        let mut expected_status = VeritySetupStatus {
            type_: "VERITY".to_string(),
            status: "verified".to_string(),
            hash_type: 1,
            data_block_size: 4096,
            hash_block_size: 4096,
            hash_name: "sha256".to_string(),
            salt: "".to_string(), // salt is not deterministic
            data_device_path: verity_dev.data_device_path.to_owned(),
            size: "".to_string(), // size is not deterministic
            mode: "readonly".to_string(),
            hash_device_path: verity_dev.hash_device_path.to_owned(),
            hash_offset: "8 sectors".to_string(),
            root_hash: verity_dev.root_hash.clone(),
            flags: None,
        };

        {
            // good hash
            let _guard = verity_dev.open_with_guard().unwrap();

            let mut status = verity_dev
                .status()
                .unwrap()
                .active()
                .expect("Expected verity device to be active");

            status.salt = "".to_string(); // salt is not deterministic
            status.size = "".to_string(); // size is not deterministic
            assert_eq!(status, expected_status);

            {
                let verity_mount_dir = tempfile::tempdir().unwrap();
                mount::mount(
                    Path::new(DEV_MAPPER_PATH).join("verity-test"),
                    verity_mount_dir.path(),
                    MountFileSystemType::Ext4,
                    &[MOUNT_OPTION_READ_ONLY.into()],
                )
                .unwrap();
                // Create a mount guard that will automatically unmount when it goes out of scope
                let _mount_guard = MountGuard {
                    mount_dir: verity_mount_dir.path(),
                };

                // Assert that all expected files exist!
                for file in &verity_vol.file_list {
                    assert!(
                        verity_mount_dir.path().join(file).exists(),
                        "File '{}' does not exist",
                        file.display()
                    );
                }
            }
        }

        // Add a file to the underlying filesystem to induce a corruption
        {
            let root_mount_dir = tempfile::tempdir().unwrap();
            // Mount image to temp dir
            mount::mount(
                &verity_vol.data_volume,
                root_mount_dir.path(),
                MountFileSystemType::Ext4,
                &[],
            )
            .unwrap();

            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: root_mount_dir.path(),
            };

            files::write_file(
                root_mount_dir.path().join("etc/hostname"),
                0o644,
                "verity-test\n".as_bytes(),
            )
            .unwrap();
        };

        {
            // Call open directly to skip the cleanup and verify device is corrupted!
            let _guard = open_with_guard(
                &verity_dev.device_name,
                &verity_dev.data_device_path,
                &verity_dev.hash_device_path,
                &verity_dev.root_hash,
            )
            .unwrap();

            let mut status = verity_dev
                .status()
                .unwrap()
                .active()
                .expect("Expected verity device to be active");

            status.salt = "".to_string(); // salt is not deterministic
            status.size = "".to_string(); // size is not deterministic
            expected_status.status = "corrupted".to_string();
            assert_eq!(status, expected_status);

            // Attempt to mount the corrupted device
            {
                let verity_mount_dir = tempfile::tempdir().unwrap();
                let errstr = mount::mount(
                    Path::new(DEV_MAPPER_PATH).join("verity-test"),
                    verity_mount_dir.path(),
                    MountFileSystemType::Ext4,
                    &[MOUNT_OPTION_READ_ONLY.into()],
                )
                .unwrap_err()
                .root_cause()
                .to_string();

                let expected = format!(
                    "mount: {}: can't read superblock on /dev/mapper/verity-test.",
                    verity_mount_dir.path().display()
                );

                assert!(errstr.contains(&expected));
            }
        }
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_fail_close_on_missing_devices() {
        assert_eq!(
            close("non-existent-device").unwrap_err().to_string(),
            "Failed to close verity device 'non-existent-device'"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_fail_on_missing_or_malformed_devices() {
        // hash device does not contain verity hash tree
        assert!(
            open(
                "foobar",
                "/dev/sda1",
                OS_DISK_DEVICE_PATH,
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string().contains("stdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nDevice /dev/sda is not a valid VERITY device.\n\n")
        );

        // hash device is not a block device
        assert!(
            open(
                "foobar",
                "/dev/sda1",
                "/etc/passwd",
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains("stdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nDevice /etc/passwd is not a valid VERITY device.\n\n")
        );

        let verity_vol = verity::setup_verity_volumes();

        // data device is not a block device
        assert!(
            open(
                "foobar",
                "/etc/passwd",
                &verity_vol.hash_volume,
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains("stdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nInvalid root hash string specified.\n\n")
        );

        // data device does not exist
        assert!(
            open(
                "foobar",
                "/dev/does-not-exist",
                "/etc/passwd",
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains("stdout:\nCommand failed with code -4 (wrong device or file specified).\n\n\nstderr:\nDevice /dev/does-not-exist does not exist or access denied.\n\n")
        );

        // hash device does not exist
        assert!(
            open(
                "foobar",
                "/dev/sda1",
                "/etc/does-not-exist",
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains("stdout:\nCommand failed with code -4 (wrong device or file specified).\n\n\nstderr:\nDevice /etc/does-not-exist does not exist or access denied.\n\n")
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_status_inactive_on_missing_device() {
        assert_eq!(
            status("non-existent-device").unwrap(),
            VerityDeviceStatus::Inactive,
        );
    }
}
