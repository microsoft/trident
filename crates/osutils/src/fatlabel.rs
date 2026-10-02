//! Thin wrapper around `fatlabel`, which reads and writes the volume label of a
//! FAT filesystem.
//!
//! `mkfs.vfat` can only set a label at creation time, via `-n`, so this is the
//! way to label a FAT filesystem that already exists.

use std::path::Path;

use anyhow::{bail, ensure, Error};

use crate::dependencies::Dependency;

/// Maximum length of a FAT volume label, in bytes.
///
/// The label occupies a fixed-width field in the FAT directory entry, so the
/// limit is on bytes rather than characters.
pub const MAX_LABEL_LENGTH: usize = 11;

/// Characters `fatlabel` refuses in a volume label.
///
/// Taken from its own diagnostic, which names the set verbatim:
/// `labels with characters *?.,;:/\|+=<>[]" are not allowed`.
const FORBIDDEN_LABEL_CHARS: &[char] = &[
    '*', '?', '.', ',', ';', ':', '/', '\\', '|', '+', '=', '<', '>', '[', ']', '"',
];

/// Returns an error if `label` cannot be stored as a FAT volume label.
///
/// Mirrors what `fatlabel` itself accepts: printable ASCII only, and no longer
/// than the fixed-width on-disk field. Checking up front reports an unusable
/// label against the image it came from, rather than as a `fatlabel` failure.
///
/// Separate from [`set_label`] so the rule can be exercised without invoking
/// `fatlabel`.
fn validate_label(label: &str) -> Result<(), Error> {
    ensure!(
        label.bytes().all(|b| b.is_ascii_graphic() || b == b' '),
        "FAT volume label '{label}' contains characters a FAT filesystem cannot store; only \
        printable ASCII is supported"
    );

    ensure!(
        label.len() <= MAX_LABEL_LENGTH,
        "FAT volume label '{label}' is {} bytes, longer than the {MAX_LABEL_LENGTH} a FAT \
        filesystem can hold",
        label.len()
    );

    if let Some(c) = label.chars().find(|c| FORBIDDEN_LABEL_CHARS.contains(c)) {
        bail!(
            "FAT volume label '{label}' contains the character '{c}', which a FAT volume label \
            cannot hold"
        );
    }

    Ok(())
}

/// Sets the volume label of the FAT filesystem at `device_path`.
///
/// The device may be mounted; the label is written to the boot sector and
/// survives the filesystem being unmounted.
pub fn set_label(device_path: impl AsRef<Path>, label: impl AsRef<str>) -> Result<(), Error> {
    let device_path = device_path.as_ref();
    let label = label.as_ref();

    validate_label(label)?;

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

    use std::{fs::File, path::PathBuf};

    use pytest_gen::functional_test;
    use tempfile::tempdir;

    use crate::{blkid, filesystems::MkfsFileSystemType, mkfs};

    /// Creates a small FAT filesystem in a regular file and returns its path.
    ///
    /// `mkfs.vfat`, `fatlabel` and `blkid` all operate on regular files, so
    /// these tests need no block device -- and must not use one, since the
    /// disks present in the test environment are in use.
    fn make_fat_image(dir: &Path) -> PathBuf {
        let image_path = dir.join("fat.img");
        let image = File::create(&image_path).unwrap();
        image.set_len(8 * 1024 * 1024).unwrap();
        drop(image);

        mkfs::run(&image_path, MkfsFileSystemType::Vfat).unwrap();
        image_path
    }

    #[functional_test(feature = "helpers")]
    fn test_set_label() {
        let dir = tempdir().unwrap();
        let image_path = make_fat_image(dir.path());

        set_label(&image_path, "TESTLABEL").unwrap();
        assert_eq!(
            blkid::get_filesystem_label(&image_path).unwrap(),
            "TESTLABEL"
        );
    }

    /// A label at the maximum length is accepted, and one byte more is not.
    #[functional_test(feature = "helpers")]
    fn test_set_label_length_boundary() {
        let dir = tempdir().unwrap();
        let image_path = make_fat_image(dir.path());

        let longest = "A".repeat(MAX_LABEL_LENGTH);
        set_label(&image_path, &longest).unwrap();
        assert_eq!(blkid::get_filesystem_label(&image_path).unwrap(), longest);

        // Rejected by validate_label, so the filesystem keeps the old label.
        set_label(&image_path, "A".repeat(MAX_LABEL_LENGTH + 1)).unwrap_err();
        assert_eq!(blkid::get_filesystem_label(&image_path).unwrap(), longest);
    }

    /// A label containing a space round-trips unchanged, which it would not if
    /// the label were whitespace-trimmed on the way back out.
    #[functional_test(feature = "helpers")]
    fn test_set_label_preserves_spaces() {
        let dir = tempdir().unwrap();
        let image_path = make_fat_image(dir.path());

        set_label(&image_path, "A B").unwrap();
        assert_eq!(blkid::get_filesystem_label(&image_path).unwrap(), "A B");
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_set_label_too_long() {
        let dir = tempdir().unwrap();
        let image_path = make_fat_image(dir.path());

        set_label(&image_path, "THIS-LABEL-IS-TOO-LONG").unwrap_err();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_label_rejects_overlong_label() {
        // 12 bytes; fatlabel reports "labels can be no longer than 11 characters".
        let err = validate_label("ABCDEFGHIJKL").unwrap_err();
        assert!(err.to_string().contains("longer than"), "got: {err}");
    }

    #[test]
    fn test_validate_label_accepts_the_maximum_length() {
        // "ABCDEFGHIJK" sits exactly on the limit; the conventional ESP label
        // is one byte inside it.
        assert_eq!("ABCDEFGHIJK".len(), MAX_LABEL_LENGTH);
        validate_label("ABCDEFGHIJK").unwrap();
        validate_label("EFI-SYSTEM").unwrap();
    }

    #[test]
    fn test_validate_label_rejects_forbidden_characters() {
        // fatlabel names this set verbatim: *?.,;:/\|+=<>[]"
        for c in FORBIDDEN_LABEL_CHARS {
            let label = format!("A{c}B");
            let err = validate_label(&label).unwrap_err();
            assert!(
                err.to_string().contains("cannot hold"),
                "'{c}' should be rejected, got: {err}"
            );
        }
    }

    #[test]
    fn test_validate_label_allows_spaces() {
        // Legal in a FAT label, and must survive validation so it can survive
        // the round trip through blkid.
        validate_label("A B").unwrap();
    }

    #[test]
    fn test_validate_label_limit_is_bytes_not_characters() {
        // 11 characters but 22 bytes. A character-based check would accept this,
        // and fatlabel would then refuse it.
        let label = "\u{c4}\u{c4}\u{c4}\u{c4}\u{c4}\u{c4}\u{c4}\u{c4}\u{c4}\u{c4}\u{c4}";
        assert_eq!(label.chars().count(), MAX_LABEL_LENGTH);
        assert!(label.len() > MAX_LABEL_LENGTH);
        validate_label(label).unwrap_err();
    }

    #[test]
    fn test_validate_label_rejects_non_ascii() {
        // Rejected on content, not length: fatlabel refuses non-ASCII outright.
        let err = validate_label("\u{c4}").unwrap_err();
        assert!(err.to_string().contains("printable ASCII"), "got: {err}");
    }

    #[test]
    fn test_validate_label_rejects_control_characters() {
        validate_label("a\tb").unwrap_err();
    }
}
