use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use anyhow::{Context, Error};
use const_format::formatcp;
use log::debug;
use osutils::{block_devices, grub::GrubConfig, lsblk, mount, veritysetup};
use sys_mount::{FilesystemType, Mount, MountFlags, UnmountFlags};
use tempfile::TempDir;

use trident_api::{
    config::{self, HostConfiguration, MountPoint},
    constants::{
        BOOT_MOUNT_POINT_PATH, BOOT_RELATIVE_MOUNT_POINT_PATH, GRUB2_CONFIG_FILENAME,
        GRUB2_CONFIG_RELATIVE_PATH, GRUB2_DIRECTORY, ROOT_MOUNT_POINT_PATH,
        TRIDENT_OVERLAY_LOWER_RELATIVE_PATH, TRIDENT_OVERLAY_PATH,
        TRIDENT_OVERLAY_UPPER_RELATIVE_PATH, TRIDENT_OVERLAY_WORK_RELATIVE_PATH,
    },
    status::{BlockDeviceContents, BlockDeviceInfo, HostStatus},
    BlockDeviceId,
};

use crate::modules;

const GRUB_CONFIG_PATH: &str = formatcp!("{}/{}", GRUB2_DIRECTORY, GRUB2_CONFIG_FILENAME);

/// Indicates to dracut whether to activate verity. This is a boolean value.
const VERITY_ENABLED: &str = "rd.systemd.verity";

/// Points to a block device with root volume data.
const VERITY_ROOT_DATA: &str = "systemd.verity_root_data";

/// Points to a block device with root volume dm-verity hash tree.
const VERITY_ROOT_HASH: &str = "systemd.verity_root_hash";

/// Polints to a block device used to hold overlay data.
const OVERLAYFS_PERSISTENT_VOLUME: &str = "rd.overlayfs_persistent_volume";

/// Holds a comma-separated list of overlayfs paths.
const OVERLAYS: &str = "rd.overlayfs";

/// Holds a comma-separated list of overlayfs paths.
pub const OVERLAYS_VALUE: &str =
    formatcp!("{TRIDENT_OVERLAY_LOWER_RELATIVE_PATH},{TRIDENT_OVERLAY_UPPER_RELATIVE_PATH},{TRIDENT_OVERLAY_WORK_RELATIVE_PATH}");

/// Checks if verity is enabled in the GRUB config
pub(super) fn check_verity_enabled(grub_config_path: &Path) -> Result<bool, Error> {
    debug!(
        "Reading GRUB config at path '{}'",
        grub_config_path.display(),
    );
    let mut grub_config = GrubConfig::read(grub_config_path)?;

    if !grub_config.contains_linux_command_line_argument(VERITY_ENABLED)? {
        return Ok(false);
    }

    let verity_value = grub_config.read_linux_command_line_argument(VERITY_ENABLED)?;

    Ok(verity_value == "1" || verity_value == "yes")
}

/// Create read-only /etc/ overlay mount point representation
pub(super) fn create_etc_overlay_mount_point() -> MountPoint {
    // inject the /etc overlay used for verity setups
    debug!("Creating /etc overlay mount point for verity setups");
    MountPoint {
        filesystem: "overlay".to_owned(),
        options: vec![
            format!("lowerdir=/{TRIDENT_OVERLAY_LOWER_RELATIVE_PATH}"),
            format!("upperdir={TRIDENT_OVERLAY_PATH}/{TRIDENT_OVERLAY_UPPER_RELATIVE_PATH}"),
            format!("workdir={TRIDENT_OVERLAY_PATH}/{TRIDENT_OVERLAY_WORK_RELATIVE_PATH}"),
            "ro".to_owned(),
        ],
        target_id: "".to_owned(),
        path: PathBuf::from(ROOT_MOUNT_POINT_PATH).join(TRIDENT_OVERLAY_LOWER_RELATIVE_PATH),
    }
}

/// Setup the root verity device
fn setup_root_verity_device(
    host_config: &HostConfiguration,
    host_status: &HostStatus,
    root_verity_device: &config::VerityDevice,
) -> Result<(BlockDeviceId, BlockDeviceInfo), Error> {
    // Extract the root hash from GRUB config
    let root_hash = get_root_verity_root_hash(host_config, host_status)?;

    // Get the verity data and hash device paths from the host status
    let (verity_data_path, verity_hash_path, _) =
        get_verity_related_device_paths(host_status, host_config, root_verity_device)?;

    // Setup the verity device
    veritysetup::open(
        verity_data_path,
        &root_verity_device.device_name,
        verity_hash_path,
        root_hash.as_str(),
    )?;

    let status = veritysetup::status(&root_verity_device.device_name);
    match status {
        Err(e) => {
            veritysetup::close(&root_verity_device.device_name)?;
            return Err(e);
        }
        Ok(status) => {
            if status.status != "verified" {
                veritysetup::close(&root_verity_device.device_name)?;
                return Err(anyhow::anyhow!(
                    "Failed to activate verity device '{}', status: '{}'",
                    root_verity_device.device_name,
                    status.status
                ));
            }
        }
    }
    Ok((
        root_verity_device.id.clone(),
        BlockDeviceInfo {
            path: PathBuf::from(format!("/dev/mapper/{}", root_verity_device.device_name)),
            contents: BlockDeviceContents::Initialized,
            size: 0, // TODO: https://dev.azure.com/mariner-org/ECF/_workitems/edit/7319/
        },
    ))
}

