use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use trident_api::{
    config::{FileSystemSource, HostConfiguration},
    error::{InternalError, InvalidInputError, ReportError, TridentError},
};

use super::EngineContext;

/// Validates that the host configuration aligns with the OS image metadata.
///
/// Checks that:
/// - There must be an equal number of filesystems in the OS image and Host Configuration
/// - Filesystems in the OS image must match on mount points with filesystems in the Host Configuration
pub fn validate_host_config(
    ctx: &EngineContext,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    let Some(os_image) = &ctx.os_image else {
        return Ok(());
    };

    // Populate hashmap with filesystems from OS image
    let all_os_image_filesystems = os_image
        .filesystems()
        .chain(os_image.esp_filesystem())
        .collect::<Vec<_>>();
    let os_image_filesystems_map = all_os_image_filesystems
        .iter()
        .map(|fs| (fs.mount_point(), fs.fs_type().to_string()))
        .collect::<HashMap<&Path, String>>();

    // Populate hashmap with filesystems from Host Configuration
    let hc_filesystems_map = host_config
        .storage
        .filesystems
        .iter()
        .filter(|fs| fs.source == FileSystemSource::OsImage)
        .map(|fs| {
            let mount_point = fs
                .mount_point
                .as_ref()
                .map(|mp| mp.path.as_path())
                .structured(InternalError::GetMountPointForOSImage)?;
            Ok((mount_point, fs.fs_type.to_string()))
        })
        .collect::<Result<HashMap<_, _>, TridentError>>()?;

    // Create sets of mount points to check for missing or unused filesystems
    let os_image_filesystems_set = os_image_filesystems_map.keys().collect::<HashSet<_>>();
    let hc_filesystems_set = hc_filesystems_map.keys().collect::<HashSet<_>>();

    // Check that all filesystems in OS image are present in Host Config
    if let Some(not_found_in_hc) = os_image_filesystems_set
        .difference(&hc_filesystems_set)
        .next()
    {
        return Err(TridentError::new(
            InvalidInputError::UnusedOsImageFilesystem {
                mount_point: not_found_in_hc.display().to_string(),
                fs_type: os_image_filesystems_map[*not_found_in_hc].clone(),
            },
        ));
    }

    // Check that all filesystems in Host Config are present in OS image
    if let Some(not_found_in_os_img) = hc_filesystems_set
        .difference(&os_image_filesystems_set)
        .next()
    {
        return Err(TridentError::new(
            InvalidInputError::MissingOsImageFilesystem {
                mount_point: not_found_in_os_img.display().to_string(),
                fs_type: hc_filesystems_map[*not_found_in_os_img].clone(),
            },
        ));
    }

    // Check for mismatched filesystems, i.e. mount point exists in both OS
    // image and Host Configuration but filesystem type differs
    if let Some((mount_point, hc_fs_type)) = hc_filesystems_map
        .iter()
        .find(|(mount_point, hc_fs_type)| **hc_fs_type != os_image_filesystems_map[*mount_point])
    {
        return Err(TridentError::new(InvalidInputError::MismatchedFsType {
            mount_point: mount_point.display().to_string(),
            hc_fs_type: hc_fs_type.clone(),
            os_img_fs_type: os_image_filesystems_map[*mount_point].clone(),
        }));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{path::PathBuf, str::FromStr};

    use super::*;

    use url::Url;
    use uuid::Uuid;

    use osutils::{
        arch::SystemArchitecture, osrelease::OsRelease, osuuid::OsUuid,
        partition_types::DiscoverablePartitionType,
    };
    use trident_api::{
        config::{FileSystem, FileSystemSource, FileSystemType, MountPoint, Storage},
        error::ErrorKind,
    };

    use crate::osimage::{
        mock::{MockImage, MockOsImage},
        OsImage,
    };

    const OSIMAGE_DUMMY_SOURCE: &str = "http://example/osimage";

    fn generate_test_engine_context(
        os_image: OsImage,
        fs: impl Iterator<Item = (&'static str, FileSystemType)>,
    ) -> EngineContext {
        EngineContext {
            os_image: Some(os_image),
            spec: HostConfiguration {
                storage: Storage {
                    filesystems: fs
                        .map(|(path, fs_type)| FileSystem {
                            device_id: Some("dev".into()),
                            fs_type,
                            source: FileSystemSource::OsImage,
                            mount_point: Some(MountPoint::from_str(path).unwrap()),
                        })
                        .collect::<Vec<_>>(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_validate_host_config_success() {
        let mock_entries = [
            ("/image/path/A", "ext4", FileSystemType::Ext4),
            ("/image/path/B", "ext4", FileSystemType::Ext4),
        ]
        .into_iter();

        // Generate mock OS image
        let os_image = OsImage::mock(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
            os_release: OsRelease::default(),
            images: mock_entries
                .clone()
                .map(|(path, fs_type, _)| MockImage {
                    mount_point: PathBuf::from(path),
                    fs_type: fs_type.to_string(),
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                })
                .collect(),
        });

        // Generate matching Engine Context and Host Configuration
        let ctx = generate_test_engine_context(
            os_image,
            mock_entries.map(|(path, _, fs_type)| (path, fs_type)),
        );

        let host_config = ctx.spec.clone();

        // Test that validation passes
        validate_host_config(&ctx, &host_config).unwrap();
    }

    /// This test checks the scenario where there are more filesystems listed in
    /// the OS image than there are in the Host Configuration
    #[test]
    fn test_validate_host_config_failure_unused() {
        let mock_entries_os_image = [
            ("/image/path/A", "ext4"),
            ("/image/path/B", "ext4"),
            ("/unused/image/C", "ext4"),
        ]
        .into_iter();

        // Generate mock OS image
        let os_image = OsImage::mock(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
            os_release: OsRelease::default(),
            images: mock_entries_os_image
                .clone()
                .map(|(path, fs_type)| MockImage {
                    mount_point: PathBuf::from(path),
                    fs_type: fs_type.to_string(),
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                })
                .collect(),
        });

        let mock_entries_hc = [
            ("/image/path/A", FileSystemType::Ext4),
            ("/image/path/B", FileSystemType::Ext4),
        ]
        .into_iter();

        // Generate Engine Context and Host Configuration
        let ctx = generate_test_engine_context(os_image, mock_entries_hc);

        let host_config = ctx.spec.clone();

        // Test that validation does not pass
        let validation_err = validate_host_config(&ctx, &host_config).unwrap_err();
        assert_eq!(
            validation_err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::UnusedOsImageFilesystem {
                mount_point: "/unused/image/C".to_string(),
                fs_type: "ext4".to_string()
            }),
            "Expected UnusedOsImageFilesystem error"
        );
    }

    /// This test checks the scenario where the filesystems on the OS image
    /// do not match those in the Host Configuration
    #[test]
    fn test_validate_host_config_failure_mismatch() {
        let mock_entries_os_image =
            [("/image/path/A", "ext4"), ("/image/path/B", "ext4")].into_iter();

        // Generate mock OS image
        let os_image = OsImage::mock(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
            os_release: OsRelease::default(),
            images: mock_entries_os_image
                .clone()
                .map(|(path, fs_type)| MockImage {
                    mount_point: PathBuf::from(path),
                    fs_type: fs_type.to_string(),
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                })
                .collect(),
        });

        let mock_entries_hc = [
            ("/image/path/A", FileSystemType::Ext4),
            ("/image/path/B", FileSystemType::Vfat),
        ]
        .into_iter();

        // Generate Engine Context and Host Configuration
        let ctx = generate_test_engine_context(os_image, mock_entries_hc);

        let host_config = ctx.spec.clone();

        // Test that validation does not pass
        let validation_err = validate_host_config(&ctx, &host_config).unwrap_err();
        assert_eq!(
            validation_err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::MismatchedFsType {
                mount_point: "/image/path/B".to_string(),
                hc_fs_type: "vfat".to_string(),
                os_img_fs_type: "ext4".to_string()
            }),
            "Expected MismatchedFsType error"
        )
    }

    /// This test checks the scenario where a filesystem on the Host
    /// Configuration is missing from the OS image
    #[test]
    fn test_validate_host_config_failure_missing() {
        let mock_entries_os_image =
            [("/image/path/A", "ext4"), ("/image/path/B", "ext4")].into_iter();

        // Generate mock OS image
        let os_image = OsImage::mock(MockOsImage {
            source: Url::parse(OSIMAGE_DUMMY_SOURCE).unwrap(),
            os_arch: SystemArchitecture::X86,
            os_release: OsRelease::default(),
            images: mock_entries_os_image
                .clone()
                .map(|(path, fs_type)| MockImage {
                    mount_point: PathBuf::from(path),
                    fs_type: fs_type.to_string(),
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                })
                .collect(),
        });

        let mock_entries_hc = [
            ("/image/path/A", FileSystemType::Ext4),
            ("/image/path/B", FileSystemType::Ext4),
            ("/image/path/C", FileSystemType::Ext4),
        ]
        .into_iter();

        // Generate Engine Context and Host Configuration
        let ctx = generate_test_engine_context(os_image, mock_entries_hc);

        let host_config = ctx.spec.clone();

        // Test that validation does not pass
        let validation_err = validate_host_config(&ctx, &host_config).unwrap_err();
        assert_eq!(
            validation_err.kind(),
            &ErrorKind::InvalidInput(InvalidInputError::MissingOsImageFilesystem {
                mount_point: "/image/path/C".to_string(),
                fs_type: "ext4".to_string()
            }),
            "Expected MissingOsImageFilesystem error"
        )
    }
}
