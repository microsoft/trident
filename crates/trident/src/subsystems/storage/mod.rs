use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use log::{debug, error, warn};

use osutils::lsblk;
use trident_api::{
    config::HostConfigurationDynamicValidationError,
    constants::internal_params::{ALLOW_HC_STORAGE_CHANGE, RELAXED_COSI_VALIDATION},
    error::{
        InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt,
        UnsupportedConfigurationError,
    },
    status::ServicingType,
    BlockDeviceId,
};

use crate::engine::{EngineContext, Subsystem};

mod encryption;
mod fstab;
mod osimage;
mod raid;
mod verity;

pub use fstab::DEFAULT_FSTAB_PATH;

const ENCRYPTION_SUBSYSTEM_NAME: &str = "encryption";
const OSIMAGE_SUBSYSTEM_NAME: &str = "osimage";

#[derive(Default, Debug)]
pub(crate) struct StorageSubsystem;
impl Subsystem for StorageSubsystem {
    fn name(&self) -> &'static str {
        "storage"
    }

    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.servicing_type != ServicingType::CleanInstall
            && !ctx.spec.internal_params.get_flag(ALLOW_HC_STORAGE_CHANGE)
        {
            // Ensure that relevant portions of the Host Configuration have not changed.
            if ctx.spec_old.storage.disks != ctx.spec.storage.disks
                || ctx.spec_old.storage.raid != ctx.spec.storage.raid
                || ctx.spec_old.storage.encryption != ctx.spec.storage.encryption
                || ctx.spec_old.storage.ab_update != ctx.spec.storage.ab_update
            {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::StorageConfigurationChanged,
                )));
            }

            // Ensure that all partitions still exist.
            let removed_block_devices: Vec<_> = ctx
                .partition_paths
                .iter()
                .filter(|&(_id, path)| !path.exists())
                .collect();
            for (id, path) in &removed_block_devices {
                error!(
                    "Partition '{id}' formerly with path '{}' no longer exists",
                    path.display()
                );
            }
            if !removed_block_devices.is_empty() {
                return Err(TridentError::new(
                    UnsupportedConfigurationError::PartitionsRemoved {
                        partition_ids: removed_block_devices
                            .into_iter()
                            .map(|(id, _path)| id.clone())
                            .collect(),
                    },
                ));
            }
        }

        // Ensure any two disks point to different devices. This requires canonicalizing the device
        // paths, which can only be done on the target system.
        let mut device_paths = HashMap::<PathBuf, BlockDeviceId>::new();
        for disk in &ctx.spec.storage.disks {
            let device_path = disk
                .device
                .canonicalize()
                .structured(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::InvalidDiskPath {
                        disk_id: disk.id.clone(),
                        disk_path: disk.device.to_string_lossy().to_string(),
                    },
                ))?;

            if !device_path.starts_with("/dev") {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::InvalidDiskBlockDevicePath {
                        name: disk.id.clone(),
                        device: device_path.display().to_string(),
                    },
                )));
            }

            if let Some(existing_disk_id) =
                device_paths.insert(device_path.clone(), disk.id.clone())
            {
                return Err(TridentError::new(InvalidInputError::from(
                    HostConfigurationDynamicValidationError::DiskDefinitionsReferToSameDevice {
                        disk1: existing_disk_id,
                        disk2: disk.id.clone(),
                        device: device_path.display().to_string(),
                    },
                )));
            }

            // If we are adopting partitions on a disk, ensure that the disk is GPT partitioned.
            if !disk.adopted_partitions.is_empty() {
                let disk_data =
                    lsblk::get(device_path.as_path()).structured(InvalidInputError::from(
                        HostConfigurationDynamicValidationError::GetBlockDeviceInfoForDisk {
                            disk_id: disk.id.clone(),
                        },
                    ))?;

                match disk_data.partition_table_type {
                    Some(lsblk::PartitionTableType::Gpt) => {}
                    _ => return Err(TridentError::new(InvalidInputError::from(
                        HostConfigurationDynamicValidationError::AdoptPartitionsOnNonGptPartitionedDisk {
                            disk_id: disk.id.clone(),
                        },
                    ))),
                }
            }
        }

        encryption::validate_host_config(ctx).message(format!(
            "Step 'Validate' failed for subunit '{ENCRYPTION_SUBSYSTEM_NAME}'"
        ))?;

        if let Err(err) = osimage::validate_host_config(ctx).message(format!(
            "Step 'Validate' failed for subunit '{OSIMAGE_SUBSYSTEM_NAME}'"
        )) {
            if ctx.spec.internal_params.get_flag(RELAXED_COSI_VALIDATION) {
                warn!(
                    "COSI validation failed, but '{RELAXED_COSI_VALIDATION}' is set. \
                    Continuing. Error: {}",
                    err.kind().to_string()
                );
            } else {
                return Err(err);
            }
        }

        Ok(())
    }

    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        if ctx.servicing_type == ServicingType::CleanInstall
            && ctx.storage_graph.root_fs_is_verity()
        {
            debug!("Root verity is enabled, setting up machine-id");
            verity::create_machine_id(mount_path).structured(ServicingError::CreateMachineId)?;
        }

        // Run encryption provisioning if encryption configuration is present
        if ctx.spec.storage.encryption.is_some() {
            debug!("Starting step 'Provision' for subunit '{ENCRYPTION_SUBSYSTEM_NAME}'");
            encryption::provision(ctx, mount_path).message(format!(
                "Step 'Provision' failed for subunit '{ENCRYPTION_SUBSYSTEM_NAME}'"
            ))?;
        }

        Ok(())
    }

    #[tracing::instrument(name = "storage_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        if ctx.is_uki()? && ctx.storage_graph.root_fs_is_verity() {
            debug!("Skipping storage configuration because UKI root-verity is in use");
            return Ok(());
        }

        if ctx.is_stream_image {
            debug!("Skipping storage configuration during stream-image");
            return Ok(());
        }

        if should_generate_fstab(ctx)? {
            fstab::generate_fstab(ctx, Path::new(fstab::DEFAULT_FSTAB_PATH)).structured(
                ServicingError::GenerateFstab {
                    fstab_path: fstab::DEFAULT_FSTAB_PATH.to_string(),
                },
            )?;
        } else {
            debug!("Skipping fstab generation because UKI usr-verity is in use");
        }

        // TODO: Update /etc/repart.d directly for the matching disk, derive it from where the root
        // is located

        encryption::configure(ctx).message(format!(
            "Step 'Configure' failed for subunit '{ENCRYPTION_SUBSYSTEM_NAME}'"
        ))?;

        // Persist on reboots
        raid::configure(ctx).structured(ServicingError::CreateMdadmConf)?;

        Ok(())
    }
}