/// Get the root verity root hash from the GRUB config
fn get_root_verity_root_hash(
    host_config: &HostConfiguration,
    host_status: &HostStatus,
) -> Result<String, Error> {
    // API check ensures there is a boot volume, look up its mount point
    let boot_mount_point = &host_config
        .storage
        .mount_points
        .iter()
        .find(|mp| mp.path == Path::new(BOOT_MOUNT_POINT_PATH))
        .context("Cannot find boot volume")?;

    // Get the boot device path
    let boot_device_id = &boot_mount_point.target_id;
    let boot_device_path = modules::get_block_device(host_status, boot_device_id, false)
        .context(format!("Failed to find boot device {}", boot_device_id))?
        .path;

    // Mount the boot device temporarily to fetch the GRUB config
    let boot_mount_dir = TempDir::new().context("Failed to create temporary directory")?;
    let _boot_mount = Mount::builder()
        .fstype(FilesystemType::from(boot_mount_point.filesystem.as_str()))
        .flags(MountFlags::RDONLY)
        .mount_autodrop(
            boot_device_path,
            boot_mount_dir.path(),
            UnmountFlags::empty(),
        )?;

    // Extract the root hash from the GRUB config
    let mut grub_config = GrubConfig::read(boot_mount_dir.path().join(GRUB_CONFIG_PATH).as_path())?;
    grub_config.check_linux_command_line_count()?;
    let root_hash = grub_config.read_linux_command_line_argument("roothash")?;

    Ok(root_hash)
}

/// Setup verity devices; currently, only the root verity device is supported
pub(super) fn setup_verity_devices(
    host_config: &HostConfiguration,
    host_status: &mut HostStatus,
) -> Result<(), Error> {
    if host_config.storage.verity.is_empty() {
        return Ok(());
    }

    // Validated from API there is only one verity device at the moment and it
    // is tied to the root volume
    let root_verity_device = &host_config.storage.verity[0];
    let (id, verity_device_status) =
        setup_root_verity_device(host_config, host_status, root_verity_device)?;

    // Update the host status
    host_status
        .storage
        .block_devices
        .insert(id, verity_device_status);

    Ok(())
}

/// Get the verity data, hash, and overlay device paths
///
/// Verity data and hash devices are fetched from the host status, and the
/// overlay is curently hardcoded to TRIDENT_OVERLAY_PATH (/var/lib/trident-overlay).
fn get_verity_related_device_paths(
    host_status: &HostStatus,
    host_config: &HostConfiguration,
    verity_device: &config::VerityDevice,
) -> Result<(std::path::PathBuf, std::path::PathBuf, std::path::PathBuf), Error> {
    let verity_data_path =
        modules::get_block_device(host_status, &verity_device.data_target_id, false)
            .context(format!(
                "Failed to find verity data target id {}",
                verity_device.data_target_id
            ))?
            .path;

    let verity_hash_path =
        modules::get_block_device(host_status, &verity_device.hash_target_id, false)
            .context(format!(
                "Failed to find verity hash target id {}",
                verity_device.hash_target_id
            ))?
            .path;

    let overlay_target_id = &host_config
        .storage
        .mount_points
        .iter()
        .find(|mp| mp.path == Path::new(TRIDENT_OVERLAY_PATH))
        .context(format!(
            "Cannot find overlay device mount point '{TRIDENT_OVERLAY_PATH}'"
        ))?
        .target_id;
    let overlay_device_path = modules::get_block_device(host_status, overlay_target_id, false)
        .context(format!(
            "Failed to find overlay device {}",
            overlay_target_id
        ))?
        .path;

    Ok((verity_data_path, verity_hash_path, overlay_device_path))
}

/// Update the root data, hash and overlay davice paths in the GRUB config,
/// along with the overlay configuration
pub(super) fn update_root_verity_in_grub_config(
    host_status: &HostStatus,
    host_config: &HostConfiguration,
    root_mount_path: &Path,
) -> Result<(), Error> {
    if host_config.storage.verity.is_empty() {
        return Ok(());
    }

    // We currently only support a single verity device, which is the root
    let verity_device = &host_config.storage.verity[0];

    let mut grub_config = GrubConfig::read(
        root_mount_path
            .join(BOOT_RELATIVE_MOUNT_POINT_PATH)
            .join(GRUB_CONFIG_PATH)
            .as_path(),
    )?;

    // Ensure there is only one linux command line
    grub_config.check_linux_command_line_count()?;

    let (verity_data_path, verity_hash_path, mnt_device_path) =
        get_verity_related_device_paths(host_status, host_config, verity_device)?;

    // Update the root data device path
    grub_config.update_linux_command_line_argument(
        VERITY_ROOT_DATA,
        verity_data_path.to_str().context(format!(
            "Failed to convert verity root data path '{}' to string",
            verity_data_path.display()
        ))?,
    )?;

    // Update the root hash device path
    grub_config.update_linux_command_line_argument(
        VERITY_ROOT_HASH,
        verity_hash_path.to_str().context(format!(
            "Failed to convert verity root hash path '{}' to string",
            verity_hash_path.display()
        ))?,
    )?;

    // Update the overlay configuration
    if grub_config.contains_linux_command_line_argument(OVERLAYS)? {
        grub_config.update_linux_command_line_argument(OVERLAYS, OVERLAYS_VALUE)?;
    } else {
        grub_config.append_linux_command_line_argument(OVERLAYS, OVERLAYS_VALUE)?;
    }

    // Update the overlay device path
    let volume_value = mnt_device_path.to_str().context(format!(
        "Failed to convert mnt device path '{}' to string",
        mnt_device_path.display()
    ))?;
    if grub_config.contains_linux_command_line_argument(OVERLAYFS_PERSISTENT_VOLUME)? {
        grub_config
            .update_linux_command_line_argument(OVERLAYFS_PERSISTENT_VOLUME, volume_value)?;
    } else {
        grub_config
            .append_linux_command_line_argument(OVERLAYFS_PERSISTENT_VOLUME, volume_value)?;
    }

    // Write down updated grub config
    grub_config.write()?;

    Ok(())
}

