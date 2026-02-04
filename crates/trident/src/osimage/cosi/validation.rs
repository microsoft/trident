use std::collections::{HashMap, HashSet};

use log::warn;

use super::{
    error::{CosiMetadataError, CosiMetadataErrorKind},
    metadata::{
        BootloaderType, CosiMetadata, GptRegionType, ImageFile, KnownMetadataVersion,
        PartitionTableType, SystemdBootloaderType,
    },
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
        if self.version >= KnownMetadataVersion::V1_1 {
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

        if self.version >= KnownMetadataVersion::V1_2 {
            // Ensure compression info is present.
            if self.compression.window_size.is_none() {
                return mk_err(CosiMetadataErrorKind::V1_2CompressionInfoRequired);
            }

            // Ensure partitions metadata is present.
            let Some(disk) = &self.disk else {
                return mk_err(CosiMetadataErrorKind::V1_2DiskInfoRequired);
            };

            // Ensure partition table type is GPT.
            match &disk.partition_table_type {
                PartitionTableType::Gpt => {}
                PartitionTableType::Unknown(value) => {
                    return mk_err(CosiMetadataErrorKind::V1_2DiskPartitionTableNotGpt(
                        value.to_string(),
                    ));
                }
            }

            // Get the first region and ensure it is the primary GPT at LBA 0.
            let Some(first_region) = &disk.gpt_regions.first() else {
                return mk_err(CosiMetadataErrorKind::V1_2DiskRegionsMissing);
            };

            if first_region.region_type != GptRegionType::PrimaryGpt {
                return mk_err(CosiMetadataErrorKind::V1_2DiskRegionsInvalidFirstRegion {
                    region_type: first_region.region_type.to_string(),
                });
            }

            // Collect known filesystem image paths for validation. It is mutable so that
            // we can remove entries as we match them to partitions and check if
            // there are any leftovers.
            let mut filesystem_paths = self
                .filesystem_image_files()
                .map(|img| (img.path.as_path(), img))
                .collect::<HashMap<_, _>>();

            let mut partition_numbers = HashSet::new();

            // Scan all other GPT regions.
            for gpt_region in disk.gpt_regions.iter().skip(1) {
                let number = match &gpt_region.region_type {
                    // Get partition number.
                    GptRegionType::Partition { number } => *number,

                    // Duplicate primary GPT region is an error.
                    GptRegionType::PrimaryGpt => {
                        return mk_err(CosiMetadataErrorKind::V1_2DuplicateGptRegion)
                    }

                    // Unknown region types are skipped with a warning. They are
                    // harmless so we do not want to fail, but we should log
                    // them.
                    GptRegionType::Other => {
                        warn!("Skipping validation for GPT region of unknown type");
                        continue;
                    }
                };

                // Partition numbers must be 1-indexed.
                if number == 0 {
                    return mk_err(CosiMetadataErrorKind::V1_2PartitionNumberZero);
                }

                // Ensure partition numbers are unique.
                if !partition_numbers.insert(number) {
                    return mk_err(CosiMetadataErrorKind::V1_2DuplicatePartitionNumber(number));
                }

                // Remove filesystem path once we match it to the partition.
                let Some(fs_image) = filesystem_paths.remove(gpt_region.image.path.as_path())
                else {
                    continue;
                };

                // Compare relevant fields between filesystem and disk images.
                compare_image_field(self, "compressedSize", fs_image, &gpt_region.image, |img| {
                    img.compressed_size
                })?;

                compare_image_field(
                    self,
                    "uncompressedSize",
                    fs_image,
                    &gpt_region.image,
                    |img| img.uncompressed_size,
                )?;

                compare_image_field(self, "sha384", fs_image, &gpt_region.image, |img| {
                    img.sha384.clone()
                })?;
            }

            // Any leftover filesystem paths were not referenced by any partition.
            if let Some((path, _)) = filesystem_paths.into_iter().next() {
                return mk_err(
                    CosiMetadataErrorKind::V1_2ImageFileHasNoCorrespondingPartition(
                        path.display().to_string(),
                    ),
                );
            }
        }

        Ok(())
    }
}

