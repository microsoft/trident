use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Error};
use log::{debug, trace};

use osutils::{
    block_devices,
    dependencies::Dependency,
    veritysetup::{self, VerityDevice as VerityDeviceUtils},
};
use trident_api::{
    config::{HostConfiguration, VerityDevice},
    constants::{
        DEV_MAPPER_PATH, ROOT_VERITY_DEVICE_NAME, USR_MOUNT_POINT_PATH, USR_VERITY_DEVICE_NAME,
    },
};

use crate::engine::{
    storage::common::{self, SetRelationship},
    EngineContext,
};

use super::raid;

pub(crate) fn get_updated_device_name(device_name: &str) -> String {
    format!("{}_new", device_name)
}

/// Get the root verity root hash.
fn get_root_verity_root_hash(ctx: &EngineContext) -> Result<String, Error> {
    // Extract information from the OS image.
    let Some(os_img) = ctx.image.as_ref() else {
        bail!("Image is not available");
    };

    trace!("Getting root verity root hash from OS image");
    let root_fs = os_img
        .root_filesystem()
        .context("Failed to get root filesystem from OS image")?;

    let Some(verity) = root_fs.verity.as_ref() else {
        bail!("Root filesystem in OS image is not verity enabled");
    };

    Ok(verity.roothash.clone())
}

/// Get the root verity root hash.
fn get_usr_verity_root_hash(ctx: &EngineContext) -> Result<String, Error> {
    // Extract information from the OS image.
    let Some(os_img) = ctx.image.as_ref() else {
        bail!("Image is not available");
    };

    trace!("Getting usr verity root hash from OS image");
    let usr_fs = os_img
        .filesystems()
        .find(|fs| fs.mount_point == Path::new(USR_MOUNT_POINT_PATH))
        .context("Failed to get usr filesystem from OS image")?;

    let Some(verity) = usr_fs.verity.as_ref() else {
        bail!("usr filesystem in OS image is not verity enabled");
    };

    Ok(verity.roothash.clone())
}

/// Setup verity devices.
///
/// Assumes that images are already in place (data and hash), so that it can
/// assemble the verity devices.
#[tracing::instrument(skip_all)]
pub(super) fn setup_verity_devices(ctx: &EngineContext) -> Result<(), Error> {
    // Validated from API there is only ONE verity device
    let Some(verity_device) = ctx.spec.storage.verity.first() else {
        return Ok(());
    };

    // Get the verity data and hash device paths from the engine context
    let (data_dev, hash_dev) = get_verity_device_paths(ctx, verity_device)?;
    let update_name = get_updated_device_name(&verity_device.name);

    let root_hash = if ctx.storage_graph.root_fs_is_verity() {
        debug!(
            "Setting up verity device '{}' for root filesystem",
            verity_device.id
        );

        get_root_verity_root_hash(ctx)?
    } else if ctx.storage_graph.usr_fs_is_verity() {
        debug!(
            "Setting up verity device '{}' for usr filesystem",
            verity_device.id
        );

        get_usr_verity_root_hash(ctx)?
    } else {
        bail!(
            "Verity device '{}' is not on a supported filesystem.",
            verity_device.name
        );
    };

    VerityDeviceUtils::new(update_name, data_dev, hash_dev, root_hash).open()
}

/// Get the verity data and hash paths.
///
/// Verity data and hash devices are fetched from the engine context.
pub fn get_verity_device_paths(
    ctx: &EngineContext,
    verity_device: &VerityDevice,
) -> Result<(PathBuf, PathBuf), Error> {
    let verity_data_path = ctx
        .get_block_device_path(&verity_device.data_device_id)
        .context(format!(
            "Failed to find path of verity data device with id '{}'",
            verity_device.data_device_id
        ))?;

    let verity_hash_path = ctx
        .get_block_device_path(&verity_device.hash_device_id)
        .context(format!(
            "Failed to find verity hash device with ID '{}'",
            verity_device.hash_device_id
        ))?;

    Ok((verity_data_path, verity_hash_path))
}

