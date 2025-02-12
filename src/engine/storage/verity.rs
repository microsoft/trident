use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use const_format::formatcp;
use log::debug;
use sys_mount::{Mount, MountFlags, UnmountFlags};
use tempfile::TempDir;

use osutils::{
    block_devices, filesystems::MountFileSystemType, grub::GrubConfig, lsblk, mount, veritysetup,
};
use trident_api::{
    config::{self, HostConfiguration},
    constants::{BOOT_MOUNT_POINT_PATH, DEV_MAPPER_PATH, GRUB2_CONFIG_FILENAME, GRUB2_DIRECTORY},
};

use crate::{engine::EngineContext, osimage::OsImage};

use super::raid;

/// GRUB config path relative to the `/boot` directory.
const GRUB_CONFIG_PATH: &str = formatcp!("{}/{}", GRUB2_DIRECTORY, GRUB2_CONFIG_FILENAME);

/// Informs the kernel of the hash to be used for verity on the root filesystem.
/// The value is provided as a hex string.
pub const KARG_VERITY_ROOT_HASH: &str = "roothash";

pub(crate) fn get_updated_device_name(device_name: &str) -> String {
    format!("{}_new", device_name)
}

/// Setup the root verity device.
fn setup_root_verity_device(
    ctx: &EngineContext,
    root_verity_device: &config::VerityDevice,
) -> Result<(), Error> {
    // Extract the root hash from GRUB config
    let root_hash = get_root_verity_root_hash(ctx)?;

    // Get the verity data and hash device paths from the engine context
    let (verity_data_path, verity_hash_path) = get_verity_device_paths(ctx, root_verity_device)?;

    let updated_device_name = get_updated_device_name(&root_verity_device.name);

    // Setup the verity device
    veritysetup::open(
        verity_data_path,
        updated_device_name.as_str(),
        verity_hash_path,
        root_hash.as_str(),
    )?;

    let status = veritysetup::status(updated_device_name.as_str());
    match status {
        Err(e) => {
            veritysetup::close(updated_device_name.as_str())?;
            return Err(e);
        }
        Ok(status) => {
            if status.status != "verified" {
                veritysetup::close(updated_device_name.as_str())?;
                return Err(anyhow::anyhow!(
                    "Failed to activate verity device '{}', status: '{}'",
                    root_verity_device.name,
                    status.status
                ));
            }
        }
    }
    Ok(())
}

/// Get the root verity root hash.
fn get_root_verity_root_hash(ctx: &EngineContext) -> Result<String, Error> {
    // When available, extract information from the OS image.
    if let Some(os_img) = ctx.image.as_ref() {
        get_root_verity_root_hash_osimage(os_img).context(format!(
            "Failed to get root hash from OS image '{}'",
            os_img.source()
        ))
    } else {
        get_root_verity_root_hash_grub(ctx)
    }
}

/// Get the root verity root hash from the OS image.
fn get_root_verity_root_hash_osimage(os_img: &OsImage) -> Result<String, Error> {
    let root_fs = os_img
        .root_filesystem()
        .context("Failed to get root filesystem from OS image")?;

    if let Some(verity) = root_fs.verity.as_ref() {
        Ok(verity.roothash.clone())
    } else {
        bail!("Root filesystem in OS image is not verity enabled");
    }
}

/// Get the root verity root hash from the GRUB config.
fn get_root_verity_root_hash_grub(ctx: &EngineContext) -> Result<String, Error> {
    // API check ensures there is a boot volume, look up its mount point
    let boot_mount_point = &ctx
        .spec
        .storage
        .internal_mount_points
        .iter()
        .find(|mp| mp.path == Path::new(BOOT_MOUNT_POINT_PATH))
        .context("Cannot find boot volume")?;

    // Get the boot device path
    let boot_device_id = &boot_mount_point.target_id;
    let boot_device_path = ctx.get_block_device_path(boot_device_id).context(format!(
        "Failed to find path of boot device with id '{}'",
        boot_device_id
    ))?;

    // Mount the boot device temporarily to fetch the GRUB config
    let boot_mount_dir = TempDir::new().context("Failed to create temporary directory")?;
    let _boot_mount = Mount::builder()
        .fstype(
            MountFileSystemType::from_api_type(boot_mount_point.filesystem).with_context(|| {
                format!(
                    "Failed to convert filesystem type for boot mount point '{}'",
                    boot_mount_point.path.display()
                )
            })?,
        )
        .flags(MountFlags::RDONLY)
        .mount_autodrop(
            boot_device_path,
            boot_mount_dir.path(),
            UnmountFlags::empty(),
        )?;

    // Extract the root hash from the GRUB config
    let mut grub_config = GrubConfig::read(boot_mount_dir.path().join(GRUB_CONFIG_PATH))?;
    grub_config.check_linux_command_line_count()?;
    let root_hash = grub_config.read_linux_command_line_argument(KARG_VERITY_ROOT_HASH)?;

    Ok(root_hash)
}

