use std::path::Path;

use anyhow::{Context, Error};

use crate::dependencies::{Dependency, DependencyError};

pub fn check_is_mountpoint(path: impl AsRef<Path>) -> Result<bool, Error> {
    let output = Dependency::Mountpoint
        .cmd()
        .arg(path.as_ref())
        .run_and_check();
    match output {
        Ok(()) => Ok(true),
        Err(e) => {
            if let DependencyError::ExecutionFailed { .. } = *e {
                Ok(false)
            } else {
                Err(e).with_context(|| {
                    format!(
                        "Failed to determine if '{}' is a mount point.",
                        path.as_ref().display()
                    )
                })
            }
        }
    }
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