/// Looks for verity devices created by Trident during servicing and stops them.
///
/// This specifically targets root verity devices (named `root_new`) and usr
/// verity devices (named `usr_new`).
#[tracing::instrument(skip_all)]
pub fn stop_trident_servicing_devices(host_config: &HostConfiguration) -> Result<(), Error> {
    // If no verity module is loaded, there are no verity devices to stop
    if !Path::new("/sys/module/dm_verity").exists() {
        return Ok(());
    }

    // Close the root verity device
    stop_verity_device(
        host_config,
        &get_updated_device_name(ROOT_VERITY_DEVICE_NAME),
    )?;
    // Close the usr verity device
    stop_verity_device(
        host_config,
        &get_updated_device_name(USR_VERITY_DEVICE_NAME),
    )?;

    Ok(())
}

/// Stops a specific verity device.
fn stop_verity_device(
    host_config: &HostConfiguration,
    verity_device_name: &str,
) -> Result<(), Error> {
    debug!("Attempting to stop pre-existing verity devices");

    let verity_device_path = Path::new(DEV_MAPPER_PATH).join(verity_device_name);

    // Check if the root verity device is present
    if !verity_device_path.exists() {
        return Ok(());
    }

    // Check if the veritysetup command is available
    if !Dependency::Veritysetup.exists() {
        bail!("Veritysetup is not installed");
    }

    let root_verity_device_status = veritysetup::status(verity_device_name)
        .context("Failed to get status of root verity device")?
        .active()
        .with_context(|| {
            format!(
                "Verity device '{}' is not active",
                verity_device_path.display()
            )
        })?;

    // Resolve disks in the HC to their /dev/... paths.
    let hc_disks = block_devices::get_resolved_disks(host_config)
        .context("Failed to resolved disks in the Host Configuration to their device paths.")?
        .iter()
        .map(|rd| rd.dev_path.to_owned())
        .collect::<HashSet<_>>();

    // Get the /dev/... paths of the disks that are used to store the verity members.
    let verity_disks = {
        let mut disks = HashSet::new();
        for verity_member in root_verity_device_status.members() {
            if let Ok(disk_path) = block_devices::get_disk_for_partition(verity_member) {
                let canonical_disk_path = disk_path
                    .canonicalize()
                    .context(format!("Failed to find the device path '{:?}'", disk_path))?;
                disks.insert(canonical_disk_path);
            } else if let Ok(disk_paths) = raid::get_raid_disks(verity_member) {
                disks.extend(disk_paths);
            } else {
                bail!(
                    "Failed to find the disk path for the device path '{:?}'",
                    verity_member
                )
            }
        }

        disks
    };

    // Get what the set of verity disks is in relation to the set of disks in the Host Configuration.
    match common::subset_check(&verity_disks, &hc_disks) {
        SetRelationship::Disjoint => {
            debug!("No overlap between the verity disks and the disks in the Host Configuration, device will not be stopped.");
            return Ok(());
        }
        SetRelationship::Overlap => {
            return Err(anyhow!(
                "A device has underlying disks that are not part of Host Configuration. Used disks: {:?}, Host Configuration disks: {:?}",
                verity_disks, hc_disks,
            )).context("Could not stop verity device.");
        }
        SetRelationship::Subset => {
            debug!("Verity disks are a subset of the disks in the Host Configuration, stopping device.");
        }
    }

    block_devices::unmount_all_mount_points(&verity_device_path).context(format!(
        "Failed to unmount all mount points for verity device '{}'",
        verity_device_path.display()
    ))?;

    debug!("Closing verity device '{}'", verity_device_path.display());
    veritysetup::close(verity_device_name).context(format!(
        "Failed to close root verity device '{}'",
        verity_device_name
    ))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use sysdefs::partition_types::DiscoverablePartitionType;
    use trident_api::constants::ROOT_MOUNT_POINT_PATH;

    use crate::osimage::{
        mock::{MockImage, MockOsImage},
        OsImage, OsImageFileSystemType,
    };

    #[test]
    fn test_get_updated_device_name() {
        assert_eq!(get_updated_device_name("root"), "root_new");
        assert_eq!(get_updated_device_name("foo"), "foo_new");
    }

    #[test]
    fn test_get_usr_verity_root_hash() {
        let expected_root_hash = "sample-roothash";
        let mut mock = MockOsImage::new().with_image(MockImage::new(
            USR_MOUNT_POINT_PATH,
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::Root,
            Some(expected_root_hash),
        ));

        let as_ctx = |mock: &MockOsImage| EngineContext {
            image: Some(OsImage::mock(mock.clone())),
            ..Default::default()
        };

        assert_eq!(
            get_usr_verity_root_hash(&as_ctx(&mock)).unwrap(),
            expected_root_hash,
            "Root hash does not match expected"
        );

        // test failure when root filesystem is not verity enabled
        mock.images[0].verity = None;
        assert_eq!(
            get_usr_verity_root_hash(&as_ctx(&mock))
                .unwrap_err()
                .to_string(),
            "usr filesystem in OS image is not verity enabled",
            "Got unexpected error"
        );

        // test failure when root filesystem is not found
        mock.images.clear();
        assert_eq!(
            get_usr_verity_root_hash(&as_ctx(&mock))
                .unwrap_err()
                .to_string(),
            "Failed to get usr filesystem from OS image",
            "Got unexpected error"
        );
    }

    #[test]
    fn test_get_root_verity_root_hash() {
        let expected_root_hash = "sample-roothash";
        let mut mock = MockOsImage::new().with_image(MockImage::new(
            ROOT_MOUNT_POINT_PATH,
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::Root,
            Some(expected_root_hash),
        ));

        let as_ctx = |mock: &MockOsImage| EngineContext {
            image: Some(OsImage::mock(mock.clone())),
            ..Default::default()
        };

        assert_eq!(
            get_root_verity_root_hash(&as_ctx(&mock)).unwrap(),
            expected_root_hash,
            "Root hash does not match expected"
        );

        // test failure when root filesystem is not verity enabled
        mock.images[0].verity = None;
        assert_eq!(
            get_root_verity_root_hash(&as_ctx(&mock))
                .unwrap_err()
                .to_string(),
            "Root filesystem in OS image is not verity enabled",
            "Got unexpected error"
        );

        // test failure when root filesystem is not found
        mock.images.clear();
        assert_eq!(
            get_root_verity_root_hash(&as_ctx(&mock))
                .unwrap_err()
                .to_string(),
            "Failed to get root filesystem from OS image",
            "Got unexpected error"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {

    use super::*;

    use std::path::PathBuf;

    use const_format::formatcp;
    use maplit::btreemap;

    use osutils::{
        filesystems::MountFileSystemType,
        mount::{self, MountGuard},
        mountpoint,
        testutils::{
            repart::TEST_DISK_DEVICE_PATH,
            verity::{self},
        },
        veritysetup::VerityDeviceGuard,
    };
    use pytest_gen::functional_test;
    use sysdefs::partition_types::DiscoverablePartitionType;
    use trident_api::{
        config::{
            Disk, FileSystem, FileSystemSource, FileSystemType, InternalMountPoint, Partition,
            PartitionType, Storage, VerityDevice,
        },
        constants::{MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    };

    use crate::osimage::{
        mock::{MockImage, MockOsImage},
        OsImageFileSystemType,
    };

    #[functional_test]
    fn test_setup_verity_devices() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // test no verity devices
        let ctx = EngineContext::default();
        setup_verity_devices(&ctx).unwrap();

        assert!(ctx.partition_paths.is_empty());

        // test root verity device
        let (boot_dev, verity_vol) = verity::setup_verity_volumes_with_boot();
        let verity_dev = verity_vol.verity_device("root_new");

        // Close the verity device if it exists
        verity_dev.close().unwrap();

        let hc = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "sdb".to_string(),
                    device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                    partitions: vec![
                        Partition {
                            id: "boot".to_string(),
                            partition_type: PartitionType::Xbootldr,
                            size: 4096.into(),
                        },
                        Partition {
                            id: "root-hash".to_string(),
                            partition_type: PartitionType::RootVerity,
                            size: 4096.into(),
                        },
                        Partition {
                            id: "root-data".to_string(),
                            partition_type: PartitionType::Root,
                            size: 4096.into(),
                        },
                        Partition {
                            id: "overlay".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 4096.into(),
                        },
                    ],
                    ..Default::default()
                }],
                verity: vec![VerityDevice {
                    id: "root".into(),
                    name: "root".into(),
                    data_device_id: "root-data".into(),
                    hash_device_id: "root-hash".into(),
                    ..Default::default()
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("root".to_string()),
                        mount_point: Some(ROOT_MOUNT_POINT_PATH.into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("boot".to_string()),
                        mount_point: Some("/boot".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        device_id: Some("overlay".to_string()),
                        mount_point: Some("/var/lib/trident-overlay".into()),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::New,
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let ctx_golden = EngineContext::default()
            .with_spec(hc)
            .with_image(MockOsImage::new().with_image(MockImage::new(
                ROOT_MOUNT_POINT_PATH,
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::Root,
                Some(verity_vol.root_hash.clone()),
            )))
            .with_partition_paths(
                [
                    ("sdb", PathBuf::from(TEST_DISK_DEVICE_PATH)),
                    ("boot", boot_dev),
                    ("root-hash", verity_vol.hash_volume.clone()),
                    ("root-data", verity_vol.data_volume.clone()),
                    (
                        "overlay",
                        PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
                    ),
                ]
                .into_iter(),
            );

        {
            let ctx = ctx_golden.clone();
            setup_verity_devices(&ctx).unwrap();
            let _verityguard = VerityDeviceGuard::new("root_new");
            assert!(verity_dev.is_active().unwrap());
        }

        // test failure when root hash is not matching
        let bad_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_ne!(bad_hash, verity_vol.root_hash, "Root hash should not match");

        let ctx = ctx_golden
            .clone()
            .with_image(MockOsImage::new().with_image(MockImage::new(
                ROOT_MOUNT_POINT_PATH,
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::Root,
                Some(bad_hash.to_string()),
            )));

        assert_eq!(
            setup_verity_devices(&ctx).unwrap_err().to_string(),
            "Failed to activate verity device 'root_new', status: 'corrupted'"
        );

        // Failure should close the device!
        assert!(!verity_dev.is_active().unwrap());
    }

    #[functional_test]
    fn test_stop_pre_existing_verity_devices() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let verity_vol = verity::setup_verity_volumes();

        let ctx_golden = EngineContext {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: 100.into(),
                            },
                            Partition {
                                id: "root-hash".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: 100.into(),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: 100.into(),
                            },
                            Partition {
                                id: "overlay".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 100.into(),
                            },
                        ],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![
                        InternalMountPoint {
                            path: PathBuf::from("/var/lib/trident-overlay"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "overlay".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        InternalMountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    verity: vec![VerityDevice {
                        id: "root-verity".into(),
                        name: "root".into(),
                        data_device_id: "root".into(),
                        hash_device_id: "root-hash".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "foo".to_owned() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root-hash".to_owned() => verity_vol.hash_volume.clone(),
                "root".to_owned() => verity_vol.data_volume.clone(),
                "overlay".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
            },
            ..Default::default()
        };

        // nothing mounted
        let verity_device = verity_vol.verity_device("root_new");
        assert!(!verity_device.is_active().unwrap());
        stop_trident_servicing_devices(&ctx_golden.spec).unwrap();

        // root verity opened
        {
            let _guard = verity_device.open_with_guard().unwrap();
            assert!(verity_device.is_active().unwrap());
            stop_trident_servicing_devices(&ctx_golden.spec).unwrap();
            assert!(!verity_device.is_active().unwrap());
        }

        // root verity opened & mounted
        {
            let _guard = verity_device.open_with_guard().unwrap();

            assert!(verity_device.is_active().unwrap());
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                verity_device.device_path(),
                mount_dir.path(),
                MountFileSystemType::Ext4,
                &["defaults".into(), MOUNT_OPTION_READ_ONLY.into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes
            // out of scope
            let _mount_guard = MountGuard {
                mount_dir: mount_dir.path(),
            };

            stop_trident_servicing_devices(&ctx_golden.spec).unwrap();
            assert!(!mountpoint::check_is_mountpoint(mount_dir.path()).unwrap());
            assert!(!verity_device.is_active().unwrap());
        }

        // TODO add across disks test
    }
}
