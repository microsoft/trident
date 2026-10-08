//! Helpers for reading hardware identity information exposed by the kernel's
//! DMI (Desktop Management Interface) sysfs tree.

use std::path::Path;

use anyhow::{Context, Error};

use crate::files::read_file_trim;

/// Path to the hardware product UUID exposed by the kernel. kubelet
/// populates a Node's `status.nodeInfo.systemUUID` from this same file, and
/// Trident's tracing metadata uses it as an asset identifier.
pub const PRODUCT_UUID_PATH: &str = "/sys/class/dmi/id/product_uuid";

/// Reads and trims the hardware product UUID from [`PRODUCT_UUID_PATH`].
pub fn read_product_uuid() -> Result<String, Error> {
    read_product_uuid_from(PRODUCT_UUID_PATH)
}

/// Reads and trims the hardware product UUID from an arbitrary path. Exposed
/// (rather than only the [`PRODUCT_UUID_PATH`]-bound [`read_product_uuid`])
/// so callers can inject a fake path in unit tests.
pub fn read_product_uuid_from(path: impl AsRef<Path>) -> Result<String, Error> {
    read_file_trim(&path.as_ref()).with_context(|| {
        format!(
            "Failed to read product UUID from '{}'",
            path.as_ref().display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_trims_product_uuid() {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let path = dir.path().join("product_uuid");
        std::fs::write(&path, "1234-ABCD\n").expect("failed to write test file");

        assert_eq!(read_product_uuid_from(&path).unwrap(), "1234-ABCD");
    }

    #[test]
    fn errors_for_missing_file() {
        assert!(read_product_uuid_from("/nonexistent/product_uuid-for-test").is_err());
    }
}