/// Returns whether an fstab should be written for this image.
///
/// A UKI image using dm-verity takes its mount topology from the signed kernel
/// command line rather than from fstab: the verity device, its data and hash
/// devices, and the root device are all named there, and the image ships units
/// for whatever else it mounts. Writing an fstab would at best duplicate that,
/// and at worst override it, since `systemd-fstab-generator` emits units into
/// `/run/systemd/generator`, which takes precedence over the units an image
/// ships in `/usr/lib/systemd/system`.
///
/// Root-verity images never reach this: they skip storage configuration
/// entirely, having no writable root to configure. A usr-verity image does have
/// a writable root, so only fstab is skipped and the rest of the configuration
/// still applies.
fn should_generate_fstab(ctx: &EngineContext) -> Result<bool, TridentError> {
    Ok(!(ctx.is_uki()? && ctx.storage_graph.usr_fs_is_verity()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs::{self, Permissions},
        os::unix::fs::PermissionsExt,
        str::FromStr,
    };

    use tempfile::NamedTempFile;
    use url::Url;

    use osutils::encryption;
    use trident_api::{
        config::{
            AbUpdate, Disk as DiskConfig, Encryption, FileSystem, FileSystemSource,
            HostConfiguration, MountOptions, MountPoint, Partition as PartitionConfig,
            PartitionSize, PartitionTableType, PartitionType, Raid, RaidLevel, SoftwareRaidArray,
            Storage as StorageConfig, VerityDevice,
        },
        error::ErrorKind,
    };

    use sysdefs::tpm2::Pcr;

    fn get_ctx() -> EngineContext {
        EngineContext {
            servicing_type: ServicingType::CleanInstall,
            is_uki: Some(false),
            ..Default::default()
        }
    }

    /// Builds a context whose `/usr` is on a verity device, optionally inline.
    ///
    /// Inline verity (data device == hash device) is the shape Azure Container
    /// Linux uses; the separate-hash form is the conventional one.
    fn usr_verity_ctx(is_uki: bool, inline: bool) -> EngineContext {
        let mut partitions = vec![
            PartitionConfig {
                id: "esp".into(),
                partition_type: PartitionType::Esp,
                size: PartitionSize::from_str("100M").unwrap(),
                uuid: None,
                label: None,
            },
            PartitionConfig {
                id: "root".into(),
                partition_type: PartitionType::Root,
                size: PartitionSize::from_str("10G").unwrap(),
                uuid: None,
                label: None,
            },
            PartitionConfig {
                id: "usr-data".into(),
                partition_type: PartitionType::Usr,
                size: PartitionSize::from_str("1G").unwrap(),
                uuid: None,
                label: None,
            },
        ];
        if !inline {
            partitions.push(PartitionConfig {
                id: "usr-hash".into(),
                partition_type: PartitionType::UsrVerity,
                size: PartitionSize::from_str("100M").unwrap(),
                uuid: None,
                label: None,
            });
        }

        EngineContext {
            is_uki: Some(is_uki),
            ..Default::default()
        }
        .with_spec(HostConfiguration {
            storage: StorageConfig {
                disks: vec![DiskConfig {
                    id: "os".into(),
                    device: PathBuf::from("/dev/disk/by-bus/foobar"),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions,
                    ..Default::default()
                }],
                verity: vec![VerityDevice {
                    id: "usr".into(),
                    name: "usr".into(),
                    data_device_id: "usr-data".into(),
                    hash_device_id: if inline { "usr-data" } else { "usr-hash" }.into(),
                    ..Default::default()
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("esp".into()),
                        source: FileSystemSource::Image,
                        mount_point: Some("/boot/efi".into()),
                        is_esp: true,
                    },
                    FileSystem {
                        device_id: Some("root".into()),
                        source: FileSystemSource::Image,
                        mount_point: Some("/".into()),
                        is_esp: false,
                    },
                    FileSystem {
                        device_id: Some("usr".into()),
                        source: FileSystemSource::Image,
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/usr"),
                            options: MountOptions::defaults().with("ro"),
                        }),
                        is_esp: false,
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        })
    }

    /// A UKI image with usr-verity takes its mounts from the kernel command
    /// line, so no fstab is written. This is the Azure Container Linux case,
    /// whose root image ships no `/etc` for an fstab to be written into, and
    /// whose `/etc` is an overlay whose lower layer holds a deliberately empty
    /// fstab.
    #[test]
    fn test_should_not_generate_fstab_for_uki_usr_verity() {
        for inline in [true, false] {
            assert!(
                !should_generate_fstab(&usr_verity_ctx(true, inline)).unwrap(),
                "UKI usr-verity should not get an fstab (inline: {inline})"
            );
        }
    }

    /// Usr-verity without UKI still gets an fstab: the mounts are not coming
    /// from a signed kernel command line in that case.
    #[test]
    fn test_should_generate_fstab_for_non_uki_usr_verity() {
        assert!(should_generate_fstab(&usr_verity_ctx(false, true)).unwrap());
    }

    /// An ordinary image gets an fstab.
    #[test]
    fn test_should_generate_fstab_without_verity() {
        assert!(should_generate_fstab(&get_ctx()).unwrap());
    }

    // Create a temporary recovery key file. The file will be deleted once
    // the object returned is out of scope and dropped.
    pub fn get_recovery_key_file() -> NamedTempFile {
        let recovery_key_file: NamedTempFile = NamedTempFile::new().unwrap();
        let recovery_key_path: PathBuf = recovery_key_file.path().to_owned();
        fs::set_permissions(&recovery_key_path, Permissions::from_mode(0o600)).unwrap();
        encryption::generate_recovery_key_file(&recovery_key_path).unwrap();
        recovery_key_file
    }

    /// Produces a baseline Host Config with A/B update, encryption, and RAID.
    pub(super) fn get_host_config(recovery_key_file: &Path) -> HostConfiguration {
        HostConfiguration {
            storage: StorageConfig {
                disks: vec![
                    DiskConfig {
                        id: "disk1".to_owned(),
                        device: "/dev/sda".into(),
                        ..Default::default()
                    },
                    DiskConfig {
                        id: "disk2".to_owned(),
                        device: "/dev".into(),
                        partitions: vec![
                            PartitionConfig {
                                id: "part1".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("1M").unwrap(),
                                uuid: None,
                                label: None,
                            },
                            PartitionConfig {
                                id: "part2".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                                uuid: None,
                                label: None,
                            },
                            PartitionConfig {
                                id: "part3".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                                uuid: None,
                                label: None,
                            },
                            PartitionConfig {
                                id: "part4".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                                uuid: None,
                                label: None,
                            },
                            PartitionConfig {
                                id: "part5".to_owned(),
                                partition_type: PartitionType::Srv,
                                size: PartitionSize::from_str("1G").unwrap(),
                                uuid: None,
                                label: None,
                            },
                            PartitionConfig {
                                id: "data".to_owned(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::from_str("1M").unwrap(),
                                uuid: None,
                                label: None,
                            },
                            PartitionConfig {
                                id: "hash".to_owned(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: PartitionSize::from_str("1M").unwrap(),
                                uuid: None,
                                label: None,
                            },
                        ],
                        ..Default::default()
                    },
                ],
                raid: Raid {
                    software: vec![SoftwareRaidArray {
                        id: "my-raid1".to_owned(),
                        name: "my-raid".to_owned(),
                        level: RaidLevel::Raid1,
                        devices: vec!["part3".to_owned(), "part4".to_owned()],
                    }],
                    ..Default::default()
                },
                filesystems: vec![FileSystem {
                    device_id: Some("part1".to_owned()),
                    source: Default::default(),
                    mount_point: Some(MountPoint::from_str("/").unwrap()),
                    is_esp: false,
                }],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![trident_api::config::AbVolumePair {
                        id: "ab1".to_owned(),
                        volume_a_id: "part1".to_owned(),
                        volume_b_id: "part2".to_owned(),
                    }],
                }),
                encryption: Some(Encryption {
                    recovery_key_url: Some(Url::from_file_path(recovery_key_file).unwrap()),
                    volumes: vec![trident_api::config::EncryptedVolume {
                        id: "enc1".to_owned(),
                        device_name: "luks-enc".to_owned(),
                        device_id: "part5".to_owned(),
                    }],
                    pcrs: vec![Pcr::Pcr7],
                    ..Default::default()
                }),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Validates Storage subsystem HostConfiguration validation logic.
    #[test]
    fn test_validate_host_config_pass() {
        let mut ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        ctx.spec = get_host_config(recovery_key_file.path());

        StorageSubsystem.validate_host_config(&ctx).unwrap();
    }

    /// Invalid disk device path should fail validation.
    /// Disk device path should start with '/dev'.
    #[test]
    fn test_validate_host_config_invalid_disk_device_path_fail() {
        let mut ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        ctx.spec = get_host_config(recovery_key_file.path());

        ctx.spec.storage.disks.get_mut(0).unwrap().device = "/tmp".into();

        assert_eq!(
            StorageSubsystem
                .validate_host_config(&ctx)
                .unwrap_err()
                .kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::InvalidDiskBlockDevicePath {
                    name: "disk1".into(),
                    device: "/tmp".into(),
                }
            })
        );
    }

    // Disk devices must be unique.
    #[test]
    fn tests_validate_host_config_duplicate_disk_path_fail() {
        let mut ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        ctx.spec = get_host_config(recovery_key_file.path());

        ctx.spec.storage.disks.get_mut(0).unwrap().device = "/dev".into();

        assert_eq!(
            StorageSubsystem
                .validate_host_config(&ctx)
                .unwrap_err()
                .kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::DiskDefinitionsReferToSameDevice {
                    disk1: "disk1".into(),
                    disk2: "disk2".into(),
                    device: "/dev".into(),
                }
            })
        );
    }

    // Validating the Storage subsystem include encryption configuration validation.
    #[test]
    fn test_validate_host_config_encryption_invalid_fail() {
        let mut ctx = get_ctx();
        let recovery_key_file = get_recovery_key_file();
        ctx.spec = get_host_config(recovery_key_file.path());

        // Delete the recovery key file to make the encryption configuration invalid.
        fs::remove_file(recovery_key_file.path()).unwrap();

        assert_eq!(
            StorageSubsystem
                .validate_host_config(&ctx)
                .unwrap_err()
                .kind(),
            &ErrorKind::InvalidInput(InvalidInputError::InvalidHostConfigurationDynamic {
                inner: HostConfigurationDynamicValidationError::InvalidEncryptionKeyFilePath {
                    path: recovery_key_file.path().to_string_lossy().to_string(),
                }
            })
        );
    }
}
