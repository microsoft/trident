use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Error};
use const_format::formatcp;
use log::debug;
use sys_mount::{Mount, MountFlags, UnmountFlags};
use tempfile::TempDir;

use osutils::{
    block_devices, exe::RunAndCheck, filesystems::MountFileSystemType, grub::GrubConfig, lsblk,
    mount, veritysetup,
};
use trident_api::{
    config::{self, HostConfiguration, InternalMountPoint},
    constants::{
        BOOT_MOUNT_POINT_PATH, BOOT_RELATIVE_MOUNT_POINT_PATH, DEV_MAPPER_PATH,
        GRUB2_CONFIG_FILENAME, GRUB2_CONFIG_RELATIVE_PATH, GRUB2_DIRECTORY, ROOT_MOUNT_POINT_PATH,
        TRIDENT_OVERLAY_LOWER_RELATIVE_PATH, TRIDENT_OVERLAY_PATH,
        TRIDENT_OVERLAY_UPPER_RELATIVE_PATH, TRIDENT_OVERLAY_WORK_RELATIVE_PATH,
    },
    status::{BlockDeviceContents, BlockDeviceInfo, HostStatus},
    BlockDeviceId,
};

use crate::modules;

use super::raid;

const GRUB_CONFIG_PATH: &str = formatcp!("{}/{}", GRUB2_DIRECTORY, GRUB2_CONFIG_FILENAME);

/// Indicates to dracut whether to activate verity. This is a boolean value.
const VERITY_ENABLED: &str = "rd.systemd.verity";

/// Points to a block device with root volume data.
const VERITY_ROOT_DATA: &str = "systemd.verity_root_data";

/// Points to a block device with root volume dm-verity hash tree.
const VERITY_ROOT_HASH: &str = "systemd.verity_root_hash";

/// Holds a comma-separated list of overlayfs paths.
const OVERLAYS: &str = "rd.overlayfs";

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
pub(super) fn create_etc_overlay_mount_point() -> InternalMountPoint {
    // inject the /etc overlay used for verity setups
    debug!("Creating /etc overlay mount point for verity setups");
    InternalMountPoint {
        filesystem: config::FileSystemType::Overlay,
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

pub(super) fn get_updated_device_name(device_name: &str) -> String {
    format!("{}_new", device_name)
}

pub(super) fn create_machine_id(new_root_path: &Path) -> Result<(), Error> {
    let machine_id_path = new_root_path.join("etc/machine-id");
    if machine_id_path.exists() {
        fs::remove_file(&machine_id_path).context(format!(
            "Failed to remove existing machine-id file at '{}'",
            machine_id_path.display()
        ))?;
    }
    Command::new("systemd-firstboot")
        .arg("--root")
        .arg(new_root_path)
        .arg("--setup-machine-id")
        .run_and_check()
        .context("Failed to generate machine-id")?;

    Ok(())
}

pub(super) fn configure_device_names(host_status: &mut HostStatus) -> Result<(), Error> {
    for vd in &host_status.spec.storage.internal_verity {
        host_status
            .storage
            .block_devices
            .get_mut(&vd.id)
            .context(format!("Failed to find verity device '{}'", vd.id))?
            .path = Path::new(DEV_MAPPER_PATH).join(&vd.device_name);
    }

    Ok(())
}

/// Setup the root verity device
fn setup_root_verity_device(
    host_status: &HostStatus,
    root_verity_device: &config::InternalVerityDevice,
) -> Result<(BlockDeviceId, BlockDeviceInfo), Error> {
    // Extract the root hash from GRUB config
    let root_hash = get_root_verity_root_hash(host_status)?;

    // Get the verity data and hash device paths from the host status
    let (verity_data_path, verity_hash_path, _) =
        get_verity_related_device_paths(host_status, root_verity_device)?;

    let updated_device_name = get_updated_device_name(&root_verity_device.device_name);

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
                    root_verity_device.device_name,
                    status.status
                ));
            }
        }
    }
    Ok((
        root_verity_device.id.clone(),
        BlockDeviceInfo {
            path: Path::new(DEV_MAPPER_PATH).join(updated_device_name),
            contents: BlockDeviceContents::Initialized,
            size: 0, // TODO: https://dev.azure.com/mariner-org/ECF/_workitems/edit/7319/
        },
    ))
}

