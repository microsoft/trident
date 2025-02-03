use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use const_format::formatcp;
use log::{debug, info};

use osutils::{
    dependencies::Dependency,
    grub::GrubConfig,
    osrelease::{AzureLinuxRelease, Distro, OsRelease},
};
use trident_api::{
    config::{self, InternalMountPoint},
    constants::{
        BOOT_RELATIVE_MOUNT_POINT_PATH, GRUB2_CONFIG_FILENAME, GRUB2_CONFIG_RELATIVE_PATH,
        GRUB2_DIRECTORY, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH,
        TRIDENT_OVERLAY_LOWER_RELATIVE_PATH, TRIDENT_OVERLAY_PATH,
        TRIDENT_OVERLAY_UPPER_RELATIVE_PATH, TRIDENT_OVERLAY_WORK_RELATIVE_PATH,
    },
};

use crate::engine::EngineContext;

/// GRUB config path relative to the `/boot` directory.
const GRUB_CONFIG_PATH: &str = formatcp!("{}/{}", GRUB2_DIRECTORY, GRUB2_CONFIG_FILENAME);

/// Indicates to dracut whether to activate verity. This is a boolean value.
const KARG_VERITY_ENABLED: &str = "rd.systemd.verity";

/// Points to a block device with root volume data.
const KARG_VERITY_ROOT_DATA_DEV: &str = "systemd.verity_root_data";

/// Points to a block device with root volume dm-verity hash tree.
const KARG_VERITY_ROOT_HASH_DEV: &str = "systemd.verity_root_hash";

/// Holds a comma-separated list of overlayfs paths.
const KARG_OVERLAYS: &str = "rd.overlayfs";

/// Checks if verity is enabled in the GRUB config.
pub(super) fn check_verity_enabled(grub_config_path: &Path) -> Result<bool, Error> {
    debug!(
        "Reading GRUB config at path '{}'",
        grub_config_path.display(),
    );
    let mut grub_config = GrubConfig::read(grub_config_path)?;

    if !grub_config.contains_linux_command_line_argument(KARG_VERITY_ENABLED)? {
        return Ok(false);
    }

    let verity_value = grub_config.read_linux_command_line_argument(KARG_VERITY_ENABLED)?;

    Ok(verity_value == "1" || verity_value == "yes")
}

/// Create read-only /etc/ overlay mount point representation.
pub(super) fn create_etc_overlay_mount_point() -> InternalMountPoint {
    // inject the /etc overlay used for verity setups
    debug!("Creating /etc overlay mount point for verity setups");
    InternalMountPoint {
        filesystem: config::FileSystemType::Overlay,
        options: vec![
            format!("lowerdir=/{TRIDENT_OVERLAY_LOWER_RELATIVE_PATH}"),
            format!("upperdir={TRIDENT_OVERLAY_PATH}/{TRIDENT_OVERLAY_UPPER_RELATIVE_PATH}"),
            format!("workdir={TRIDENT_OVERLAY_PATH}/{TRIDENT_OVERLAY_WORK_RELATIVE_PATH}"),
            MOUNT_OPTION_READ_ONLY.to_owned(),
        ],
        target_id: "".to_owned(),
        path: PathBuf::from(ROOT_MOUNT_POINT_PATH).join(TRIDENT_OVERLAY_LOWER_RELATIVE_PATH),
    }
}

