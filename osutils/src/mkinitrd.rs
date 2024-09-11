use std::{path::Path, process::Command};

use trident_api::error::{ReportError, ServicingError, TridentError};

use crate::exe::RunAndCheck;

/// Generate a new initrd image using either mkinitrd or dracut.
///
/// If mkinitrd is available, it will be used. Azl 3.0 doesn't have mkinitrd anymore, so dracut is
/// used instead.
pub fn execute() -> Result<(), TridentError> {
    if Path::new("/usr/bin/mkinitrd").exists() {
        Command::new("mkinitrd")
            .run_and_check()
            .structured(ServicingError::RegenerateInitrd)
    } else {
        Command::new("dracut")
            .arg("--force")
            .arg("--regenerate-all")
            .arg("--zstd")
            .run_and_check()
            .structured(ServicingError::RegenerateInitrd)
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    use crate::osrelease;

    #[functional_test]
    fn test_regenerate_initrd() {
        let pattern = if osrelease::is_azl3().unwrap() {
            "/boot/initramfs-*.azl3.img"
        } else {
            "/boot/initrd.img-*"
        };

        let initrd_path = glob::glob(pattern).unwrap().next();
        let original = &initrd_path;
        if let Some(initrd_path) = &initrd_path {
            std::fs::remove_file(initrd_path.as_ref().unwrap()).unwrap();
        }

        execute().unwrap();

        // Some initrd should have been created
        let initrd_path = glob::glob(pattern).unwrap().next();
        assert!(initrd_path.is_some());

        // And the filename should match the original, if it previously existed
        if let Some(original) = original {
            let initrd_path = initrd_path.unwrap().unwrap();
            assert_eq!(original.as_ref().unwrap(), &initrd_path);
        }
    }
}