/// Get the root verity root hash from the GRUB config
fn get_root_verity_root_hash(host_status: &HostStatus) -> Result<String, Error> {
    // API check ensures there is a boot volume, look up its mount point
    let boot_mount_point = &host_status
        .spec
        .storage
        .internal_mount_points
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
    let root_hash = grub_config.read_linux_command_line_argument("roothash")?;

    Ok(root_hash)
}

/// Setup verity devices; currently, only the root verity device is supported
#[tracing::instrument(skip_all)]
pub(super) fn setup_verity_devices(host_status: &mut HostStatus) -> Result<(), Error> {
    if host_status.spec.storage.internal_verity.is_empty() {
        return Ok(());
    }

    // Validated from API there is only one verity device at the moment and it
    // is tied to the root volume
    let root_verity_device = &host_status.spec.storage.internal_verity[0];
    let (id, verity_device_status) = setup_root_verity_device(host_status, root_verity_device)?;

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
    verity_device: &config::InternalVerityDevice,
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

    let overlay_target_id = &host_status
        .spec
        .storage
        .internal_mount_points
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
#[tracing::instrument(skip_all)]
pub(super) fn update_root_verity_in_grub_config(
    host_status: &HostStatus,
    root_mount_path: &Path,
) -> Result<(), Error> {
    if host_status.spec.storage.internal_verity.is_empty() {
        return Ok(());
    }

    // We currently only support a single verity device, which is the root
    let verity_device = &host_status.spec.storage.internal_verity[0];

    let mut grub_config = GrubConfig::read(
        root_mount_path
            .join(BOOT_RELATIVE_MOUNT_POINT_PATH)
            .join(GRUB_CONFIG_PATH),
    )?;

    // Ensure there is only one linux command line
    grub_config.check_linux_command_line_count()?;

    let (verity_data_path, verity_hash_path, mnt_device_path) =
        get_verity_related_device_paths(host_status, verity_device)?;

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

    // Dynamically build the OVERLAYS value including the mount device path
    let volume_value = mnt_device_path.to_str().context(format!(
        "Failed to convert mnt device path '{}' to string",
        mnt_device_path.display()
    ))?;
    let overlays_value = format!(
        "{},{},{},{}",
        TRIDENT_OVERLAY_LOWER_RELATIVE_PATH,
        TRIDENT_OVERLAY_UPPER_RELATIVE_PATH,
        TRIDENT_OVERLAY_WORK_RELATIVE_PATH,
        volume_value
    );

    // Update the overlay configuration
    if grub_config.contains_linux_command_line_argument(OVERLAYS)? {
        grub_config.update_linux_command_line_argument(OVERLAYS, &overlays_value)?;
    } else {
        grub_config.append_linux_command_line_argument(OVERLAYS, &overlays_value)?;
    }

    // Write down updated grub config
    grub_config.write()?;

    Ok(())
}

#[tracing::instrument(skip_all)]
pub(super) fn stop_pre_existing_verity_devices(
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    // If no verity module is loaded, there are no verity devices to stop
    if !Path::new("/sys/module/dm_verity").exists() {
        return Ok(());
    }

    debug!("Attempting to stop pre-existing verity devices");

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
        let block_device = lsblk::run(&root_verity_device_path)?;
        debug!(
            "Unmounting any mounted partitions on verity device '{}'",
            root_verity_device_path.display()
        );
        let mount_points = block_device.mountpoints;
        if !mount_points.is_empty() {
            for mount_point in mount_points.iter().flatten() {
                mount::umount(mount_point, true)?;
            }
        }
        debug!(
            "Deactivating verity device '{}'",
            root_verity_device_path.display()
        );
        veritysetup::close(&updated_device_name).context("Failed to close root verity device")?;
    }

    Ok(())
}

