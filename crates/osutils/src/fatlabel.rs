//! Thin wrapper around `fatlabel`, which reads and writes the volume label of a
//! FAT filesystem.
//!
//! `mkfs.vfat` can only set a label at creation time, via `-n`, so this is the
//! way to label a FAT filesystem that already exists.

use std::path::Path;

use anyhow::{ensure, Error};

use crate::dependencies::Dependency;

/// Maximum length of a FAT volume label, in characters, as enforced by
/// `fatlabel` itself.
pub const MAX_LABEL_LENGTH: usize = 11;

/// Sets the volume label of the FAT filesystem at `device_path`.
///
/// The device may be mounted; the label is written to the boot sector and
/// survives the filesystem being unmounted.
pub fn set_label(device_path: impl AsRef<Path>, label: impl AsRef<str>) -> Result<(), Error> {
    let device_path = device_path.as_ref();
    let label = label.as_ref();

    ensure!(
        label.chars().count() <= MAX_LABEL_LENGTH,
        "FAT volume label '{label}' is longer than the {MAX_LABEL_LENGTH} characters a FAT \
        filesystem can hold"
    );

    Dependency::Fatlabel
        .cmd()
        .arg(device_path)
        .arg(label)
        .run_and_check()
        .map_err(Error::from)
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use pytest_gen::functional_test;

    use crate::{blkid, filesystems::MkfsFileSystemType, mkfs};

    #[functional_test(feature = "helpers")]
    fn test_set_label() {
        let device = Path::new("/dev/sda1");
        mkfs::run(device, MkfsFileSystemType::Vfat).unwrap();

        set_label(device, "TESTLABEL").unwrap();
        assert_eq!(blkid::get_filesystem_label(device).unwrap(), "TESTLABEL");
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_set_label_too_long() {
        set_label(Path::new("/dev/sda1"), "THIS-LABEL-IS-TOO-LONG").unwrap_err();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_label_rejects_overlong_label() {
        // Rejected before invoking fatlabel, so this needs no filesystem.
        let err = set_label(Path::new("/dev/null"), "THIS-LABEL-IS-TOO-LONG").unwrap_err();
        assert!(
            err.to_string().contains("longer than"),
            "got: {}",
            err.to_string()
        );
    }

    #[test]
    fn test_max_label_length_is_accepted_by_the_length_check() {
        // 11 characters, e.g. the conventional ESP label, must not be rejected
        // by the length check. (The call itself will fail on /dev/null, which
        // is not a FAT filesystem, so only the check is exercised here.)
        let label = "EFI-SYSTEM!";
        assert_eq!(label.chars().count(), MAX_LABEL_LENGTH);
        let err = set_label(Path::new("/dev/null"), label).unwrap_err();
        assert!(
            !err.to_string().contains("longer than"),
            "length check should have passed, got: {}",
            err.to_string()
        );
    }
}
