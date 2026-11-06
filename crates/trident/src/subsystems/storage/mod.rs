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

        if should_generate_fstab(ctx) {
            fstab::generate_fstab(ctx, Path::new(fstab::DEFAULT_FSTAB_PATH)).structured(
                ServicingError::GenerateFstab {
                    fstab_path: fstab::DEFAULT_FSTAB_PATH.to_string(),
                },
            )?;
        } else {
            debug!("Skipping fstab generation: the image assembles /etc at boot and takes its mounts from the kernel command line");
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
/// Azure Container Linux assembles `/etc` at boot as an overlay over the
/// factory `/usr/share/distro/etc`, whose `fstab` is deliberately empty: ACL
/// mounts `/usr` and `/` from the signed UKI command line, and `/boot` and
/// `/oem` from units it ships. An fstab written here lands in the overlay's
/// upper layer, and `systemd-fstab-generator` turns it into units under
/// `/run/systemd/generator` that shadow the image's own.
///
/// Keyed on the distro rather than a proxy such as "UKI with verity on
/// `/usr`", since the cause is the shared `/etc`. Only ACL is handled, and
/// storage the Host Configuration adds beyond the image's own gets no entry,
/// so it does not persist across reboot.
fn should_generate_fstab(ctx: &EngineContext) -> bool {
    !ctx.image_distro().is_acl()
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

    use osutils::{
        encryption,
        osrelease::{Distro, OsRelease},
    };
    use trident_api::{
        config::{
            AbUpdate, Disk as DiskConfig, Encryption, FileSystem, HostConfiguration, MountPoint,
            Partition as PartitionConfig, PartitionSize, PartitionType, Raid, RaidLevel,
            SoftwareRaidArray, Storage as StorageConfig,
        },
        error::ErrorKind,
    };

    use sysdefs::tpm2::Pcr;

    use crate::osimage::{mock::MockOsImage, OsImage};

    fn get_ctx() -> EngineContext {
        EngineContext {
            servicing_type: ServicingType::CleanInstall,
            is_uki: Some(false),
            ..Default::default()
        }
    }

    /// Builds a context whose OS image reports the given distro identity.
    fn ctx_for_distro(id: &str, variant_id: Option<&str>) -> EngineContext {
        let os_release = OsRelease {
            id: Some(id.to_string()),
            variant_id: variant_id.map(str::to_string),
            ..OsRelease::EMPTY
        };

        EngineContext {
            image: Some(OsImage::mock(MockOsImage {
                os_release,
                ..MockOsImage::new()
            })),
            ..Default::default()
        }
    }

    /// ACL assembles `/etc` at boot and takes its mounts from the signed UKI
    /// command line, so an fstab written to the root filesystem would shadow
    /// the units the image ships.
    #[test]
    fn test_should_not_generate_fstab_for_acl() {
        let ctx = ctx_for_distro("azurelinux", Some("azurecontainerlinux"));
        assert_eq!(ctx.image_distro(), Distro::AzureContainerLinux);
        assert!(!should_generate_fstab(&ctx));
    }

    /// Azure Linux shares ACL's UKI machinery but not its `/etc` layout, so it
    /// still gets an fstab.
    #[test]
    fn test_should_generate_fstab_for_non_acl() {
        let ctx = ctx_for_distro("azurelinux", None);
        assert_ne!(ctx.image_distro(), Distro::AzureContainerLinux);
        assert!(should_generate_fstab(&ctx));
    }

    /// An image with no distro information gets an fstab.
    #[test]
    fn test_should_generate_fstab_without_distro() {
        assert!(should_generate_fstab(&get_ctx()));
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