/// Compares a specific field between a filesystem image and a disk image,
/// returning an error if they do not match.
fn compare_image_field<T>(
    metadata: &CosiMetadata,
    field: &str,
    fs: &ImageFile,
    disk: &ImageFile,
    extractor: fn(&ImageFile) -> T,
) -> Result<(), CosiMetadataError>
where
    T: Eq + ToString,
{
    let fs_value = extractor(fs);
    let disk_value = extractor(disk);

    if fs_value != disk_value {
        return Err(CosiMetadataError {
            version: metadata.version,
            kind: CosiMetadataErrorKind::V1_2ImageFileMetadataMismatch {
                path: fs.path.display().to_string(),
                field: field.to_string(),
                disk_image: disk_value.to_string(),
                fs_image: fs_value.to_string(),
            },
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    const SAMPLE_SHA384: &str = "1d0f284efe3edea4b9ca3bd514fa134b17eae361ccc7a1eefeff801b9bd6604e01f21f6bf249ef030599f0c218f2ba8c";

    /// Helper to parse and validate COSI metadata, returning only the validation error kind.
    fn parse_and_validate(value: Value) -> Result<CosiMetadata, CosiMetadataErrorKind> {
        let metadata: CosiMetadata = serde_json::from_value(value).unwrap();
        metadata.validate().map_err(|e| e.kind)?;
        Ok(metadata)
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
                        "path": "path/to/image1",
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
                        "path": "path/to/image2",
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
        let metadata = parse_and_validate(base.clone()).unwrap();
        assert_eq!(metadata.version, KnownMetadataVersion::V1_0.as_version());

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
        let metadata = parse_and_validate(base.clone()).unwrap();
        assert_eq!(metadata.version, KnownMetadataVersion::V1_1);

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

    #[test]
    fn test_cosi_1_2_validation() {
        // Base COSI metadata (v1.2) that is valid for this version of Trident.
        let base = json!({
            "version": "1.2",
            "osArch": "amd64",
            "osRelease": "ID=azurelinux\nVERSION_ID=3.0\n",
            "images": [
                {
                    "image": {
                        "path": "path/to/image1",
                        "compressedSize": 100,
                        "uncompressedSize": 200,
                        "sha384": SAMPLE_SHA384
                    },
                    "mountPoint": "/mnt1",
                    "fsType": "ext4",
                    "fsUuid": "550e8400-e29b-41d4-a716-446655440000",
                    "partType": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
                    "verity": {
                        "roothash": "abc123",
                        "image": {
                            "path": "path/to/image1.verity",
                            "compressedSize": 50,
                            "uncompressedSize": 100,
                            "sha384": SAMPLE_SHA384
                        }
                    }
                },
                {
                    "image": {
                        "path": "path/to/image2",
                        "compressedSize": 100,
                        "uncompressedSize": 200,
                        "sha384": SAMPLE_SHA384
                    },
                    "mountPoint": "/mnt2",
                    "fsType": "ext4",
                    "fsUuid": "550e8400-e29b-41d4-a716-446655440001",
                    "partType": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",

                }
            ],
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
            },
            "disk": {
                "type": "gpt",
                "size": 15u64*1024*1024*1024, // 15 GiB
                "lbaSize": 512,
                "gptRegions": [
                    {
                        "type": "primary-gpt",
                        "image": {
                            "path": "path/to/image1",
                            "compressedSize": 4096,
                            "uncompressedSize": 17408,
                            "sha384": SAMPLE_SHA384
                        }
                    },
                    {
                        "type": "partition",
                        "number": 1,
                        "image": {
                            "path": "path/to/image1",
                            "compressedSize": 100,
                            "uncompressedSize": 200,
                            "sha384": SAMPLE_SHA384
                        },
                    },
                    {
                        "type": "partition",
                        "number": 2,
                        "image": {
                            "path": "path/to/image1.verity",
                            "compressedSize": 50,
                            "uncompressedSize": 100,
                            "sha384": SAMPLE_SHA384
                        }
                    },
                    {
                        "type": "partition",
                        "number": 3,
                        "image": {
                            "path": "path/to/image2",
                            "compressedSize": 100,
                            "uncompressedSize": 200,
                            "sha384": SAMPLE_SHA384
                        }
                    }
                ]
            },
            "compression": {
                "windowSize": 27
            }
        });

        // Sanity: base should validate.
        let metadata = parse_and_validate(base.clone()).unwrap();
        assert_eq!(metadata.version, KnownMetadataVersion::V1_2);

        // v1.2 requires compression info.
        let mut no_compression = base.clone();
        no_compression
            .as_object_mut()
            .unwrap()
            .remove("compression");
        assert_validate_err_kind(
            no_compression,
            CosiMetadataErrorKind::V1_2CompressionInfoRequired,
        );

        // v1.2 requires disk metadata.
        let mut no_disk = base.clone();
        no_disk.as_object_mut().unwrap().remove("disk");
        assert_validate_err_kind(no_disk, CosiMetadataErrorKind::V1_2DiskInfoRequired);

        // Disk regions array empty should error.
        let mut empty_regions = base.clone();
        empty_regions["disk"]["gptRegions"] = json!([]);
        assert_validate_err_kind(empty_regions, CosiMetadataErrorKind::V1_2DiskRegionsMissing);

        // First region not primary GPT at LBA 0 should error.
        let mut invalid_first_region = base.clone();
        invalid_first_region["disk"]["gptRegions"][0]["type"] = json!("partition");
        invalid_first_region["disk"]["gptRegions"][0]["number"] = json!(42);
        assert_validate_err_kind(
            invalid_first_region,
            CosiMetadataErrorKind::V1_2DiskRegionsInvalidFirstRegion {
                region_type: "partition".to_string(),
            },
        );

        // Partition table type not GPT should error.
        let mut not_gpt = base.clone();
        not_gpt["disk"]["type"] = json!("mbr");
        assert_validate_err_kind(
            not_gpt,
            CosiMetadataErrorKind::V1_2DiskPartitionTableNotGpt("mbr".to_string()),
        );

        // Duplicate partition numbers should error.
        let mut duplicate_partition_number = base.clone();
        duplicate_partition_number["disk"]["gptRegions"][2]["number"] = json!(1);
        assert_validate_err_kind(
            duplicate_partition_number,
            CosiMetadataErrorKind::V1_2DuplicatePartitionNumber(1),
        );

        // Partition number 0 should error.
        let mut partition_number_zero = base.clone();
        partition_number_zero["disk"]["gptRegions"][1]["number"] = json!(0);
        assert_validate_err_kind(
            partition_number_zero,
            CosiMetadataErrorKind::V1_2PartitionNumberZero,
        );

        // Partition and image file metadata mismatch should error.
        let mut image_file_mismatch_compressed = base.clone();
        image_file_mismatch_compressed["disk"]["gptRegions"][1]["image"]["compressedSize"] =
            json!(999);
        assert_validate_err_kind(
            image_file_mismatch_compressed,
            CosiMetadataErrorKind::V1_2ImageFileMetadataMismatch {
                path: "path/to/image1".to_string(),
                field: "compressedSize".to_string(),
                disk_image: "999".to_string(),
                fs_image: "100".to_string(),
            },
        );

        let mut image_file_mismatch_uncompressed = base.clone();
        image_file_mismatch_uncompressed["disk"]["gptRegions"][1]["image"]["uncompressedSize"] =
            json!(999);
        assert_validate_err_kind(
            image_file_mismatch_uncompressed,
            CosiMetadataErrorKind::V1_2ImageFileMetadataMismatch {
                path: "path/to/image1".to_string(),
                field: "uncompressedSize".to_string(),
                disk_image: "999".to_string(),
                fs_image: "200".to_string(),
            },
        );

        let other_sha = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        let mut image_file_mismatch_sha384 = base.clone();
        image_file_mismatch_sha384["disk"]["gptRegions"][1]["image"]["sha384"] = json!(other_sha);
        assert_validate_err_kind(
            image_file_mismatch_sha384,
            CosiMetadataErrorKind::V1_2ImageFileMetadataMismatch {
                path: "path/to/image1".to_string(),
                field: "sha384".to_string(),
                disk_image: other_sha.to_string(),
                fs_image: SAMPLE_SHA384.to_string(),
            },
        );

        // All image files must have a corresponding partition.
        let mut image_file_no_partition = base.clone();
        image_file_no_partition["images"]
            .as_array_mut()
            .unwrap()
            .push(json!(
                {
                    "image": {
                        "path": "path/to/image3",
                        "compressedSize": 100,
                        "uncompressedSize": 200,
                        "sha384": SAMPLE_SHA384
                    },
                    "mountPoint": "/mnt3",
                    "fsType": "ext4",
                    "fsUuid": "550e8400-e29b-41d4-a716-446655440015",
                    "partType": "0FC63DAF-8483-4772-8E79-3D69D8477DE4",
                }
            ));
        assert_validate_err_kind(
            image_file_no_partition,
            CosiMetadataErrorKind::V1_2ImageFileHasNoCorrespondingPartition(
                "path/to/image3".to_string(),
            ),
        );

        // That includes verity image files as well.
        let mut verity_image_file_no_partition = base.clone();
        verity_image_file_no_partition["disk"]["gptRegions"]
            .as_array_mut()
            .unwrap()
            .retain(|p| p["image"]["path"] != json!("path/to/image1.verity"));
        assert_validate_err_kind(
            verity_image_file_no_partition,
            CosiMetadataErrorKind::V1_2ImageFileHasNoCorrespondingPartition(
                "path/to/image1.verity".to_string(),
            ),
        );

        // More than one primary GPT region should error.
        let mut duplicate_primary_gpt = base.clone();
        duplicate_primary_gpt["disk"]["gptRegions"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "type": "primary-gpt",
                "image": {
                    "path": "path/to/image8",
                    "compressedSize": 4096,
                    "uncompressedSize": 17408,
                    "sha384": SAMPLE_SHA384
                }
            }));
        assert_validate_err_kind(
            duplicate_primary_gpt,
            CosiMetadataErrorKind::V1_2DuplicateGptRegion,
        );

        // Validation should ignore unknown region types with a warning.
        let mut unknown_region_type = base.clone();
        unknown_region_type["disk"]["gptRegions"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "type": "my-custom-type",
                "image": {
                    "path": "path/to/image8",
                    "compressedSize": 4096,
                    "uncompressedSize": 17408,
                    "sha384": SAMPLE_SHA384
                }
            }));
        parse_and_validate(unknown_region_type).unwrap();
    }
}
