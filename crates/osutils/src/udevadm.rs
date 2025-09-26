use std::path::Path;

use anyhow::{Context, Error};

use crate::dependencies::Dependency;

pub fn settle() -> Result<(), Error> {
    Dependency::Udevadm
        .cmd()
        .arg("settle")
        .run_and_check()
        .context("Failed settle udev setup")
}

pub fn trigger() -> Result<(), Error> {
    Dependency::Udevadm
        .cmd()
        .arg("trigger")
        .run_and_check()
        .context("Failed trigger udev")
}

pub fn wait(path: &Path) -> Result<(), Error> {
    Dependency::Udevadm
        .cmd()
        .arg("wait")
        .arg("--settle")
        .arg("--timeout=120")
        .arg(path)
        .run_and_check()
        .context("Failed wait udev")
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_settle() {
        settle().unwrap();
    }

    #[functional_test(feature = "helpers")]
    fn test_trigger() {
        trigger().unwrap();
        wait(Path::new("/dev/sda")).unwrap();
    }
}
