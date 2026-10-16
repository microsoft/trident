//! Helpers for reading hardware identity information exposed by the kernel's
//! DMI (Desktop Management Interface) sysfs tree.

use std::path::Path;

use anyhow::{Context, Error};

use sysdefs::osuuid::OsUuid;

use crate::files;

/// Path to the hardware product UUID exposed by the kernel. kubelet
/// populates a Node's `status.nodeInfo.systemUUID` from this same file, and
/// Trident's tracing metadata uses it as an asset identifier.
pub const PRODUCT_UUID_PATH: &str = "/sys/class/dmi/id/product_uuid";

/// Reads and trims the hardware product UUID from [`PRODUCT_UUID_PATH`].
/// Canonicalizes a well-formed UUID (e.g. lowercases its hex digits) - use
/// this for typed comparisons (e.g. against Kubernetes systemUUID), but see
/// [`read_product_uuid_raw`] for contexts where that canonicalization itself
/// would be an unwanted behavior change.
pub fn read_product_uuid() -> Result<OsUuid, Error> {
    read_uuid_from(PRODUCT_UUID_PATH)
}

/// Reads and trims the hardware product UUID from [`PRODUCT_UUID_PATH`] as
/// its original raw text, preserving casing verbatim. Unlike
/// [`read_product_uuid`], this does not parse/canonicalize the value - use
/// it for display-only/telemetry contexts (e.g. `trident`'s tracestream
/// `asset_id`) where changing an existing uppercase UUID's case across an
/// upgrade would silently change that identifier's representation.
pub fn read_product_uuid_raw() -> Result<String, Error> {
    read_raw_from(PRODUCT_UUID_PATH)
}

fn read_uuid_from(path: impl AsRef<Path>) -> Result<OsUuid, Error> {
    read_raw_from(path).map(OsUuid::from)
}

fn read_raw_from(path: impl AsRef<Path>) -> Result<String, Error> {
    files::read_file_trim(&path.as_ref())
        .with_context(|| format!("Failed to read UUID from '{}'", path.as_ref().display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    #[test]
    fn reads_and_trims_product_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("product_uuid");
        for value in ["1234-ABCD", "6BA7B810-9DAD-11D1-80B4-00C04FD430C8", ""] {
            fs::write(&path, format!("  {value}\n")).unwrap();
            assert_eq!(read_uuid_from(&path).unwrap(), OsUuid::from(value));
        }
    }

    #[test]
    fn reads_and_trims_raw_product_uuid_preserving_case() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("product_uuid");
        // An uppercase, well-formed UUID must come back verbatim (not
        // lowercased), unlike read_uuid_from/OsUuid's canonicalized Display.
        fs::write(&path, "  6BA7B810-9DAD-11D1-80B4-00C04FD430C8\n").unwrap();
        assert_eq!(
            read_raw_from(&path).unwrap(),
            "6BA7B810-9DAD-11D1-80B4-00C04FD430C8"
        );
    }

    #[test]
    fn errors_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing");
        let error = read_uuid_from(&path).unwrap_err();
        assert!(
            error.to_string().contains(&path.display().to_string()),
            "{error:#}"
        );
    }
}
