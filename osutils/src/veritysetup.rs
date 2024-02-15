use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Error};

use crate::exe::RunAndCheck;

pub fn open(
    data_device_path: impl AsRef<Path>,
    device_name: &str,
    hash_device_name: impl AsRef<Path>,
    root_hash: &str,
) -> Result<(), Error> {
    Command::new("veritysetup")
        .arg("open")
        .arg(data_device_path.as_ref())
        .arg(device_name)
        .arg(hash_device_name.as_ref())
        .arg(root_hash)
        .arg("--verbose")
        .run_and_check()
        .context(format!("Failed to open verity device {}", device_name))?;
    let dm_verity_root_path = Path::new("/dev/mapper").join(device_name);
    if !dm_verity_root_path.exists() {
        bail!(
            "Verity device {} does not exist",
            dm_verity_root_path.display()
        );
    }

    Ok(())
}

pub fn is_present() -> Result<(), Error> {
    Command::new("veritysetup")
        .arg("--version")
        .run_and_check()
        .context("Failed to check veritysetup presence")
}

#[derive(Debug)]
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
    pub flags: String,
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
        flags: key_values
            .get("flags")
            .context("Missing 'flags' in the output of veritysetup status")?
            .to_string(),
    };

    Ok(verity_setup_status)
}

pub fn status(device_name: &str) -> Result<VeritySetupStatus, Error> {
    let output = Command::new("veritysetup")
        .arg("status")
        .arg(device_name)
        .arg("--verbose")
        .output_and_check()
        .context(format!(
            "Failed to get status of verity device {device_name}",
        ))?;

    parse_veritysetup_status_output(output.as_str())
}

pub fn close(device_name: &str) -> Result<(), Error> {
    Command::new("veritysetup")
        .arg("close")
        .arg(device_name)
        .arg("--verbose")
        .run_and_check()
        .context(format!("Failed to close verity device {}", device_name))
}

#[cfg(test)]
mod test {
    use super::*;

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
        assert_eq!(status.flags, "panic_on_corruption");

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

    #[functional_test(feature = "helpers")]
    fn test_open_and_close() {
        // TODO, need to have a valid data volume and verity hash volume to
        // validate
        // Tracked by:
        // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6756

        // TODO add more negative cases around validating incorrect hash
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_fail_close_on_missing_devices() {
        assert_eq!(
            close("non-existent-device").unwrap_err().to_string(),
            "Failed to close verity device non-existent-device"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_fail_on_missing_or_malformed_devices() {
        // hash device does not contain verity hash tree
        assert_eq!(
            open(
                Path::new("/dev/sda1"),
                "foobar",
                Path::new("/dev/sdb"),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nDevice /dev/sdb is not a valid VERITY device.\n\n"
        );

        // hash device is not a block device
        assert_eq!(
            open(
                Path::new("/dev/sda1"),
                "foobar",
                Path::new("/etc/passwd"),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nDevice /etc/passwd is not a valid VERITY device.\n\n"
        );

        // TODO use proper verity device here
        // data device is not a block device
        assert_eq!(
            open(
                Path::new("/etc/passwd"),
                "foobar",
                Path::new("/dev/sdb"),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -1 (wrong or missing parameters).\n\n\nstderr:\nDevice /dev/sdb is not a valid VERITY device.\n\n"
        );

        // data device does not exist
        assert_eq!(
            open(
                Path::new("/dev/does-not-exist"),
                "foobar",
                Path::new("/etc/passwd"),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -4 (wrong device or file specified).\n\n\nstderr:\nDevice /dev/does-not-exist does not exist or access denied.\n\n"
        );

        // hash device does not exist
        assert_eq!(
            open(
                Path::new("/dev/sda1"),
                "foobar",
                Path::new("/etc/does-not-exist"),
                "foobar",
            )
            .unwrap_err()
            .root_cause()
            .to_string(),
            "Process output:\nstdout:\nCommand failed with code -4 (wrong device or file specified).\n\n\nstderr:\nDevice /etc/does-not-exist does not exist or access denied.\n\n"
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_is_present() {
        is_present().unwrap();
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_status_fail_on_missing_device() {
        assert_eq!(
            status("non-existent-device").unwrap_err().to_string(),
            "Failed to get status of verity device non-existent-device"
        );
    }
}
