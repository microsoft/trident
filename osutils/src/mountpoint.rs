use std::{path::Path, process::Command};

use anyhow::{Context, Error};

pub fn check_is_mountpoint(path: impl AsRef<Path>) -> Result<bool, Error> {
    let output = Command::new("mountpoint")
        .arg(path.as_ref())
        .output()
        .context("Failed to execute mountpoint")?;

    Ok(output.status.success())
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    #[functional_test(feature = "helpers")]
    fn test_check_is_mountpoint() {
        assert!(!super::check_is_mountpoint(Path::new("/dev/sda1")).unwrap());

        assert!(super::check_is_mountpoint(Path::new("/")).unwrap());

        assert!(!super::check_is_mountpoint(Path::new("/etc")).unwrap());

        assert!(!super::check_is_mountpoint(Path::new("/does-not-exist")).unwrap());
    }
}
