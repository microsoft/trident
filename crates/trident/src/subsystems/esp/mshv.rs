//! ACL, efitreeloader (ETL), and MSHV-related functionality for ESP provisioning.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use log::{debug, warn};

/// Filename of the second-stage EFI executable for efitreeloader (ETL).
pub(super) const ETL_SECOND_STAGE_EFI_FILENAME: &str = "second.efi";

/// List of top-level MSHV ESP payload binaries that need to be copied.
const MSHV_ESP_PAYLOAD_BINARIES: [&str; 2] = ["efi/HvLoader.efi", "efi/lxhvloader.dll"];

/// Optional ESP-relative directory containing secondary MSHV payload binaries.
/// Contents vary between images, so discover its direct files instead of
/// maintaining a fixed binary list. Subdirectories are ignored.
const MSHV_ESP_PAYLOAD_DIR: &str = "efi/Windows/System32";

/// Copies the MSHV ESP payload files from the source ESP to the destination ESP
/// for a clean install.
///
/// Specifically, it copies the top-level MSHV ESP payload binaries listed in
/// `MSHV_ESP_PAYLOAD_BINARIES` from the source ESP to the destination ESP, and
/// then copies any and all secondary MSHV ESP payload binaries from the
/// `MSHV_ESP_PAYLOAD_DIR` directory directly, preserving ESP-relative paths.
/// Subdirectories are ignored.
/// An absent secondary payload directory is allowed.
pub(super) fn copy_mshv_esp_payload(
    source_esp: &Path,
    destination_esp: &Path,
) -> Result<(), Error> {
    // List of relative paths to copy from the source ESP to the destination ESP.
    let mut files_to_copy: Vec<PathBuf> = Vec::new();

    // Validate and discover the payload before writing, so missing required files
    // or traversal errors do not leave a partially copied payload.
    for relative_path in MSHV_ESP_PAYLOAD_BINARIES {
        let source_path = source_esp.join(relative_path);
        ensure!(
            source_path.is_file(),
            "Required MSHV ESP payload file '{relative_path}' is missing from the source ESP"
        );
        files_to_copy.push(relative_path.into());
    }

    let payload_dir = source_esp.join(MSHV_ESP_PAYLOAD_DIR);
    if payload_dir.try_exists().with_context(|| {
        format!(
            "Failed to inspect MSHV payload directory '{}'",
            payload_dir.display()
        )
    })? {
        for entry in fs::read_dir(&payload_dir).with_context(|| {
            format!(
                "Failed to read MSHV payload directory '{}'",
                payload_dir.display()
            )
        })? {
            let entry = entry
                .with_context(|| format!("Failed to read entry in '{}'", payload_dir.display()))?;
            let file_type = entry.file_type().with_context(|| {
                format!(
                    "Failed to inspect MSHV payload entry '{}'",
                    entry.path().display()
                )
            })?;
            if file_type.is_dir() {
                warn!(
                    "Ignoring unexpected directory '{}' in MSHV ESP payload",
                    entry.path().display()
                );
                continue;
            }
            // Reject symlinks and special files rather than following
            // references outside the payload directory.
            ensure!(
                file_type.is_file(),
                "MSHV payload entry '{}' is not a regular file or directory",
                entry.path().display()
            );
            files_to_copy.push(Path::new(MSHV_ESP_PAYLOAD_DIR).join(entry.file_name()));
        }
    }

    for relative_path in files_to_copy {
        let source_path = source_esp.join(&relative_path);
        let destination_path = destination_esp.join(&relative_path);
        let parent = destination_path.parent().with_context(|| {
            format!(
                "Failed to get parent directory for '{}'",
                destination_path.display()
            )
        })?;
        fs::create_dir_all(parent).with_context(|| {
            format!(
                "Failed to create MSHV ESP payload directory '{}'",
                parent.display()
            )
        })?;
        debug!(
            "Copying MSHV ESP payload file '{}' to '{}'",
            source_path.display(),
            destination_path.display()
        );
        fs::copy(&source_path, &destination_path).with_context(|| {
            format!(
                "Failed to copy MSHV ESP payload file '{}' to '{}'",
                source_path.display(),
                destination_path.display()
            )
        })?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs as unix_fs},
    };

    use nix::{sys::stat::Mode, unistd};
    use tempfile::TempDir;

    struct EspFixture {
        source: TempDir,
        destination: TempDir,
    }

    impl EspFixture {
        fn new() -> Self {
            let fixture = Self {
                source: TempDir::new().unwrap(),
                destination: TempDir::new().unwrap(),
            };
            for relative_path in MSHV_ESP_PAYLOAD_BINARIES {
                fixture.write_source(relative_path, relative_path.as_bytes());
            }
            fixture
        }

        fn write_source(&self, relative_path: impl AsRef<Path>, contents: &[u8]) {
            let path = self.source.path().join(relative_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }

        fn payload_dir(&self) -> PathBuf {
            self.source.path().join(MSHV_ESP_PAYLOAD_DIR)
        }

        fn copy(&self) -> Result<(), Error> {
            copy_mshv_esp_payload(self.source.path(), self.destination.path())
        }

        fn assert_required_binaries_copied(&self) {
            for relative_path in MSHV_ESP_PAYLOAD_BINARIES {
                assert_eq!(
                    fs::read(self.destination.path().join(relative_path)).unwrap(),
                    relative_path.as_bytes()
                );
                assert_eq!(
                    fs::read(self.source.path().join(relative_path)).unwrap(),
                    relative_path.as_bytes()
                );
            }
        }

        fn assert_rejected_before_copy(&self, expected_message: &str) {
            let sentinel = self.destination.path().join("unrelated.efi");
            fs::write(&sentinel, b"preserved").unwrap();

            let error = self.copy().unwrap_err();
            assert!(
                format!("{error:#}").contains(expected_message),
                "unexpected error: {error:#}"
            );
            assert_eq!(fs::read(sentinel).unwrap(), b"preserved");
            assert_eq!(fs::read_dir(self.destination.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn copies_required_binaries_without_secondary_payload_directory() {
        let fixture = EspFixture::new();

        fixture.copy().unwrap();

        fixture.assert_required_binaries_copied();
        assert!(!fixture
            .destination
            .path()
            .join(MSHV_ESP_PAYLOAD_DIR)
            .exists());
    }

    #[test]
    fn accepts_empty_secondary_payload_directory() {
        let fixture = EspFixture::new();
        fs::create_dir_all(fixture.payload_dir()).unwrap();

        fixture.copy().unwrap();

        fixture.assert_required_binaries_copied();
        assert!(!fixture
            .destination
            .path()
            .join(MSHV_ESP_PAYLOAD_DIR)
            .exists());
    }

    #[test]
    fn copies_all_direct_files_preserving_paths_and_binary_contents() {
        let fixture = EspFixture::new();
        let payload_files = [
            ("hvix64.exe", &[0, 0xff, 1, 0x80][..]),
            ("hvax64.exe", &[2, 0xfe, 3][..]),
            ("kdstub.dll", &[4, 5, 0][..]),
            ("additional.EFI", &[6, 0xfd][..]),
            (".hidden", &[7, 8][..]),
            ("no-extension", &[9][..]),
            ("empty.dll", &[][..]),
        ];
        for (filename, contents) in payload_files {
            fixture.write_source(Path::new(MSHV_ESP_PAYLOAD_DIR).join(filename), contents);
        }

        fixture.copy().unwrap();

        fixture.assert_required_binaries_copied();
        for (filename, contents) in payload_files {
            let relative_path = Path::new(MSHV_ESP_PAYLOAD_DIR).join(filename);
            assert_eq!(
                fs::read(fixture.destination.path().join(&relative_path)).unwrap(),
                contents
            );
            assert_eq!(
                fs::read(fixture.source.path().join(relative_path)).unwrap(),
                contents
            );
            assert!(!fixture.destination.path().join(filename).exists());
        }
        assert_eq!(
            fs::read_dir(fixture.destination.path().join(MSHV_ESP_PAYLOAD_DIR))
                .unwrap()
                .count(),
            payload_files.len()
        );
    }

    #[test]
    fn preserves_non_utf8_filenames() {
        let fixture = EspFixture::new();
        let filename = OsString::from_vec(b"payload-\xff.dll".to_vec());
        let relative_path = Path::new(MSHV_ESP_PAYLOAD_DIR).join(filename);
        fixture.write_source(&relative_path, b"binary");

        fixture.copy().unwrap();

        assert_eq!(
            fs::read(fixture.destination.path().join(relative_path)).unwrap(),
            b"binary"
        );
    }

    #[test]
    fn ignores_nested_and_empty_directories() {
        let fixture = EspFixture::new();
        fixture.write_source(
            Path::new(MSHV_ESP_PAYLOAD_DIR).join("direct.dll"),
            b"direct",
        );
        fixture.write_source(
            Path::new(MSHV_ESP_PAYLOAD_DIR).join("drivers/nested/hidden.dll"),
            b"nested",
        );
        fs::create_dir_all(fixture.payload_dir().join("empty")).unwrap();

        fixture.copy().unwrap();

        fixture.assert_required_binaries_copied();
        let destination_payload = fixture.destination.path().join(MSHV_ESP_PAYLOAD_DIR);
        assert_eq!(
            fs::read(destination_payload.join("direct.dll")).unwrap(),
            b"direct"
        );
        assert!(!destination_payload.join("drivers").exists());
        assert!(!destination_payload.join("empty").exists());
        assert_eq!(fs::read_dir(destination_payload).unwrap().count(), 1);
    }

    #[test]
    fn preserves_unrelated_source_and_destination_files() {
        let fixture = EspFixture::new();
        fixture.write_source("efi/unrelated.efi", b"source-only");
        fixture.write_source("efi/BOOT/second.efi", b"second-stage");
        let managed_files = [
            "EFI/AZLA/bootx64.efi",
            "EFI/Linux/trident.efi",
            "EFI/BOOT/bootx64.efi",
            "efi/Windows/System32/existing.dll",
            "efi/Windows/System32/drivers/existing.dll",
        ];
        for relative_path in managed_files {
            let path = fixture.destination.path().join(relative_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"managed").unwrap();
        }

        fixture.copy().unwrap();

        fixture.assert_required_binaries_copied();
        for relative_path in managed_files {
            assert_eq!(
                fs::read(fixture.destination.path().join(relative_path)).unwrap(),
                b"managed"
            );
        }
        for relative_path in ["efi/unrelated.efi", "efi/BOOT/second.efi"] {
            assert!(!fixture.destination.path().join(relative_path).exists());
            assert!(fixture.source.path().join(relative_path).is_file());
        }
    }

    #[test]
    fn overwrites_existing_payload_files_and_truncates_old_contents() {
        let fixture = EspFixture::new();
        let secondary_path = Path::new(MSHV_ESP_PAYLOAD_DIR).join("additional.dll");
        fixture.write_source(&secondary_path, b"new");
        let mut relative_paths: Vec<PathBuf> = MSHV_ESP_PAYLOAD_BINARIES
            .into_iter()
            .map(PathBuf::from)
            .collect();
        relative_paths.push(secondary_path.clone());
        for relative_path in relative_paths {
            let path = fixture.destination.path().join(relative_path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"old contents that are longer than the replacement").unwrap();
        }

        fixture.copy().unwrap();
        fixture.copy().unwrap();

        fixture.assert_required_binaries_copied();
        assert_eq!(
            fs::read(fixture.destination.path().join(secondary_path)).unwrap(),
            b"new"
        );
    }

    #[test]
    fn creates_missing_destination_root_and_parent_directories() {
        let fixture = EspFixture::new();
        let destination = fixture.destination.path().join("new/esp");
        let relative_path = Path::new(MSHV_ESP_PAYLOAD_DIR).join("secondary.dll");
        fixture.write_source(&relative_path, b"secondary");

        copy_mshv_esp_payload(fixture.source.path(), &destination).unwrap();

        for required in MSHV_ESP_PAYLOAD_BINARIES {
            assert_eq!(
                fs::read(destination.join(required)).unwrap(),
                required.as_bytes()
            );
        }
        assert_eq!(
            fs::read(destination.join(relative_path)).unwrap(),
            b"secondary"
        );
    }

    #[test]
    fn rejects_each_missing_required_binary_before_copying() {
        for required in MSHV_ESP_PAYLOAD_BINARIES {
            let fixture = EspFixture::new();
            fs::remove_file(fixture.source.path().join(required)).unwrap();
            fixture.write_source(
                Path::new(MSHV_ESP_PAYLOAD_DIR).join("secondary.dll"),
                b"secondary",
            );

            fixture.assert_rejected_before_copy(&format!(
                "Required MSHV ESP payload file '{required}' is missing from the source ESP"
            ));
        }
    }

    #[test]
    fn rejects_directories_at_each_required_binary_path() {
        for required in MSHV_ESP_PAYLOAD_BINARIES {
            let fixture = EspFixture::new();
            let path = fixture.source.path().join(required);
            fs::remove_file(&path).unwrap();
            fs::create_dir(&path).unwrap();

            fixture.assert_rejected_before_copy(required);
        }
    }

    #[test]
    fn rejects_missing_source_esp() {
        let fixture = EspFixture::new();
        let error = copy_mshv_esp_payload(
            &fixture.source.path().join("missing"),
            fixture.destination.path(),
        )
        .unwrap_err();

        assert!(
            format!("{error:#}").contains(MSHV_ESP_PAYLOAD_BINARIES[0]),
            "unexpected error: {error:#}"
        );
        assert_eq!(fs::read_dir(fixture.destination.path()).unwrap().count(), 0);
    }

    #[test]
    fn rejects_file_at_secondary_payload_directory_before_copying() {
        let fixture = EspFixture::new();
        fixture.write_source(MSHV_ESP_PAYLOAD_DIR, b"not a directory");

        fixture.assert_rejected_before_copy(&format!(
            "Failed to read MSHV payload directory '{}'",
            fixture.payload_dir().display()
        ));
    }

    #[test]
    fn reports_secondary_payload_inspection_errors_before_copying() {
        let fixture = EspFixture::new();
        let payload_dir = fixture.payload_dir();
        fs::create_dir_all(payload_dir.parent().unwrap()).unwrap();
        unix_fs::symlink(&payload_dir, &payload_dir).unwrap();

        fixture.assert_rejected_before_copy(&format!(
            "Failed to inspect MSHV payload directory '{}'",
            payload_dir.display()
        ));
    }

    #[test]
    fn rejects_secondary_payload_symlinks_before_copying() {
        for target in ["file", "directory", "missing"] {
            let fixture = EspFixture::new();
            let outside = TempDir::new().unwrap();
            fs::write(outside.path().join("file"), b"outside").unwrap();
            fs::create_dir(outside.path().join("directory")).unwrap();
            fs::create_dir_all(fixture.payload_dir()).unwrap();
            let link = fixture.payload_dir().join("linked.dll");
            unix_fs::symlink(outside.path().join(target), &link).unwrap();

            fixture.assert_rejected_before_copy(&format!(
                "MSHV payload entry '{}' is not a regular file or directory",
                link.display()
            ));
            assert_eq!(fs::read(outside.path().join("file")).unwrap(), b"outside");
        }
    }

    #[test]
    fn rejects_secondary_payload_special_files_before_copying() {
        let fixture = EspFixture::new();
        fs::create_dir_all(fixture.payload_dir()).unwrap();
        let fifo = fixture.payload_dir().join("pipe.dll");
        unistd::mkfifo(&fifo, Mode::S_IRUSR | Mode::S_IWUSR).unwrap();

        fixture.assert_rejected_before_copy(&format!(
            "MSHV payload entry '{}' is not a regular file or directory",
            fifo.display()
        ));
    }

    #[test]
    fn reports_destination_directory_creation_errors() {
        for relative_path in ["esp", "esp/efi"] {
            let fixture = EspFixture::new();
            let destination = fixture.destination.path().join("esp");
            let obstruction = fixture.destination.path().join(relative_path);
            fs::create_dir_all(obstruction.parent().unwrap()).unwrap();
            fs::write(&obstruction, b"not a directory").unwrap();

            let error = copy_mshv_esp_payload(fixture.source.path(), &destination).unwrap_err();

            assert!(
                format!("{error:#}").contains(&format!(
                    "Failed to create MSHV ESP payload directory '{}'",
                    destination.join("efi").display()
                )),
                "unexpected error: {error:#}"
            );
            assert_eq!(fs::read(obstruction).unwrap(), b"not a directory");
        }
    }

    #[test]
    fn reports_required_binary_copy_errors() {
        for required in MSHV_ESP_PAYLOAD_BINARIES {
            let fixture = EspFixture::new();
            let destination = fixture.destination.path().join(required);
            fs::create_dir_all(&destination).unwrap();

            let error = fixture.copy().unwrap_err();

            assert!(
                format!("{error:#}").contains(&format!(
                    "Failed to copy MSHV ESP payload file '{}' to '{}'",
                    fixture.source.path().join(required).display(),
                    destination.display()
                )),
                "unexpected error: {error:#}"
            );
            assert!(destination.is_dir());
            assert_eq!(fs::read_dir(destination).unwrap().count(), 0);
        }
    }

    #[test]
    fn reports_secondary_payload_copy_errors() {
        let fixture = EspFixture::new();
        let relative_path = Path::new(MSHV_ESP_PAYLOAD_DIR).join("secondary.dll");
        fixture.write_source(&relative_path, b"secondary");
        let destination = fixture.destination.path().join(&relative_path);
        fs::create_dir_all(&destination).unwrap();

        let error = fixture.copy().unwrap_err();

        assert!(
            format!("{error:#}").contains(&format!(
                "Failed to copy MSHV ESP payload file '{}' to '{}'",
                fixture.source.path().join(relative_path).display(),
                destination.display()
            )),
            "unexpected error: {error:#}"
        );
        fixture.assert_required_binaries_copied();
        assert!(destination.is_dir());
    }
}
