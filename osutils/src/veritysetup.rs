use std::{path::Path, process::Command};

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

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_open() {
        // TODO, need to have a valid data volume and verity hash volume to
        // validate
        // Tracked by:
        // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6756

        // TODO add more negative cases around validating incorrect hash
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
            "Process output:\nstderr:\nDevice /dev/sdb is not a valid VERITY device.\n\n"
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
            "Process output:\nstderr:\nDevice /etc/passwd is not a valid VERITY device.\n\n"
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
            "Process output:\nstderr:\nDevice /dev/sdb is not a valid VERITY device.\n\n"
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
            "Process output:\nstderr:\nDevice /dev/does-not-exist does not exist or access denied.\n\n"
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
            "Process output:\nstderr:\nDevice /etc/does-not-exist does not exist or access denied.\n\n"
        );
    }
}