/// Setup verity devices; currently, only the root verity device is supported.
#[tracing::instrument(skip_all)]
pub(super) fn setup_verity_devices(ctx: &EngineContext) -> Result<(), Error> {
    // Validated from API there is only one verity device at the moment and it
    // is tied to the root volume
    if let Some(root_verity_device) = ctx.spec.storage.internal_verity.first() {
        // Prefer old verity API for now.
        setup_root_verity_device(ctx, root_verity_device)?;
    } else if let Some(verity_device) = ctx.spec.storage.verity.first() {
        // Failback to new verity API.
        setup_root_verity_device(ctx, verity_device)?;
    }

    Ok(())
}

/// Get the verity data and hash paths.
///
/// Verity data and hash devices are fetched from the engine context.
pub fn get_verity_device_paths(
    ctx: &EngineContext,
    verity_device: &config::VerityDevice,
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

#[tracing::instrument(skip_all)]
pub fn stop_pre_existing_verity_devices(host_config: &HostConfiguration) -> Result<(), Error> {
    // If no verity module is loaded, there are no verity devices to stop
    if !Path::new("/sys/module/dm_verity").exists() {
        return Ok(());
    }

    debug!("Attempting to stop pre-existing verity devices");

    // Compose path of the root verity device for the updated volume
    let updated_device_name = get_updated_device_name("root");
    let root_verity_device_path = Path::new(DEV_MAPPER_PATH).join(&updated_device_name);

    // Check if the root verity device is present
    if !root_verity_device_path.exists() {
        return Ok(());
    }

    veritysetup::is_present().context("Unable to deactivate pre-existing dm-verity volumes.")?;

    let root_verity_device_status = veritysetup::status(&updated_device_name)
        .context("Failed to get status of root verity device")?;
    let hc_disks = super::get_hostconfig_disk_paths(host_config)
        .context("Failed to get disks defined in Host Configuration")?;
    let verity_disks = [
        root_verity_device_status.data_device_path,
        root_verity_device_status.hash_device_path,
    ]
    .map(|device_path| {
        if let Ok(disk_path) = block_devices::get_disk_for_partition(&device_path) {
            [disk_path.canonicalize().context(format!(
                "Failed to find the device path '{:?}'",
                device_path
            ))]
            .into_iter()
            .collect::<Result<Vec<PathBuf>, Error>>()
        } else if let Ok(disk_paths) = raid::get_raid_disks(&device_path) {
            Ok(disk_paths.into_iter().collect::<Vec<_>>())
        } else {
            bail!(
                "Failed to find the disk path for the device path '{:?}'",
                device_path
            )
        }
    })
    .into_iter()
    .collect::<Result<Vec<Vec<PathBuf>>, Error>>()
    .context("Failed to get verity disks")?
    .into_iter()
    .flatten()
    .collect::<HashSet<_>>();

    if block_devices::can_stop_pre_existing_device(
        &verity_disks,
        &hc_disks.iter().cloned().collect::<HashSet<_>>(),
    )
    .context(format!(
        "Failed to stop verity device '{}'",
        root_verity_device_path.display()
    ))? {
        let block_device = lsblk::get(&root_verity_device_path)?;
        debug!(
            "Unmounting any mounted partitions on verity device '{}'",
            root_verity_device_path.display()
        );
        let mount_points = block_device.mountpoints;
        if !mount_points.is_empty() {
            for mount_point in mount_points.iter() {
                mount::umount(mount_point, true)?;
            }
        }
        debug!(
            "Deactivating verity device '{}'",
            root_verity_device_path.display()
        );
        veritysetup::close(&updated_device_name).context(format!(
            "Failed to close root verity device '{}'",
            updated_device_name
        ))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use osutils::partition_types::DiscoverablePartitionType;
    use trident_api::constants::ROOT_MOUNT_POINT_PATH;

    use crate::osimage::{
        mock::{MockImage, MockOsImage},
        OsImageFileSystemType,
    };

    #[test]
    fn test_get_updated_device_name() {
        assert_eq!(get_updated_device_name("root"), "root_new");
        assert_eq!(get_updated_device_name("foo"), "foo_new");
    }

    #[test]
    fn test_get_root_verity_root_hash_osimage() {
        let expected_root_hash = "sample-roothash";
        let mut mock = MockOsImage::new().with_image(MockImage::new(
            ROOT_MOUNT_POINT_PATH,
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::Root,
            Some(expected_root_hash),
        ));

        assert_eq!(
            get_root_verity_root_hash_osimage(&OsImage::mock(mock.clone())).unwrap(),
            expected_root_hash,
            "Root hash does not match expected"
        );

        // test failure when root filesystem is not verity enabled
        mock.images[0].verity = None;
        assert_eq!(
            get_root_verity_root_hash_osimage(&OsImage::mock(mock.clone()))
                .unwrap_err()
                .to_string(),
            "Root filesystem in OS image is not verity enabled",
            "Got unexpected error"
        );

        // test failure when root filesystem is not found
        mock.images.clear();
        assert_eq!(
            get_root_verity_root_hash_osimage(&OsImage::mock(mock.clone()))
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

    use std::{fs, path::PathBuf};

    use maplit::btreemap;

    use osutils::{
        files,
        filesystems::MountFileSystemType,
        mount::{self, MountGuard},
        mountpoint,
        testutils::{
            repart::TEST_DISK_DEVICE_PATH,
            verity::{self, VerityGuard},
        },
    };
    use pytest_gen::functional_test;
    use trident_api::{
        config::{Disk, FileSystemType, Partition, PartitionType, Storage},
        constants::MOUNT_OPTION_READ_ONLY,
    };

    #[functional_test]
    fn test_get_root_verity_root_hash_grub() {
        let expected_root_hash = verity::setup_verity_volumes();

        let ctx = EngineContext {
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
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: 100.into(),
                            },
                            Partition {
                                id: "root-verity".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: 100.into(),
                            },
                        ],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![
                        config::InternalMountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        config::InternalMountPoint {
                            path: PathBuf::from("/"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "root".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "sdb".to_owned() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                "root-verity".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
            },
            ..Default::default()
        };

        assert_eq!(
            get_root_verity_root_hash_grub(&ctx).unwrap(),
            expected_root_hash
        );

        // test failure on missing boot partition in config/status
        let mut ctx_no_boot_mount = ctx.clone();
        ctx_no_boot_mount
            .spec
            .storage
            .internal_mount_points
            .retain(|mp| mp.path != PathBuf::from("/boot"));
        assert_eq!(
            get_root_verity_root_hash_grub(&ctx_no_boot_mount)
                .unwrap_err()
                .to_string(),
            "Cannot find boot volume"
        );

        let mut ctx_no_boot_part = ctx.clone();
        ctx_no_boot_part
            .spec
            .storage
            .disks
            .iter_mut()
            .find(|d| d.id == "sdb")
            .unwrap()
            .partitions
            .retain(|p| p.id != "boot");
        ctx_no_boot_part.partition_paths.remove("boot");
        assert_eq!(
            get_root_verity_root_hash_grub(&ctx_no_boot_part)
                .unwrap_err()
                .to_string(),
            "Failed to find path of boot device with id 'boot'"
        );

        // test failure when linux command line does not carry roothash argument
        {
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                mount_dir.path(),
                MountFileSystemType::Ext4,
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: mount_dir.path(),
            };

            let grub_config_path = mount_dir.path().join("grub2/grub.cfg");
            let grub_config = fs::read_to_string(&grub_config_path).unwrap();
            let grub_config = grub_config.replace(KARG_VERITY_ROOT_HASH, "foobar");
            files::write_file(grub_config_path, 0o644, grub_config.as_bytes()).unwrap();
        }

        assert!(get_root_verity_root_hash_grub(&ctx)
            .unwrap_err()
            .to_string()
            .starts_with("Failed to find 'roothash' on linux command line in '"));
    }

    #[functional_test]
    fn test_setup_root_verity_device() {
        let _expected_root_hash = verity::setup_verity_volumes();

        let verity_device_path = Path::new(DEV_MAPPER_PATH).join("root_new");
        if verity_device_path.exists() {
            veritysetup::close("root_new").unwrap();
        }

        assert!(!verity_device_path.exists());

        let ctx = EngineContext {
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
                        config::InternalMountPoint {
                            path: PathBuf::from("/var/lib/trident-overlay"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "overlay".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        config::InternalMountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    internal_verity: vec![config::VerityDevice {
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
                "sdb".to_owned() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root-hash".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                "root".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                "overlay".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
            },
            ..Default::default()
        };

        {
            setup_root_verity_device(&ctx, &ctx.spec.storage.internal_verity[0]).unwrap();
            let _verityguard = VerityGuard {
                device_name: "root_new",
            };
            assert!(verity_device_path.exists());
        }

        // test failure when root hash is not matching
        {
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                mount_dir.path(),
                MountFileSystemType::Ext4,
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: mount_dir.path(),
            };

            let grub_config_path = mount_dir.path().join("grub2/grub.cfg");
            let mut grub_config = GrubConfig::read(grub_config_path).unwrap();
            grub_config
                .update_linux_command_line_argument(
                    KARG_VERITY_ROOT_HASH,
                    "4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076",
                )
                .unwrap();
            grub_config.write().unwrap();
        }

        assert_eq!(
            setup_root_verity_device(&ctx, &ctx.spec.storage.internal_verity[0])
                .unwrap_err()
                .to_string(),
            "Failed to activate verity device 'root', status: 'corrupted'"
        );
        assert!(!verity_device_path.exists());
    }

    #[functional_test]
    fn test_setup_verity_devices() {
        // test no verity devices
        let ctx = EngineContext::default();
        setup_verity_devices(&ctx).unwrap();

        assert!(ctx.partition_paths.is_empty());

        // test root verity device
        let _expected_root_hash = verity::setup_verity_volumes();

        let verity_device_path = Path::new(DEV_MAPPER_PATH).join("root_new");
        if verity_device_path.exists() {
            veritysetup::close("root_new").unwrap();
        }

        assert!(!verity_device_path.exists());

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
                        config::InternalMountPoint {
                            path: PathBuf::from("/var/lib/trident-overlay"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "overlay".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        config::InternalMountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    internal_verity: vec![config::VerityDevice {
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
                "sdb".to_owned() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root-hash".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                "root".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                "overlay".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
            },
            ..Default::default()
        };

        {
            let ctx = ctx_golden.clone();
            setup_verity_devices(&ctx).unwrap();
            let _verityguard = VerityGuard {
                device_name: "root_new",
            };
            assert!(verity_device_path.exists());
            assert_eq!(ctx.partition_paths.len(), 5);
        }

        // test failure when root hash is not matching
        {
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                mount_dir.path(),
                MountFileSystemType::Ext4,
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: mount_dir.path(),
            };

            let grub_config_path = mount_dir.path().join("grub2/grub.cfg");
            let mut grub_config = GrubConfig::read(grub_config_path).unwrap();
            grub_config
                .update_linux_command_line_argument(
                    KARG_VERITY_ROOT_HASH,
                    "4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076",
                )
                .unwrap();
            grub_config.write().unwrap();
        }

        let ctx = ctx_golden.clone();
        assert_eq!(
            setup_verity_devices(&ctx).unwrap_err().to_string(),
            "Failed to activate verity device 'root', status: 'corrupted'"
        );
        assert!(!verity_device_path.exists());
        assert_eq!(ctx.partition_paths.len(), 5);
        assert_eq!(ctx.partition_paths, ctx_golden.partition_paths);
    }

    #[functional_test]
    fn test_stop_pre_existing_verity_devices() {
        verity::setup_verity_volumes();
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
                        config::InternalMountPoint {
                            path: PathBuf::from("/var/lib/trident-overlay"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "overlay".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        config::InternalMountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: FileSystemType::Ext4,
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    internal_verity: vec![config::VerityDevice {
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
                "root-hash".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                "root".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                "overlay".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
            },
            ..Default::default()
        };

        // nothing mounted
        let verity_root_path = Path::new(DEV_MAPPER_PATH).join("root_new");
        assert!(!verity_root_path.exists());
        stop_pre_existing_verity_devices(&ctx_golden.spec).unwrap();

        // root verity opened
        {
            let ctx = ctx_golden.clone();
            setup_verity_devices(&ctx).unwrap();
            assert!(verity_root_path.exists());
            stop_pre_existing_verity_devices(&ctx.spec).unwrap();
            assert!(!verity_root_path.exists());
        }

        // root verity opened & mounted
        {
            let ctx = ctx_golden.clone();
            setup_verity_devices(&ctx).unwrap();
            assert!(verity_root_path.exists());
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                &verity_root_path,
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
            stop_pre_existing_verity_devices(&ctx.spec).unwrap();
            assert!(!mountpoint::check_is_mountpoint(mount_dir.path()).unwrap());
            assert!(!verity_root_path.exists());
        }

        // TODO add across disks test
    }
}
