use std::{path::Path, process::Command};

use anyhow::{Context, Error};
use uuid::Uuid;

use crate::exe::RunAndCheck;

fn run(device_path: impl AsRef<Path>, tag: &str) -> Result<String, Error> {
    let output = Command::new("blkid")
        .arg("-o") // output format
        .arg("value") // single value
        .arg("-s") // tag
        .arg(tag)
        .arg(device_path.as_ref())
        .output_and_check()
        .context("Failed to execute blkid")?;

    Ok(output.trim().to_owned())
}

fn get_filesystem_uuid_raw(device_path: impl AsRef<Path>) -> Result<String, Error> {
    run(device_path, "UUID")
}

pub fn get_filesystem_uuid(device_path: impl AsRef<Path>) -> Result<Uuid, Error> {
    let output = get_filesystem_uuid_raw(&device_path)?;
    Uuid::parse_str(output.as_str()).context(format!(
        "Failed to get UUID for path '{}', received '{}'",
        device_path.as_ref().display(),
        output
    ))
}

pub fn get_partition_label(device_path: impl AsRef<Path>) -> Result<String, Error> {
    run(device_path, "PARTLABEL")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use uuid::Uuid;

    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_run_success() {
        let partlabel = super::run(Path::new("/dev/sda1"), "PARTLABEL").unwrap();
        assert_eq!(partlabel, "esp");

        let uuid = super::run(Path::new("/dev/sda2"), "UUID").unwrap();
        Uuid::parse_str(&uuid).unwrap();

        let partlabel = super::run(Path::new("/dev/sda2"), "PARTLABEL").unwrap();
        assert_eq!(partlabel, "root-a");

        assert_eq!(super::run(Path::new("/dev/sda1"), "UUID").unwrap().len(), 9);
        // e.g. 8AA2-EE49
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_run_fail_on_non_block_file() {
        assert_eq!(
            super::run(Path::new("/dev/null"), "PARTLABEL")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "(No output was captured)"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_run_fail_on_missing_file() {
        assert_eq!(
            super::run(Path::new("/dev/does-not-exist"), "PARTLABEL")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "(No output was captured)"
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_get_filesystem_uuid_raw_success() {
        let uuid = super::get_filesystem_uuid_raw(Path::new("/dev/sda2")).unwrap();
        Uuid::parse_str(&uuid).unwrap();
        assert_eq!(super::run(Path::new("/dev/sda2"), "UUID").unwrap(), uuid);
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_get_filesystem_uuid_raw_fail_on_missing_file() {
        assert_eq!(
            super::get_filesystem_uuid_raw(Path::new("/dev/does-not-exist"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "(No output was captured)"
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_get_filesystem_uuid_success() {
        let uuid = super::get_filesystem_uuid(Path::new("/dev/sda2")).unwrap();
        assert_eq!(
            super::run(Path::new("/dev/sda2"), "UUID").unwrap(),
            uuid.to_string()
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_get_filesystem_uuid_fail_on_missing_file() {
        assert_eq!(
            super::get_filesystem_uuid(Path::new("/dev/does-not-exist"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "(No output was captured)"
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_get_filesystem_uuid_fail_on_vfat() {
        assert_eq!(
            super::get_filesystem_uuid(Path::new("/dev/sda1"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "invalid group count: expected 5, found 2"
        );
    }

    #[functional_test(feature = "helpers")]
    fn test_get_partition_label_success() {
        let partlabel = super::get_partition_label(Path::new("/dev/sda1")).unwrap();
        assert_eq!(partlabel, "esp");
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_get_partition_label_fail_on_missing_file() {
        assert_eq!(
            super::get_partition_label(Path::new("/dev/does-not-exist"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "(No output was captured)"
        );
    }
}