/// Ensure that if verity is enabled in the root filesystem, the host
/// configuration contains a verity definition as well. And vice-versa, ensure
/// that if verity is not enabled in the root filesystem, the host configuration
/// does not contain a verity configuration.
/// Returns true if verity is enabled, false if not enabled and error if there
/// is some indication of misconfiguration (e.g. images are verity enabled, but
/// HC is not and vice-versa).
pub(super) fn validate_compatibility(
    host_config: &HostConfiguration,
    new_root: &Path,
) -> Result<bool, Error> {
    if check_verity_enabled(&new_root.join(GRUB2_CONFIG_RELATIVE_PATH))? {
        // If verity is enabled, we need to ensure that the verity definition is present in the
        // host configuration; API checks ensure that root verity is present
        // and correctly populated.
        if host_config.storage.internal_verity.is_empty() {
            return Err(anyhow::anyhow!(
                "Verity is enabled for the root image, but no verity definition is present in the Host Configuration"
            ));
        }

        // The input configuration (HC+images) are correctly configured for
        // verity scenarios.
        Ok(true)
    } else {
        // If verity is not enabled, we need to ensure that the verity definition is not present in
        // the host configuration.
        if !host_config.storage.internal_verity.is_empty() {
            return Err(anyhow::anyhow!(
                "Verity is not enabled for the root image, but a verity definition is present in the Host Configuration"
            ));
        }

        // The input configuration (HC+images) do not expect verity scenarios
        // and are not attempting to use it.
        Ok(false)
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use std::{fs, path::PathBuf, str::FromStr};

    use indoc;
    use maplit::btreemap;

    use osutils::testutils::repart::TEST_DISK_DEVICE_PATH;
    use trident_api::{
        config::{Disk, FileSystemType, Partition, PartitionSize, PartitionType, Storage},
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
            InternalMountPoint {
                path: PathBuf::from("/etc"),
                filesystem: FileSystemType::Overlay,
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
        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".into(),
                        device: TEST_DISK_DEVICE_PATH.into(),
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
                    internal_mount_points: vec![config::InternalMountPoint {
                        path: PathBuf::from("/var/lib/trident-overlay"),
                        filesystem: FileSystemType::Ext4,
                        target_id: "overlay".to_string(),
                        options: vec!["defaults".to_string()],
                    }],
                    internal_verity: vec![config::InternalVerityDevice {
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
                        path: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
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
                &host_status.spec.storage.internal_verity[0],
            )
            .unwrap();
        assert_eq!(verity_data_path, PathBuf::from("/dev/sdb2"));
        assert_eq!(verity_hash_path, PathBuf::from("/dev/sdb3"));
        assert_eq!(overlay_device_path, PathBuf::from("/dev/sdb4"));

        // test no overlay mount point
        let mut host_status_no_overlay = host_status.clone();
        host_status_no_overlay
            .spec
            .storage
            .internal_mount_points
            .retain(|mp| mp.path != PathBuf::from("/var/lib/trident-overlay"));
        assert_eq!(
            get_verity_related_device_paths(
                &host_status_no_overlay,
                &host_status.spec.storage.internal_verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Cannot find overlay device mount point '/var/lib/trident-overlay'"
        );

        // test no verity data target id
        let mut host_status_no_verity_data = host_status.clone();
        host_status_no_verity_data
            .spec
            .storage
            .internal_verity
            .get_mut(0)
            .unwrap()
            .data_target_id = "non-existing".into();
        assert_eq!(
            get_verity_related_device_paths(
                &host_status_no_verity_data,
                &host_status_no_verity_data.spec.storage.internal_verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Failed to find verity data target id non-existing"
        );

        // test no verity hash target id
        let mut host_status_no_verity_hash = host_status.clone();
        host_status_no_verity_hash
            .spec
            .storage
            .internal_verity
            .get_mut(0)
            .unwrap()
            .hash_target_id = "non-existing".into();
        assert_eq!(
            get_verity_related_device_paths(
                &host_status_no_verity_hash,
                &host_status_no_verity_hash.spec.storage.internal_verity[0]
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
                &host_status_no_overlay.spec.storage.internal_verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Failed to find overlay device overlay"
        );
    }

    #[test]
    fn test_configure_device_names() {
        let mut host_status = HostStatus {
            spec: config::HostConfiguration {
                storage: config::Storage {
                    internal_verity: vec![
                        config::InternalVerityDevice {
                            id: "root".into(),
                            device_name: "root".into(),
                            data_target_id: "root".into(),
                            hash_target_id: "root".into(),
                        },
                        config::InternalVerityDevice {
                            id: "boot".into(),
                            device_name: "boot".into(),
                            data_target_id: "boot".into(),
                            hash_target_id: "boot".into(),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: status::Storage {
                block_devices: btreemap! {
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda2"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        configure_device_names(&mut host_status).unwrap();

        assert_eq!(
            host_status
                .spec
                .storage
                .internal_verity
                .iter()
                .find(|vd| vd.id == "root")
                .unwrap()
                .device_name,
            "root"
        );
        assert_eq!(
            host_status
                .spec
                .storage
                .internal_verity
                .iter()
                .find(|vd| vd.id == "boot")
                .unwrap()
                .device_name,
            "boot"
        );

        // test non-existing device
        let mut host_status_no_device = host_status.clone();
        host_status_no_device
            .spec
            .storage
            .internal_verity
            .get_mut(0)
            .unwrap()
            .id = "non-existing".into();
        assert_eq!(
            configure_device_names(&mut host_status_no_device)
                .unwrap_err()
                .to_string(),
            "Failed to find verity device 'non-existing'"
        );
    }

    #[test]
    fn test_get_updated_device_name() {
        assert_eq!(get_updated_device_name("root"), "root_new");
        assert_eq!(get_updated_device_name("foo"), "foo_new");
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

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
    use trident_api::{
        config::{
            Disk, FileSystemType, InternalVerityDevice, Partition, PartitionSize, PartitionType,
            Storage,
        },
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

        host_config.storage.internal_verity = vec![];
        validate_compatibility(&host_config, new_root_dir.path()).unwrap();

        host_config.storage.internal_verity = vec![InternalVerityDevice {
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

    #[functional_test]
    fn test_get_root_verity_root_hash() {
        let expected_root_hash = verity::setup_verity_volumes();

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
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
            storage: status::Storage {
                block_devices: btreemap! {
                    "sdb".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-verity".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            get_root_verity_root_hash(&host_status).unwrap(),
            expected_root_hash
        );

        // test failure on missing boot partition in config/status
        let mut host_status_no_boot_mount = host_status.clone();
        host_status_no_boot_mount
            .spec
            .storage
            .internal_mount_points
            .retain(|mp| mp.path != PathBuf::from("/boot"));
        assert_eq!(
            get_root_verity_root_hash(&host_status_no_boot_mount)
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
            get_root_verity_root_hash(&host_status_no_boot_part)
                .unwrap_err()
                .to_string(),
            "Failed to find boot device boot"
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
            let grub_config = grub_config.replace("roothash", "foobar");
            files::write_file(grub_config_path, 0o644, grub_config.as_bytes()).unwrap();
        }

        assert!(get_root_verity_root_hash(&host_status)
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

        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
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
                    internal_verity: vec![config::InternalVerityDevice {
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
                        path: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
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
                &host_status,
                &host_status.spec.storage.internal_verity[0],
            )
            .unwrap();
            let _verityguard = VerityGuard {
                device_name: "root_new",
            };
            assert_eq!(bdi, "root-verity");
            assert!(verity_device_path.exists());
            assert_eq!(
                vd,
                BlockDeviceInfo {
                    path: PathBuf::from(DEV_MAPPER_PATH).join("root_new"),
                    size: 0,
                    contents: BlockDeviceContents::Initialized,
                }
            );
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
                    "roothash",
                    "4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076",
                )
                .unwrap();
            grub_config.write().unwrap();
        }

        assert_eq!(
            setup_root_verity_device(&host_status, &host_status.spec.storage.internal_verity[0])
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
        setup_verity_devices(&mut host_status).unwrap();

        assert!(host_status.storage.block_devices.is_empty());

        // test root verity device
        let _expected_root_hash = verity::setup_verity_volumes();

        let verity_device_path = Path::new(DEV_MAPPER_PATH).join("root_new");
        if verity_device_path.exists() {
            veritysetup::close("root_new").unwrap();
        }

        assert!(!verity_device_path.exists());

        let host_status_golden = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
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
                    internal_verity: vec![config::InternalVerityDevice {
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
                        path: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
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
            setup_verity_devices(&mut host_status).unwrap();
            let _verityguard = VerityGuard {
                device_name: "root_new",
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
                    path: PathBuf::from(DEV_MAPPER_PATH).join("root_new"),
                    size: 0,
                    contents: BlockDeviceContents::Initialized,
                }
            );
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
                    "roothash",
                    "4392712ba01368efdf14b05c76f9e4df0d53664630b5d48632ed17a137f39076",
                )
                .unwrap();
            grub_config.write().unwrap();
        }

        let mut host_status = host_status_golden.clone();
        assert_eq!(
            setup_verity_devices(&mut host_status)
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
        verity::setup_verity_volumes();

        // no change
        {
            let host_status = HostStatus::default();

            let mount_dir = tempfile::tempdir().unwrap();
            let boot_path = mount_dir.path().join("boot");
            files::create_dirs(&boot_path).unwrap();
            mount::mount(
                Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                &boot_path,
                MountFileSystemType::Ext4,
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: &boot_path,
            };

            let grub_config_path = boot_path.join("grub2/grub.cfg");
            let grub_config_original = fs::read_to_string(&grub_config_path).unwrap();

            update_root_verity_in_grub_config(&host_status, mount_dir.path()).unwrap();

            let grub_config_updated = fs::read_to_string(grub_config_path).unwrap();
            assert_eq!(grub_config_original, grub_config_updated);
        }

        // updated
        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
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
                    internal_verity: vec![config::InternalVerityDevice {
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
                        path: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
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
                Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                &boot_path,
                MountFileSystemType::Ext4,
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: &boot_path,
            };

            update_root_verity_in_grub_config(&host_status, mount_dir.path()).unwrap();

            let grub_config_path = boot_path.join("grub2/grub.cfg");
            let mut grub_config = GrubConfig::read(grub_config_path).unwrap();

            assert_eq!(
                grub_config
                    .read_linux_command_line_argument("systemd.verity_root_data")
                    .unwrap(),
                formatcp!("{TEST_DISK_DEVICE_PATH}3")
            );
            assert_eq!(
                grub_config
                    .read_linux_command_line_argument("systemd.verity_root_hash")
                    .unwrap(),
                formatcp!("{TEST_DISK_DEVICE_PATH}2")
            );
            assert_eq!(
                grub_config
                    .read_linux_command_line_argument("rd.overlayfs")
                    .unwrap(),
                format!("etc,etc/upper,etc/work,{TEST_DISK_DEVICE_PATH}4")
            );
        }

        // missing kernel argument
        {
            let mount_dir = tempfile::tempdir().unwrap();
            let boot_path = mount_dir.path().join("boot");
            files::create_dirs(&boot_path).unwrap();
            mount::mount(
                Path::new(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                &boot_path,
                MountFileSystemType::Ext4,
                &["defaults".into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes out of scope
            let _mount_guard = MountGuard {
                mount_dir: &boot_path,
            };

            let grub_config_path = boot_path.join("grub2/grub.cfg");
            let mut grub_config = fs::read_to_string(&grub_config_path).unwrap();
            grub_config = grub_config.replace("systemd.verity_root_data", "foobar");
            files::write_file(grub_config_path, 0o644, grub_config.as_bytes()).unwrap();

            assert_eq!(update_root_verity_in_grub_config(&host_status, mount_dir.path())
                .unwrap_err().root_cause().to_string(), format!("Unable to find systemd.verity_root_data on linux command line in '{}/boot/grub2/grub.cfg'", mount_dir.path().display()));
        }
    }

    #[functional_test]
    fn test_stop_pre_existing_verity_devices() {
        verity::setup_verity_volumes();
        let host_status_golden = HostStatus {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
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
                    internal_verity: vec![config::InternalVerityDevice {
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
                        path: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        size: 300,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "boot".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root-hash".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "overlay".to_owned() => status::BlockDeviceInfo {
                        path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
                        size: 100,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // nothing mounted
        let verity_root_path = Path::new(DEV_MAPPER_PATH).join("root_new");
        assert!(!verity_root_path.exists());
        stop_pre_existing_verity_devices(&host_status_golden.spec).unwrap();

        // root verity opened
        {
            let mut host_status = host_status_golden.clone();
            setup_verity_devices(&mut host_status).unwrap();
            assert!(verity_root_path.exists());
            stop_pre_existing_verity_devices(&host_status.spec).unwrap();
            assert!(!verity_root_path.exists());
        }

        // root verity opened & mounted
        {
            let mut host_status = host_status_golden.clone();
            setup_verity_devices(&mut host_status).unwrap();
            assert!(verity_root_path.exists());
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                &verity_root_path,
                mount_dir.path(),
                MountFileSystemType::Ext4,
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

    #[functional_test]
    fn test_create_machine_id() {
        let root_dir = tempfile::tempdir().unwrap();
        let machine_id_path = root_dir.path().join("etc/machine-id");
        create_machine_id(root_dir.path()).unwrap();
        assert!(machine_id_path.exists());
        let machine_id = fs::read_to_string(&machine_id_path).unwrap();
        assert_eq!(machine_id.trim().len(), 32);

        create_machine_id(root_dir.path()).unwrap();
        assert!(machine_id_path.exists());
        let machine_id2 = fs::read_to_string(machine_id_path).unwrap();
        assert_eq!(machine_id2.trim().len(), 32);

        assert_ne!(machine_id, machine_id2);
    }
}
