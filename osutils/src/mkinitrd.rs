use std::process::Command;

use trident_api::error::{ManagementError, ReportError, TridentError};

use crate::exe::RunAndCheck;

/// Execute mkinitrd wrapper script of dracut, to generate initrd with the
/// default configuration
pub fn execute() -> Result<(), TridentError> {
    Command::new("mkinitrd")
        .run_and_check()
        .structured(ManagementError::RegenerateInitrd)
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    #[functional_test]
    fn test_regenerate_initrd() {
        let initrd_path = glob::glob("/boot/initrd.img-*").unwrap().next();
        let original = &initrd_path;
        if let Some(initrd_path) = &initrd_path {
            std::fs::remove_file(initrd_path.as_ref().unwrap()).unwrap();
        }

        execute().unwrap();

        // some should have been created
        let initrd_path = glob::glob("/boot/initrd.img-*").unwrap().next();
        assert!(initrd_path.is_some());

        // and the filename should match the original, if we can find t
        // original; making it conditional in case it was missing in the first
        // place, possibly due to failure in a test that makes changes to the initrd
        if let Some(original) = original {
            let initrd_path = initrd_path.unwrap().unwrap();
            assert_eq!(original.as_ref().unwrap(), &initrd_path);
        }
    }
}