pub(super) fn stop_pre_existing_verity_devices(
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    // If no verity module is loaded, there are no verity devices to stop
    if !Path::new("/sys/module/dm_verity").exists() {
        return Ok(());
    }

    let root_verity_device_path = Path::new("/dev/mapper/root");

    // Check if the root verity device is present
    if !root_verity_device_path.exists() {
        return Ok(());
    }

    veritysetup::is_present().context("Unable to deactivate pre-existing dm-verity volumes.")?;

    let root_verity_device_status =
        veritysetup::status("root").context("Failed to get status of root verity device")?;
    let hc_disks = super::get_hostconfig_disk_paths(host_config)
        .context("Failed to get disks defined in Host Configuration")?;
    let mut verity_disks = HashSet::new();
    verity_disks.insert(
        block_devices::get_disk_for_partition(root_verity_device_status.data_device_path.as_path())
            .context(format!(
                "Failed to get disk for partition '{:?}'",
                root_verity_device_status.data_device_path
            ))?
            .canonicalize()?,
    );
    verity_disks.insert(
        block_devices::get_disk_for_partition(root_verity_device_status.hash_device_path.as_path())
            .context(format!(
                "Failed to get disk for partition '{:?}'",
                root_verity_device_status.data_device_path
            ))?
            .canonicalize()?,
    );

    if block_devices::can_stop_pre_existing_device(
        &verity_disks,
        &hc_disks.iter().cloned().collect::<HashSet<_>>(),
    )
    .context(format!(
        "Failed to stop verity device '{}'",
        root_verity_device_path.display()
    ))? {
        let result = lsblk::run(root_verity_device_path)?;
        if result.len() != 1 {
            return Err(anyhow::anyhow!(
                "Expected exactly one block device for verity device '{}', found {}",
                root_verity_device_path.display(),
                result.len()
            ));
        }
        let mount_points = &result[0].mountpoints;
        if !mount_points.is_empty() {
            for mount_point in mount_points.iter().flatten() {
                mount::umount(mount_point, true)?;
            }
        }
        veritysetup::close("root").context("Failed to close root verity device")?;
    }

    Ok(())
}

