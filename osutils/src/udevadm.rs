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

#[cfg(feature = "functional-tests")]
mod functional_tests {
    #[cfg(test)]
    use super::*;
    use pytest_gen::pytest;

    #[pytest(feature = "helpers")]
    fn test_settle() {
        settle().unwrap();
    }

    #[pytest(feature = "helpers")]
    fn test_trigger() {
        trigger().unwrap();
        wait(Path::new("/dev/sda")).unwrap();
    }
}
