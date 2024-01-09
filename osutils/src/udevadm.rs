use std::{path::Path, process::Command};

use anyhow::{Context, Error};

use crate::exe::RunAndCheck;

pub fn settle() -> Result<(), Error> {
    Command::new("udevadm")
        .arg("settle")
        .run_and_check()
        .context("Failed settle udev setup")
}

pub fn trigger() -> Result<(), Error> {
    Command::new("udevadm")
        .arg("trigger")
        .run_and_check()
        .context("Failed trigger udev")
}

pub fn wait(path: &Path) -> Result<(), Error> {
    Command::new("udevadm")
        .arg("wait")
        .arg("--settle")
        .arg("--timeout=120")
        .arg(path)
        .run_and_check()
        .context("Failed wait udev")
}

#[cfg(all(test, feature = "functional-tests"))]
mod functional_tests {
    use super::*;

    #[test]
    fn test() {
        settle().unwrap();
        trigger().unwrap();
        wait(Path::new("/dev/sda")).unwrap();
    }
}