/// Ensure that if verity is enabled in the root filesystem, the host
/// configuration contains a verity definition as well. And vice-versa, ensure
/// that if verity is not enabled in the root filesystem, the host configuration
/// does not contain a verity configuration.
pub(super) fn validate_compatibility(
    host_config: &HostConfiguration,
    new_root: &Path,
) -> Result<(), Error> {
    if check_verity_enabled(new_root.join(GRUB2_CONFIG_RELATIVE_PATH).as_path())? {
        // If verity is enabled, we need to ensure that the verity definition is present in the
        // host configuration; API checks ensure that root verity is present
        // and correctly populated.
        if host_config.storage.verity.is_empty() {
            return Err(anyhow::anyhow!(
                "Verity is enabled for the root image, but no verity definition is present in the Host Configuration"
            ));
        }
    } else {
        // If verity is not enabled, we need to ensure that the verity definition is not present in
        // the host configuration.
        if !host_config.storage.verity.is_empty() {
            return Err(anyhow::anyhow!(
                "Verity is not enabled for the root image, but a verity definition is present in the Host Configuration"
            ));
        }
    }

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;

    use std::{fs, path::PathBuf, str::FromStr};

    use indoc;
    use maplit::btreemap;

    use trident_api::{
        config::{Disk, Partition, PartitionSize, PartitionType, Storage},
        status::{self, BlockDeviceContents},
    };

    fn get_original_grub_content() -> &'static str {
        indoc::indoc! {r#"
            set timeout=0
            set bootprefix=/boot
            search -n -u 9e6a9d2c-b7fe-4359-ac45-18b505e29d8b -s

            load_env -f $bootprefix/mariner.cfg
            if [ -f  $bootprefix/systemd.cfg ]; then
                    load_env -f $bootprefix/systemd.cfg
            else
                    set systemd_cmdline=net.ifnames=0
            fi
            if [ -f $bootprefix/grub2/grubenv ]; then
                    load_env -f $bootprefix/grub2/grubenv
            fi

            set rootdevice=PARTUUID=29f8eed2-3c85-4da0-b32e-480e54379766

            menuentry "CBL-Mariner" {
                    linux $bootprefix/$mariner_linux   rd.auto=1 root=$rootdevice $mariner_cmdline lockdown=integrity sysctl.kernel.unprivileged_bpf_disabled=1 $systemd_cmdline console=tty0 console=ttyS0 $kernelopts
                    if [ -f $bootprefix/$mariner_initrd ]; then
                            initrd $bootprefix/$mariner_initrd
                    fi
            }"#
        }
    }

    #[test]
    fn test_create_etc_overlay_mount_point() {
        assert_eq!(
            create_etc_overlay_mount_point(),
            MountPoint {
                path: PathBuf::from("/etc"),
                filesystem: "overlay".into(),
                options: vec![
                    "lowerdir=/etc".into(),
                    "upperdir=/var/lib/trident-overlay/etc/upper".into(),
                    "workdir=/var/lib/trident-overlay/etc/work".into(),
                    "ro".into()
                ],
                target_id: "".into()
            }
        );
    }

    #[test]
    fn test_check_verity_enabled() {
        let original_content_grub_boot = get_original_grub_content();
        let grub_config_file = tempfile::NamedTempFile::new().unwrap();
        fs::write(grub_config_file.path(), original_content_grub_boot).unwrap();

        assert!(!check_verity_enabled(grub_config_file.path()).unwrap());

        let mut grub_config = GrubConfig::read(grub_config_file.path()).unwrap();
        grub_config
            .append_linux_command_line_argument(VERITY_ENABLED, "1")
            .unwrap();
        grub_config.write().unwrap();

        assert!(check_verity_enabled(grub_config_file.path()).unwrap());

        grub_config
            .append_linux_command_line_argument(VERITY_ENABLED, "0")
            .unwrap();
        grub_config.write().unwrap();

        assert!(!check_verity_enabled(grub_config_file.path()).unwrap());

        grub_config
            .append_linux_command_line_argument(VERITY_ENABLED, "yes")
            .unwrap();
        grub_config.write().unwrap();

        assert!(check_verity_enabled(grub_config_file.path()).unwrap());

        grub_config
            .append_linux_command_line_argument(VERITY_ENABLED, "no")
            .unwrap();
        grub_config.write().unwrap();

        assert!(!check_verity_enabled(grub_config_file.path()).unwrap());

        // test non-existing input
        assert_eq!(
            check_verity_enabled(Path::new("/non-existing"))
                .unwrap_err()
                .root_cause()
                .to_string(),
            "GRUB config does not exist at path: '/non-existing'"
        );
    }

    #[test]
    fn test_get_verity_related_device_paths() {
        let host_config = HostConfiguration {
            storage: Storage {
                mount_points: vec![config::MountPoint {
                    path: PathBuf::from("/var/lib/trident-overlay"),
                    filesystem: "ext4".to_string(),
                    target_id: "overlay".to_string(),
                    options: vec!["defaults".to_string()],
                }],
                verity: vec![config::VerityDevice {
                    id: "root-verity".into(),
                    device_name: "root".into(),
                    data_target_id: "root".into(),
                    hash_target_id: "root-hash".into(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".into(),
                        device: "/dev/sdb".into(),
                        partitions: vec![
                            Partition {
                                id: "boot".into(),
                                size: PartitionSize::from_str("1M").unwrap(),
                                partition_type: PartitionType::Xbootldr,
                            },
                            Partition {
                                id: "root".into(),
                                size: PartitionSize::from_str("1G").unwrap(),
                                partition_type: PartitionType::Root,
                            },
                            Partition {
                                id: "root-hash".into(),
                                size: PartitionSize::from_str("1G").unwrap(),
                                partition_type: PartitionType::RootVerity,
                            },
                            Partition {
                                id: "overlay".into(),
                                size: PartitionSize::from_str("1G").unwrap(),
                                partition_type: PartitionType::LinuxGeneric,
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: status::Storage {
                block_devices: btreemap! {
                    "sdb".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb2"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb3"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb4"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let (verity_data_path, verity_hash_path, overlay_device_path) =
            get_verity_related_device_paths(
                &host_status,
                &host_config,
                &host_config.storage.verity[0],
            )
            .unwrap();
        assert_eq!(verity_data_path, PathBuf::from("/dev/sdb2"));
        assert_eq!(verity_hash_path, PathBuf::from("/dev/sdb3"));
        assert_eq!(overlay_device_path, PathBuf::from("/dev/sdb4"));

        // test no overlay mount point
        let mut host_config_no_overlay = host_config.clone();
        host_config_no_overlay
            .storage
            .mount_points
            .retain(|mp| mp.path != PathBuf::from("/var/lib/trident-overlay"));
        assert_eq!(
            get_verity_related_device_paths(
                &host_status,
                &host_config_no_overlay,
                &host_config.storage.verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Cannot find overlay device mount point '/var/lib/trident-overlay'"
        );

        // test no verity data target id
        let mut host_config_no_verity_data = host_config.clone();
        host_config_no_verity_data
            .storage
            .verity
            .get_mut(0)
            .unwrap()
            .data_target_id = "non-existing".into();
        assert_eq!(
            get_verity_related_device_paths(
                &host_status,
                &host_config_no_verity_data,
                &host_config_no_verity_data.storage.verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Failed to find verity data target id non-existing"
        );

        // test no verity hash target id
        let mut host_config_no_verity_hash = host_config.clone();
        host_config_no_verity_hash
            .storage
            .verity
            .get_mut(0)
            .unwrap()
            .hash_target_id = "non-existing".into();
        assert_eq!(
            get_verity_related_device_paths(
                &host_status,
                &host_config_no_verity_hash,
                &host_config_no_verity_hash.storage.verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Failed to find verity hash target id non-existing"
        );

        // test no overlay device
        let mut host_status_no_overlay = host_status.clone();
        host_status_no_overlay
            .spec
            .storage
            .disks
            .iter_mut()
            .find(|d| d.id == "sdb")
            .unwrap()
            .partitions
            .retain(|p| p.id != "overlay");
        host_status_no_overlay
            .storage
            .block_devices
            .remove("overlay");
        assert_eq!(
            get_verity_related_device_paths(
                &host_status_no_overlay,
                &host_config,
                &host_config.storage.verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Failed to find overlay device overlay"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use std::{
        fs::{self, read_to_string, File},
        io::Read,
        path::PathBuf,
    };

    use maplit::btreemap;

    use osutils::{
        files,
        hashing_reader::HashingReader,
        image_streamer,
        mount::{self, MountGuard},
        mountpoint,
        partition_types::DiscoverablePartitionType,
        repart::{RepartMode, RepartPartitionEntry, SystemdRepartInvoker},
        udevadm,
    };
    use trident_api::{
        config::{Disk, Partition, PartitionSize, PartitionType, Storage, VerityDevice},
        status::{self, BlockDeviceContents},
    };

    #[functional_test]
    fn test_validate_verity_compatibility() {
        let mut host_config = HostConfiguration::default();

        let new_root_dir = tempfile::tempdir().unwrap();

        assert_eq!(
            validate_compatibility(&host_config, new_root_dir.path())
                .unwrap_err()
                .root_cause()
                .to_string(),
            format!(
                "GRUB config does not exist at path: '{}/boot/grub2/grub.cfg'",
                new_root_dir.path().display()
            )
        );

        let config_dir_path = Path::new(new_root_dir.path()).join("boot/grub2");
        files::create_dirs(&config_dir_path).unwrap();
        let grub_config_path = config_dir_path.join("grub.cfg");
        files::write_file(&grub_config_path, 0o644, "".as_bytes()).unwrap();

        assert_eq!(
            validate_compatibility(&host_config, new_root_dir.path())
                .unwrap_err()
                .to_string(),
            format!(
                "Failed to find linux command line in '{}/boot/grub2/grub.cfg'",
                new_root_dir.path().display()
            )
        );

        files::write_file(
            &grub_config_path,
            0o644,
            indoc::indoc! {
                r#"
                    set root='hd0,gpt2'
                    linux /vmlinuz-5.4.0-1052-azure root=UUID
                "#
            }
            .as_bytes(),
        )
        .unwrap();

        validate_compatibility(&host_config, new_root_dir.path()).unwrap();

        host_config.storage.verity = vec![];
        validate_compatibility(&host_config, new_root_dir.path()).unwrap();

        host_config.storage.verity = vec![VerityDevice {
            id: "root".into(),
            device_name: "root".into(),
            data_target_id: "root".into(),
            hash_target_id: "root".into(),
        }];
        assert_eq!(
            validate_compatibility(&host_config, new_root_dir.path())
                .unwrap_err()
                .to_string(),
            "Verity is not enabled for the root image, but a verity definition is present in the Host Configuration"
        );

        // now enable verity in the grub config
        files::write_file(
            &grub_config_path,
            0o644,
            indoc::indoc! {
                r#"
                    set root='hd0,gpt2'
                    linux /vmlinuz-5.4.0-1052-azure root=UUID rd.systemd.verity=1
                "#
            }
            .as_bytes(),
        )
        .unwrap();

        validate_compatibility(&host_config, new_root_dir.path()).unwrap();

        let host_config = HostConfiguration::default();
        assert_eq!(
            validate_compatibility(&host_config, new_root_dir.path())
                .unwrap_err()
                .to_string(),
            "Verity is enabled for the root image, but no verity definition is present in the Host Configuration"
        );
    }

    fn setup_verity_images() -> PathBuf {
        let cdrom_mount_path = Path::new("/mnt/cdrom");
        if !cdrom_mount_path.exists() {
            files::create_dirs(cdrom_mount_path).unwrap();
        }
        if !mountpoint::check_is_mountpoint(cdrom_mount_path).unwrap() {
            mount::mount("/dev/sr0", cdrom_mount_path, "iso9660", &[]).unwrap();
        }

        let verity_data_path = cdrom_mount_path.join("data/verity_root.rawzst");
        assert!(verity_data_path.exists());

        let verity_hash_path = cdrom_mount_path.join("data/verity_roothash.rawzst");
        assert!(verity_hash_path.exists());

        let boot_path = cdrom_mount_path.join("data/verity_boot.rawzst");
        assert!(boot_path.exists());

        cdrom_mount_path.to_owned()
    }

    fn stream_zstd(image: &Path, destination: &Path) -> Result<(), Error> {
        let stream: Box<dyn Read> = Box::new(File::open(image)?);
        let reader = HashingReader::new(stream);
        image_streamer::stream_zstd(reader, destination, None)?;

        Ok(())
    }

    pub struct VerityGuard<'a> {
        pub device_name: &'a str,
    }

    impl<'a> Drop for VerityGuard<'a> {
        fn drop(&mut self) {
            veritysetup::close(self.device_name).unwrap();
        }
    }

    fn setup_verity_volumes() -> String {
        let cdrom_mount_path = setup_verity_images();

        let block_device_path = Path::new("/dev/sdb");

        let boot_path = cdrom_mount_path.join("data/verity_boot.rawzst");
        stream_zstd(boot_path.as_path(), block_device_path).unwrap();

        let expected_root_hash = {
            let boot_mount_dir = tempfile::tempdir().unwrap();
            // Mount image to temp dir
            mount::mount(block_device_path, boot_mount_dir.path(), "ext4", &[]).unwrap();

            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: boot_mount_dir.path(),
            };

            let mut grub_config =
                GrubConfig::read(boot_mount_dir.path().join("grub2/grub.cfg")).unwrap();
            grub_config
                .read_linux_command_line_argument("roothash")
                .unwrap()
        };

        let repart = SystemdRepartInvoker::new(block_device_path, RepartMode::Force)
            .with_partition_entries(vec![
                RepartPartitionEntry {
                    partition_type: DiscoverablePartitionType::Xbootldr,
                    label: None,
                    size_min_bytes: Some(1024 * 1024 * 1024),
                    size_max_bytes: None,
                },
                RepartPartitionEntry {
                    partition_type: DiscoverablePartitionType::RootVerity,
                    label: None,
                    size_min_bytes: Some(1024 * 1024 * 1024),
                    size_max_bytes: None,
                },
                RepartPartitionEntry {
                    partition_type: DiscoverablePartitionType::Root,
                    label: None,
                    // When min==max==None, it's a grow partition
                    size_min_bytes: None,
                    size_max_bytes: None,
                },
            ]);

        repart.execute().unwrap();
        udevadm::settle().unwrap();

        let verity_data_path = cdrom_mount_path.join("data/verity_root.rawzst");
        let verity_data_block_device_path = Path::new("/dev/sdb3");
        stream_zstd(verity_data_path.as_path(), verity_data_block_device_path).unwrap();
        let verity_hash_path = cdrom_mount_path.join("data/verity_roothash.rawzst");
        let verity_hash_block_device_path = Path::new("/dev/sdb2");
        stream_zstd(verity_hash_path.as_path(), verity_hash_block_device_path).unwrap();
        let verity_boot_path = cdrom_mount_path.join("data/verity_boot.rawzst");
        let verity_boot_block_device_path = Path::new("/dev/sdb1");
        stream_zstd(verity_boot_path.as_path(), verity_boot_block_device_path).unwrap();

        expected_root_hash
    }

    #[functional_test]
    fn test_get_root_verity_root_hash() {
        let expected_root_hash = setup_verity_volumes();

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from("/dev/sdb"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root-verity".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::Fixed(100),
                            },
                        ],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        config::MountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: "ext4".to_string(),
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        config::MountPoint {
                            path: PathBuf::from("/"),
                            filesystem: "ext4".to_string(),
                            target_id: "root".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: status::Storage {
                block_devices: btreemap! {
                    "sdb".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb"),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb1"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb2"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-verity".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb3"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            get_root_verity_root_hash(&host_status.spec, &host_status).unwrap(),
            expected_root_hash
        );

        // test failure on missing boot partition in config/status
        let mut host_status_no_boot_mount = host_status.clone();
        host_status_no_boot_mount
            .spec
            .storage
            .mount_points
            .retain(|mp| mp.path != PathBuf::from("/boot"));
        assert_eq!(
            get_root_verity_root_hash(&host_status_no_boot_mount.spec, &host_status_no_boot_mount)
                .unwrap_err()
                .to_string(),
            "Cannot find boot volume"
        );

        let mut host_status_no_boot_part = host_status.clone();
        host_status_no_boot_part
            .spec
            .storage
            .disks
            .iter_mut()
            .find(|d| d.id == "sdb")
            .unwrap()
            .partitions
            .retain(|p| p.id != "boot");
        host_status_no_boot_part
            .storage
            .block_devices
            .remove("boot");
        assert_eq!(
            get_root_verity_root_hash(&host_status_no_boot_part.spec, &host_status_no_boot_part)
                .unwrap_err()
                .to_string(),
            "Failed to find boot device boot"
        );

        // test failure when linux command line does not carry roothash argument
        {
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                Path::new("/dev/sdb1"),
                mount_dir.path(),
                "ext4",
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: mount_dir.path(),
            };

            let grub_config_path = mount_dir.path().join("grub2/grub.cfg");
            let grub_config = read_to_string(&grub_config_path).unwrap();
            let grub_config = grub_config.replace("roothash", "foobar");
            files::write_file(grub_config_path, 0o644, grub_config.as_bytes()).unwrap();
        }

        assert!(get_root_verity_root_hash(&host_status.spec, &host_status)
            .unwrap_err()
            .to_string()
            .starts_with("Failed to find 'roothash' on linux command line in '"));
    }

    #[functional_test]
    fn test_setup_root_verity_device() {
        let _expected_root_hash = setup_verity_volumes();

        let verity_device_path = Path::new("/dev/mapper/root");
        if verity_device_path.exists() {
            veritysetup::close("root").unwrap();
        }

        assert!(!verity_device_path.exists());

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from("/dev/sdb"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root-hash".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "overlay".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(100),
                            },
                        ],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        config::MountPoint {
                            path: PathBuf::from("/var/lib/trident-overlay"),
                            filesystem: "ext4".to_string(),
                            target_id: "overlay".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        config::MountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: "ext4".to_string(),
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    verity: vec![config::VerityDevice {
                        id: "root-verity".into(),
                        device_name: "root".into(),
                        data_target_id: "root".into(),
                        hash_target_id: "root-hash".into(),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: status::Storage {
                block_devices: btreemap! {
                    "sdb".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb"),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb1"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb2"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb3"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb4"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        {
            let (bdi, vd) = setup_root_verity_device(
                &host_status.spec,
                &host_status,
                &host_status.spec.storage.verity[0],
            )
            .unwrap();
            let _verityguard = VerityGuard {
                device_name: "root",
            };
            assert_eq!(bdi, "root-verity");
            assert!(verity_device_path.exists());
            assert_eq!(
                vd,
                BlockDeviceInfo {
                    path: PathBuf::from("/dev/mapper/root"),
                    size: 0,
                    contents: BlockDeviceContents::Initialized,
                }
            );
        }

        // test failure when root hash is not matching
        {
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                Path::new("/dev/sdb1"),
                mount_dir.path(),
                "ext4",
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
                    "roothash",
                    "4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076",
                )
                .unwrap();
            grub_config.write().unwrap();
        }

        assert_eq!(
            setup_root_verity_device(
                &host_status.spec,
                &host_status,
                &host_status.spec.storage.verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Failed to activate verity device 'root', status: 'corrupted'"
        );
        assert!(!verity_device_path.exists());
    }

    #[functional_test]
    fn test_setup_verity_devices() {
        // test no verity devices
        let mut host_status = HostStatus::default();
        setup_verity_devices(&Default::default(), &mut host_status).unwrap();

        assert!(host_status.storage.block_devices.is_empty());

        // test root verity device
        let _expected_root_hash = setup_verity_volumes();

        let verity_device_path = Path::new("/dev/mapper/root");
        if verity_device_path.exists() {
            veritysetup::close("root").unwrap();
        }

        assert!(!verity_device_path.exists());

        let host_status_golden = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from("/dev/sdb"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root-hash".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "overlay".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(100),
                            },
                        ],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        config::MountPoint {
                            path: PathBuf::from("/var/lib/trident-overlay"),
                            filesystem: "ext4".to_string(),
                            target_id: "overlay".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        config::MountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: "ext4".to_string(),
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    verity: vec![config::VerityDevice {
                        id: "root-verity".into(),
                        device_name: "root".into(),
                        data_target_id: "root".into(),
                        hash_target_id: "root-hash".into(),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: status::Storage {
                block_devices: btreemap! {
                    "sdb".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb"),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb1"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb2"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb3"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb4"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        {
            let mut host_status = host_status_golden.clone();
            setup_verity_devices(&host_status_golden.spec, &mut host_status).unwrap();
            let _verityguard = VerityGuard {
                device_name: "root",
            };
            assert!(verity_device_path.exists());
            assert_eq!(host_status.storage.block_devices.len(), 6);
            let verity_device = host_status
                .storage
                .block_devices
                .get("root-verity")
                .unwrap();
            assert_eq!(
                verity_device,
                &BlockDeviceInfo {
                    path: PathBuf::from("/dev/mapper/root"),
                    size: 0,
                    contents: BlockDeviceContents::Initialized,
                }
            );
        }

        // test failure when root hash is not matching
        {
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                Path::new("/dev/sdb1"),
                mount_dir.path(),
                "ext4",
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
                    "roothash",
                    "4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076",
                )
                .unwrap();
            grub_config.write().unwrap();
        }

        let mut host_status = host_status_golden.clone();
        assert_eq!(
            setup_verity_devices(&host_status_golden.spec, &mut host_status)
                .unwrap_err()
                .to_string(),
            "Failed to activate verity device 'root', status: 'corrupted'"
        );
        assert!(!verity_device_path.exists());
        assert_eq!(host_status.storage.block_devices.len(), 5);
        assert_eq!(
            host_status.storage.block_devices,
            host_status_golden.storage.block_devices
        );
    }

    #[functional_test]
    fn test_update_root_verity_in_grub_config() {
        setup_verity_volumes();

        // no change
        {
            let host_status = HostStatus::default();

            let mount_dir = tempfile::tempdir().unwrap();
            let boot_path = mount_dir.path().join("boot");
            files::create_dirs(&boot_path).unwrap();
            mount::mount(
                Path::new("/dev/sdb1"),
                &boot_path,
                "ext4",
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: boot_path.as_path(),
            };

            let grub_config_path = boot_path.join("grub2/grub.cfg");
            let grub_config_original = fs::read_to_string(&grub_config_path).unwrap();

            update_root_verity_in_grub_config(&host_status, &host_status.spec, mount_dir.path())
                .unwrap();

            let grub_config_updated = fs::read_to_string(grub_config_path).unwrap();
            assert_eq!(grub_config_original, grub_config_updated);
        }

        // updated
        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from("/dev/sdb"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root-hash".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "overlay".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(100),
                            },
                        ],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        config::MountPoint {
                            path: PathBuf::from("/var/lib/trident-overlay"),
                            filesystem: "ext4".to_string(),
                            target_id: "overlay".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        config::MountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: "ext4".to_string(),
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    verity: vec![config::VerityDevice {
                        id: "root-verity".into(),
                        device_name: "root".into(),
                        data_target_id: "root".into(),
                        hash_target_id: "root-hash".into(),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: status::Storage {
                block_devices: btreemap! {
                    "sdb".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb"),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb1"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb2"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb3"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb4"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        {
            let mount_dir = tempfile::tempdir().unwrap();
            let boot_path = mount_dir.path().join("boot");
            files::create_dirs(&boot_path).unwrap();
            mount::mount(
                Path::new("/dev/sdb1"),
                &boot_path,
                "ext4",
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: boot_path.as_path(),
            };

            update_root_verity_in_grub_config(&host_status, &host_status.spec, mount_dir.path())
                .unwrap();

            let grub_config_path = boot_path.join("grub2/grub.cfg");
            let mut grub_config = GrubConfig::read(grub_config_path).unwrap();

            assert_eq!(
                grub_config
                    .read_linux_command_line_argument("systemd.verity_root_data")
                    .unwrap(),
                "/dev/sdb3"
            );
            assert_eq!(
                grub_config
                    .read_linux_command_line_argument("systemd.verity_root_hash")
                    .unwrap(),
                "/dev/sdb2"
            );
            assert_eq!(
                grub_config
                    .read_linux_command_line_argument("rd.overlayfs")
                    .unwrap(),
                "etc,etc/upper,etc/work"
            );
            assert_eq!(
                grub_config
                    .read_linux_command_line_argument("rd.overlayfs_persistent_volume")
                    .unwrap(),
                "/dev/sdb4"
            );
        }

        // missing kernel argument
        {
            let mount_dir = tempfile::tempdir().unwrap();
            let boot_path = mount_dir.path().join("boot");
            files::create_dirs(&boot_path).unwrap();
            mount::mount(
                Path::new("/dev/sdb1"),
                &boot_path,
                "ext4",
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: boot_path.as_path(),
            };

            let grub_config_path = boot_path.join("grub2/grub.cfg");
            let mut grub_config = fs::read_to_string(&grub_config_path).unwrap();
            grub_config = grub_config.replace("systemd.verity_root_data", "foobar");
            files::write_file(grub_config_path, 0o644, grub_config.as_bytes()).unwrap();

            assert_eq!(update_root_verity_in_grub_config(&host_status, &host_status.spec, mount_dir.path())
                .unwrap_err().root_cause().to_string(), format!("Unable to find systemd.verity_root_data on linux command line in '{}/boot/grub2/grub.cfg'", mount_dir.path().display()));
        }
    }

    #[functional_test]
    fn test_stop_pre_existing_verity_devices() {
        setup_verity_volumes();
        let host_status_golden = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from("/dev/sdb"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root-hash".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(100),
                            },
                            Partition {
                                id: "overlay".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::Fixed(100),
                            },
                        ],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        config::MountPoint {
                            path: PathBuf::from("/var/lib/trident-overlay"),
                            filesystem: "ext4".to_string(),
                            target_id: "overlay".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                        config::MountPoint {
                            path: PathBuf::from("/boot"),
                            filesystem: "ext4".to_string(),
                            target_id: "boot".to_string(),
                            options: vec!["defaults".to_string()],
                        },
                    ],
                    verity: vec![config::VerityDevice {
                        id: "root-verity".into(),
                        device_name: "root".into(),
                        data_target_id: "root".into(),
                        hash_target_id: "root-hash".into(),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: status::Storage {
                block_devices: btreemap! {
                    "foo".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb"),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb1"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb2"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb3"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sdb4"),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // nothing mounted
        let verity_root_path = Path::new("/dev/mapper/root");
        assert!(!verity_root_path.exists());
        stop_pre_existing_verity_devices(&host_status_golden.spec).unwrap();

        // root verity opened
        {
            let mut host_status = host_status_golden.clone();
            setup_verity_devices(&host_status_golden.spec, &mut host_status).unwrap();
            assert!(verity_root_path.exists());
            stop_pre_existing_verity_devices(&host_status.spec).unwrap();
            assert!(!verity_root_path.exists());
        }

        // root verity opened & mounted
        {
            let mut host_status = host_status_golden.clone();
            setup_verity_devices(&host_status_golden.spec, &mut host_status).unwrap();
            assert!(verity_root_path.exists());
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                verity_root_path,
                mount_dir.path(),
                "ext4",
                &["defaults".into(), "ro".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes
            // out of scope
            let _mount_guard = MountGuard {
                mount_dir: mount_dir.path(),
            };
            stop_pre_existing_verity_devices(&host_status.spec).unwrap();
            assert!(!mountpoint::check_is_mountpoint(mount_dir.path()).unwrap());
            assert!(!verity_root_path.exists());
        }

        // TODO add across disks test
    }
}
