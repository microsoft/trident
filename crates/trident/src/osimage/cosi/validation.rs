use std::collections::HashSet;

use log::warn;

use super::{
    error::{CosiMetadataError, CosiMetadataErrorKind},
    metadata::{BootloaderType, CosiMetadata, KnownMetadataVersion, SystemdBootloaderType},
};

impl CosiMetadata {
    /// Validates the COSI metadata.
    pub(super) fn validate(&self) -> Result<(), CosiMetadataError> {
        let mk_err = |kind: CosiMetadataErrorKind| {
            Err(CosiMetadataError {
                version: self.version,
                kind,
            })
        };

        // Ensure that all mount points are unique.
        let mut mount_points = HashSet::new();
        for image in &self.images {
            if !mount_points.insert(&image.mount_point) {
                return mk_err(CosiMetadataErrorKind::V1_0DuplicateMountPoint(
                    image.mount_point.display().to_string(),
                ));
            }
        }

        // Validate bootloader on COSI version >= 1.1
        if self.version >= KnownMetadataVersion::V1_1.as_version() {
            let Some(bootloader) = self.bootloader.as_ref() else {
                return mk_err(CosiMetadataErrorKind::V1_1BootloaderRequired);
            };

            match (&bootloader.bootloader_type, &bootloader.systemd_boot) {
                // Grub with systemd-boot entries is invalid
                (BootloaderType::Grub, Some(_)) => {
                    return mk_err(CosiMetadataErrorKind::V1_1GrubWithSystemdBootEntries);
                }

                // Systemd-boot without entries is invalid
                (BootloaderType::SystemdBoot, None) => {
                    return mk_err(CosiMetadataErrorKind::V1_1SystemdBootMissingEntries);
                }

                // Systemd-boot with not exactly 1 UKI entry is invalid for this version of Trident
                (BootloaderType::SystemdBoot, Some(systemd_boot)) => {
                    match systemd_boot.entries.as_slice() {
                        // No entries is invalid
                        [] => {
                            return mk_err(
                                CosiMetadataErrorKind::V1_1SystemdBootEmptyEntries,
                            );
                        }

                        // First entry MUST be of type 'uki-standalone'
                        [entry, ..] if !entry.boot_type.eq(&SystemdBootloaderType::UkiStandalone) => warn!(
                            "First entry of bootloader type 'systemd-boot' is not of type 'uki-standalone'"
                        ),

                        // Having more than one entry is warned about, only the first is used in this version of Trident.
                        [_, rest @..] if !rest.is_empty() => warn!(
                            "Bootloader type 'systemd-boot' has more than one entry, only the first entry will be used"
                        ),

                        // Everything else is OK
                        _ => {}
                    }
                }

                // Unknown bootloader type is warned about, it may cause issues down the line
                (BootloaderType::Unknown(bootloader_type), _) => {
                    warn!("Unknown bootloader type: {}", bootloader_type)
                }

                // Everything else is OK
                _ => {}
            }

            // Ensure osPackages are present and all required info is provided.
            let Some(os_packages) = &self.os_packages else {
                return mk_err(CosiMetadataErrorKind::V1_1OsPackagesRequired);
            };

            // Ensure both release and arch are provided.
            for os_package in os_packages {
                if os_package.release.is_none() {
                    return mk_err(CosiMetadataErrorKind::V1_1OsPackageMissingRelease(
                        os_package.name.clone(),
                    ));
                }

                if os_package.arch.is_none() {
                    return mk_err(CosiMetadataErrorKind::V1_1OsPackageMissingArch(
                        os_package.name.clone(),
                    ));
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const SAMPLE_SHA384: &str = "1d0f284efe3edea4b9ca3bd514fa134b17eae361ccc7a1eefeff801b9bd6604e01f21f6bf249ef030599f0c218f2ba8c";

    /// Helper to parse and validate COSI metadata, returning only the validation error kind.
    fn parse_and_validate(value: Value) -> Result<(), CosiMetadataErrorKind> {
        let metadata: CosiMetadata = serde_json::from_value(value).unwrap();
        metadata.validate().map_err(|e| e.kind)
    }

    /// Helper to assert that parsing and validating the given COSI metadata value
    /// results in the expected validation error kind.
    fn assert_validate_err_kind(value: Value, expected: CosiMetadataErrorKind) {
        let err_kind = parse_and_validate(value).unwrap_err();
        assert_eq!(err_kind, expected);
    }

    #[test]
    fn test_cosi_1_0_validation() {
        // Base COSI metadata (v1.0) that is valid for this version of Trident.
        let base = json!({
            "version": "1.0",
            "osArch": "amd64",
            "osRelease": "ID=azurelinux\nVERSION_ID=3.0\n",
            "images": [
                {
                    "image": {
                        "path": "/path/to/image1",
                        "compressedSize": 100,
                        "uncompressedSize": 200,
                        "sha384": SAMPLE_SHA384
                    },
                    "mountPoint": "/mnt",
                    "fsType": "ext4",
                    "fsUuid": "550e8400-e29b-41d4-a716-446655440000",
                    "partType": "linux-generic"
                },
                {
                    "image": {
                        "path": "/path/to/image2",
                        "compressedSize": 150,
                        "uncompressedSize": 300,
                        "sha384": SAMPLE_SHA384
                    },
                    "mountPoint": "/var",
                    "fsType": "ext4",
                    "fsUuid": "550e8400-e29b-41d4-a716-446655440001",
                    "partType": "linux-generic"
                }
            ]
        });

        // Sanity: base should validate.
        parse_and_validate(base.clone()).unwrap();

        // Duplicate mount point should error.
        let mut duplicate_mount = base.clone();
        duplicate_mount["images"][1]["mountPoint"] = json!("/mnt");
        assert_validate_err_kind(
            duplicate_mount,
            CosiMetadataErrorKind::V1_0DuplicateMountPoint("/mnt".to_string()),
        );
    }

    #[test]
    fn test_cosi_1_1_validation() {
        // Base COSI metadata (v1.1) that is valid for this version of Trident.
        // We keep `images` empty to focus coverage on the v1.1 bootloader validation.
        let base = json!({
            "version": "1.1",
            "osArch": "amd64",
            "osRelease": "ID=azurelinux\nVERSION_ID=3.0\n",
            "images": [],
            "osPackages": [
                {
                    "name": "bash",
                    "version": "1.0.0",
                    "release": "1",
                    "arch": "noarch"
                }
            ],
            "bootloader": {
                "type": "systemd-boot",
                "systemdBoot": {
                    "entries": [
                        {
                            "type": "uki-standalone",
                            "kernel": "vmlinuz",
                            "path": "EFI/Linux/uki.efi",
                            "cmdline": "quiet"
                        }
                    ]
                }
            }
        });

        // Sanity: base should validate.
        parse_and_validate(base.clone()).unwrap();

        // v1.0 does not require bootloader metadata.
        let mut v1_0 = base.clone();
        v1_0["version"] = json!("1.0");
        if let Some(obj) = v1_0.as_object_mut() {
            obj.remove("bootloader");
        }
        parse_and_validate(v1_0).unwrap();

        // v1.1 requires bootloader metadata.
        let mut no_bootloader = base.clone();
        if let Some(obj) = no_bootloader.as_object_mut() {
            obj.remove("bootloader");
        }
        assert_validate_err_kind(no_bootloader, CosiMetadataErrorKind::V1_1BootloaderRequired);

        // Grub with systemd-boot entries is invalid.
        let mut grub_with_sdb = base.clone();
        grub_with_sdb["bootloader"]["type"] = json!("grub");
        assert_validate_err_kind(
            grub_with_sdb,
            CosiMetadataErrorKind::V1_1GrubWithSystemdBootEntries,
        );

        // systemd-boot without systemd-boot entries is invalid.
        let mut sdb_missing_entries = base.clone();
        if let Some(obj) = sdb_missing_entries["bootloader"].as_object_mut() {
            obj.remove("systemdBoot");
        }
        assert_validate_err_kind(
            sdb_missing_entries,
            CosiMetadataErrorKind::V1_1SystemdBootMissingEntries,
        );

        // systemd-boot with empty entries is invalid.
        let mut sdb_empty_entries = base.clone();
        sdb_empty_entries["bootloader"]["systemdBoot"]["entries"] = json!([]);
        assert_validate_err_kind(
            sdb_empty_entries,
            CosiMetadataErrorKind::V1_1SystemdBootEmptyEntries,
        );

        // systemd-boot with first entry NOT uki-standalone only warns.
        let mut sdb_first_not_uki = base.clone();
        sdb_first_not_uki["bootloader"]["systemdBoot"]["entries"][0]["type"] = json!("config");
        parse_and_validate(sdb_first_not_uki).unwrap();

        // systemd-boot with more than one entry only warns.
        let mut sdb_multiple_entries = base.clone();
        sdb_multiple_entries["bootloader"]["systemdBoot"]["entries"] = json!([
            {
                "type": "uki-standalone",
                "kernel": "vmlinuz",
                "path": "EFI/Linux/uki.efi",
                "cmdline": "quiet"
            },
            {
                "type": "config",
                "kernel": "vmlinuz2",
                "path": "EFI/Linux/other.efi",
                "cmdline": "debug"
            }
        ]);
        parse_and_validate(sdb_multiple_entries).unwrap();

        // Unknown bootloader type only warns.
        let mut unknown_bootloader = base.clone();
        unknown_bootloader["bootloader"]["type"] = json!("lilo");
        parse_and_validate(unknown_bootloader).unwrap();

        // v1.1 requires osPackages metadata.
        let mut no_os_packages = base.clone();
        if let Some(obj) = no_os_packages.as_object_mut() {
            obj.remove("osPackages");
        }
        assert_validate_err_kind(
            no_os_packages,
            CosiMetadataErrorKind::V1_1OsPackagesRequired,
        );

        // v1.1 requires per-package release.
        let mut missing_release = base.clone();
        missing_release["osPackages"][0]
            .as_object_mut()
            .unwrap()
            .remove("release");
        assert_validate_err_kind(
            missing_release,
            CosiMetadataErrorKind::V1_1OsPackageMissingRelease("bash".to_string()),
        );

        // v1.1 requires per-package arch.
        let mut missing_arch = base.clone();
        missing_arch["osPackages"][0]
            .as_object_mut()
            .unwrap()
            .remove("arch");
        assert_validate_err_kind(
            missing_arch,
            CosiMetadataErrorKind::V1_1OsPackageMissingArch("bash".to_string()),
        );
    }
}