pub(super) fn create_machine_id(new_root_path: &Path) -> Result<(), Error> {
    let machine_id_path = new_root_path.join("etc/machine-id");
    if machine_id_path.exists() {
        fs::remove_file(&machine_id_path).context(format!(
            "Failed to remove existing machine-id file at '{}'",
            machine_id_path.display()
        ))?;
    }
    Dependency::SystemdFirstboot
        .cmd()
        .arg("--root")
        .arg(new_root_path)
        .arg("--setup-machine-id")
        .run_and_check()
        .context("Failed to generate machine-id")?;

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

/// Returns the device path of the block device which holds the verity overlay.
///
/// When root verity is used, Trident creates an overlay over the root filesystem to allow itself to
/// perform write operations. This overlay must be located on a writable filesystem, and thus
/// cannot be on the root partition itself.
fn get_verity_overlay_device_path(ctx: &EngineContext) -> Result<PathBuf, Error> {
    let overlay_target_id = &ctx
        .spec
        .storage
        .internal_mount_points
        .iter()
        .find(|mp| mp.path == Path::new(TRIDENT_OVERLAY_PATH))
        .context(format!(
            "'{TRIDENT_OVERLAY_PATH}' is not on a dedicated partition (currently required for dm-verity)"
        ))?
        .target_id;
    ctx.get_block_device_path(overlay_target_id).context(format!(
        "Failed to find device '{overlay_target_id}' which is supposed to be mounted at '{TRIDENT_OVERLAY_PATH}'",
    ))
}

/// Update the root data, hash and overlay davice paths in the GRUB config,
/// along with the overlay configuration.
#[tracing::instrument(name = "verity_configuration", skip_all)]
pub(super) fn configure(ctx: &EngineContext, root_mount_path: &Path) -> Result<(), Error> {
    if !ctx.spec.storage.has_verity_device() {
        return Ok(());
    }

    info!("Updating root verity configuration in GRUB config");

    // We currently only support a single verity device, which is the root
    let verity_device = &ctx
        .spec
        .storage
        .internal_verity
        .first()
        .or(ctx.spec.storage.verity.first())
        .context("No verity device found")?;

    let mut grub_config = GrubConfig::read(
        root_mount_path
            .join(BOOT_RELATIVE_MOUNT_POINT_PATH)
            .join(GRUB_CONFIG_PATH),
    )?;

    // Ensure there is only one linux command line
    grub_config.check_linux_command_line_count()?;

    let (verity_data_path, verity_hash_path) = get_verity_device_paths(ctx, verity_device)?;
    let mnt_device_path = get_verity_overlay_device_path(ctx)?;

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

    match OsRelease::read_root(root_mount_path)
        .context("Failed to determine the Linux distribution of the new OS")?
        .get_distro()
    {
        Distro::AzureLinux(AzureLinuxRelease::AzL2) => {
            info!("Updating GRUB config for Azure Linux 2.0");
            // Update the root data device path
            grub_config.update_linux_command_line_argument(
                KARG_VERITY_ROOT_DATA_DEV,
                verity_data_path.to_str().context(format!(
                    "Failed to convert verity root data path '{}' to string",
                    verity_data_path.display()
                ))?,
            )?;

            // Update the root hash device path
            grub_config.update_linux_command_line_argument(
                KARG_VERITY_ROOT_HASH_DEV,
                verity_hash_path.to_str().context(format!(
                    "Failed to convert verity root hash path '{}' to string",
                    verity_hash_path.display()
                ))?,
            )?;

            // Update the overlay configuration
            if grub_config.contains_linux_command_line_argument(KARG_OVERLAYS)? {
                grub_config.update_linux_command_line_argument(KARG_OVERLAYS, &overlays_value)?;
            } else {
                grub_config.append_linux_command_line_argument(KARG_OVERLAYS, &overlays_value)?;
            }

            // Write down updated GRUB config
            grub_config
                .write()
                .context("Failed to update GRUB config")?;
        }

        // In Azure Linux 3.0 Trident relies on OSModifier to update the GRUB config.
        Distro::AzureLinux(AzureLinuxRelease::AzL3) => {}

        distro => {
            bail!("Unsupported Linux distribution for verity setup: '{distro:?}'")
        }
    };

    Ok(())
}

/// Ensures that the Host Config and the provided image have matching verity
/// configurations. Returns whether verity is enabled, or error if there is some
/// indication of misconfiguration (e.g. images are verity enabled, but HC is
/// not and vice-versa).
pub(super) fn validate_verity_compatibility(
    ctx: &EngineContext,
    new_root: &Path,
) -> Result<bool, Error> {
    let root_verity_in_image = if let Some(os_img) = ctx.os_image.as_ref() {
        // Prefer checking the OS image for verity configuration when possible.
        os_img
            .root_filesystem()
            .with_context(|| {
                format!(
                    "Failed to get root filesystem from OS image '{}'",
                    os_img.source()
                )
            })?
            .verity
            .is_some()
    } else {
        // Fall back to the GRUB config when the OS image is not available.
        check_verity_enabled(&new_root.join(GRUB2_CONFIG_RELATIVE_PATH))?
    };

    match (root_verity_in_image, ctx.spec.storage.has_verity_device()) {
        // Image has verity but HC doesn't.
        (true, false) => bail!("Verity is enabled for the root image, but no verity definition is present in the Host Configuration"),

        // Image doesn't have verity but HC does.
        (false, true) => bail!("Verity is not enabled for the root image, but a verity definition is present in the Host Configuration"),

        // Verity and HC are in sync, return their state.
        _ => Ok(root_verity_in_image),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, path::PathBuf, str::FromStr};

    use maplit::btreemap;

    use config::HostConfiguration;
    use osutils::testutils::repart::TEST_DISK_DEVICE_PATH;
    use trident_api::config::{
        Disk, FileSystemType, Partition, PartitionSize, PartitionType, Storage,
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
                    MOUNT_OPTION_READ_ONLY.into()
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
            .append_linux_command_line_argument(KARG_VERITY_ENABLED, "1")
            .unwrap();
        grub_config.write().unwrap();

        assert!(check_verity_enabled(grub_config_file.path()).unwrap());

        grub_config
            .append_linux_command_line_argument(KARG_VERITY_ENABLED, "0")
            .unwrap();
        grub_config.write().unwrap();

        assert!(!check_verity_enabled(grub_config_file.path()).unwrap());

        grub_config
            .append_linux_command_line_argument(KARG_VERITY_ENABLED, "yes")
            .unwrap();
        grub_config.write().unwrap();

        assert!(check_verity_enabled(grub_config_file.path()).unwrap());

        grub_config
            .append_linux_command_line_argument(KARG_VERITY_ENABLED, "no")
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
        let ctx = EngineContext {
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
                "root".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                "root-hash".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}3")),
                "overlay".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
            },
            ..Default::default()
        };

        let (verity_data_path, verity_hash_path) =
            get_verity_device_paths(&ctx, &ctx.spec.storage.internal_verity[0]).unwrap();
        let overlay_device_path = get_verity_overlay_device_path(&ctx).unwrap();
        assert_eq!(verity_data_path, PathBuf::from("/dev/sdb2"));
        assert_eq!(verity_hash_path, PathBuf::from("/dev/sdb3"));
        assert_eq!(overlay_device_path, PathBuf::from("/dev/sdb4"));

        // test no overlay mount point
        let mut ctx_no_overlay = ctx.clone();
        ctx_no_overlay
            .spec
            .storage
            .internal_mount_points
            .retain(|mp| mp.path != PathBuf::from("/var/lib/trident-overlay"));
        assert_eq!(
            get_verity_overlay_device_path(&ctx_no_overlay)
                .unwrap_err()
                .to_string(),
            "'/var/lib/trident-overlay' is not on a dedicated partition (currently required for dm-verity)"
        );

        // test no verity data target id
        let mut ctx_no_verity_data = ctx.clone();
        ctx_no_verity_data
            .spec
            .storage
            .internal_verity
            .get_mut(0)
            .unwrap()
            .data_device_id = "non-existing".into();
        assert_eq!(
            get_verity_device_paths(
                &ctx_no_verity_data,
                &ctx_no_verity_data.spec.storage.internal_verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Failed to find path of verity data device with id 'non-existing'"
        );

        // test no verity hash target id
        let mut ctx_no_verity_hash = ctx.clone();
        ctx_no_verity_hash
            .spec
            .storage
            .internal_verity
            .get_mut(0)
            .unwrap()
            .hash_device_id = "non-existing".into();
        assert_eq!(
            get_verity_device_paths(
                &ctx_no_verity_hash,
                &ctx_no_verity_hash.spec.storage.internal_verity[0]
            )
            .unwrap_err()
            .to_string(),
            "Failed to find verity hash device with ID 'non-existing'"
        );

        // test no overlay device
        let mut ctx_no_overlay = ctx.clone();
        ctx_no_overlay
            .spec
            .storage
            .disks
            .iter_mut()
            .find(|d| d.id == "sdb")
            .unwrap()
            .partitions
            .retain(|p| p.id != "overlay");
        ctx_no_overlay.partition_paths.remove("overlay");
        assert_eq!(
            get_verity_overlay_device_path(&ctx_no_overlay,)
                .unwrap_err()
                .to_string(),
            "Failed to find device 'overlay' which is supposed to be mounted at '/var/lib/trident-overlay'"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::{fs, path::PathBuf};

    use config::HostConfiguration;
    use maplit::btreemap;

    use osutils::{
        files,
        filesystems::MountFileSystemType,
        mount::{self, MountGuard},
        testutils::{self, repart::TEST_DISK_DEVICE_PATH, verity},
    };
    use pytest_gen::functional_test;
    use trident_api::config::{Disk, FileSystemType, Partition, PartitionType, Storage};

    #[functional_test]
    fn test_update_root_verity_in_grub_config() {
        let _ = env_logger::builder()
            .is_test(true)
            .filter_level(log::LevelFilter::max())
            .try_init();
        info!("Set up logging in tests!");

        let _expected_root_hash = verity::setup_verity_volumes();

        // no change
        {
            let ctx = EngineContext::default();

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

            testutils::osrelease::make_mock_os_release(mount_dir.path(), AzureLinuxRelease::AzL2)
                .expect("Create mock os-release file");

            let grub_config_path = boot_path.join("grub2/grub.cfg");
            let grub_config_original = fs::read_to_string(&grub_config_path).unwrap();

            configure(&ctx, mount_dir.path()).unwrap();

            let grub_config_updated = fs::read_to_string(grub_config_path).unwrap();
            assert_eq!(grub_config_original, grub_config_updated);
        }

        // updated
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

            testutils::osrelease::make_mock_os_release(mount_dir.path(), AzureLinuxRelease::AzL2)
                .expect("Create mock os-release file");

            configure(&ctx, mount_dir.path()).unwrap();

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

            testutils::osrelease::make_mock_os_release(mount_dir.path(), AzureLinuxRelease::AzL2)
                .expect("Create mock os-release file");

            assert_eq!(configure(&ctx, mount_dir.path())
                .unwrap_err().root_cause().to_string(), format!("Unable to find systemd.verity_root_data on linux command line in '{}/boot/grub2/grub.cfg'", mount_dir.path().display()));
        }
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
