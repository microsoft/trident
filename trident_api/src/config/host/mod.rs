use std::path::Path;

use log::warn;
use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{
    constants::{MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    is_default,
};

pub(crate) mod error;
pub(crate) mod image;
pub(crate) mod internal_params;
pub(crate) mod os;
pub(crate) mod scripts;
pub(crate) mod storage;
pub(crate) mod trident;

use image::OsImage;
use internal_params::InternalParams;
use os::{Os, SelinuxMode};
use scripts::Scripts;
use storage::Storage;
use trident::Trident;

use error::HostConfigurationStaticValidationError;

use self::os::ManagementOs;

/// HostConfiguration is the configuration for a host. Trident agent will use this to configure the host.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct HostConfiguration {
    /// The Trident Management configuration controls the installation of the
    /// Trident agent onto the runtime OS.
    #[serde(default, skip_serializing_if = "is_default")]
    pub trident: Trident,

    /// Describes the storage configuration of the host.
    #[serde(default, skip_serializing_if = "is_default")]
    pub storage: Storage,

    /// Optional scripts to be run after different Trident stages have completed.
    #[serde(default, skip_serializing_if = "is_default")]
    pub scripts: Scripts,

    /// OS Configuration for the runtime OS.
    #[serde(default, skip_serializing_if = "is_default")]
    pub os: Os,

    /// OS Configuration for the management OS.
    ///
    /// These settings are only applicable for clean install servicing. They are
    /// ignored on updates.
    #[serde(default, skip_serializing_if = "is_default")]
    pub management_os: ManagementOs,

    /// PREVIEW-ONLY: TODO: Remove before GA. (#9023)
    ///
    /// Extra parameters to override default trident behavior.
    #[serde(default, skip_serializing_if = "is_default")]
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub internal_params: InternalParams,

    /// OS Image
    #[serde(default, skip_serializing_if = "is_default")]
    #[cfg_attr(feature = "schemars", schemars(skip))]
    pub os_image: Option<OsImage>,
}

impl HostConfiguration {
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        let require_root_mount_point = self.trident != Trident::default()
            || self.scripts != Scripts::default()
            || self.os != Os::default()
            || self.os.network.is_some();
        self.storage.validate(require_root_mount_point)?;
        self.os.validate()?;
        self.scripts.validate()?;
        self.management_os.validate()?;
        self.trident.validate()?;

        // If self-upgrade is requested, ensure that root is not a RO verity filesystem b/c Trident
        // will not be able to copy itself into the FS.
        if self.trident.self_upgrade
            && self.storage.verity_filesystems.iter().any(|v| {
                v.mount_point.path == Path::new(ROOT_MOUNT_POINT_PATH)
                    && v.mount_point.options.contains(MOUNT_OPTION_READ_ONLY)
            })
        {
            return Err(HostConfigurationStaticValidationError::SelfUpgradeOnReadOnlyRootVerityFs);
        }

        self.validate_selinux_mode()?;
        self.validate_datastore_location()?;

        Ok(())
    }

    fn validate_selinux_mode(&self) -> Result<(), HostConfigurationStaticValidationError> {
        // If SELinux is in `enforcing` mode, ensure that verity filesystems are not used. Warn if
        // SELinux is in `permissive` mode.
        if !self.storage.verity_filesystems.is_empty() {
            match self.os.selinux.mode {
                Some(SelinuxMode::Enforcing) => {
                    return Err(
                        HostConfigurationStaticValidationError::VerityAndSelinuxUnsupported {
                            selinux_mode: SelinuxMode::Enforcing.to_string(),
                        },
                    );
                }
                Some(SelinuxMode::Permissive) => {
                    warn!("The use of SELinux with verity is not supported. SELinux mode is currently set to '{}', but should be 'disabled'.", SelinuxMode::Permissive.to_string());
                }
                _ => {}
            }
        }

        Ok(())
    }

    fn validate_datastore_location(&self) -> Result<(), HostConfigurationStaticValidationError> {
        // Nothing to do if trident is disabled on the runtime OS.
        if self.trident.disable {
            return Ok(());
        }

        let datastore_path = &self.trident.datastore_path;

        // Ensure that the datastore path is in a known volume.
        let datastore_block_device_id = &self
            .storage
            .path_to_mount_point_info(datastore_path)
            .and_then(|mp| mp.device_id)
            .ok_or(
                HostConfigurationStaticValidationError::DatastorePathNotInKnownVolume {
                    datastore_path: datastore_path.to_string_lossy().to_string(),
                },
            )?;

        // Ensure that the datastore path is not in an A/B update volume.
        if self
            .storage
            .ab_update
            .as_ref()
            .map(|ab| ab.volume_pairs.iter())
            .into_iter()
            .flatten()
            .any(|p| &p.id == *datastore_block_device_id)
        {
            return Err(
                HostConfigurationStaticValidationError::DatastorePathInABUpdateVolume {
                    datastore_path: datastore_path.to_string_lossy().to_string(),
                    volume_id: datastore_block_device_id.to_string(),
                },
            );
        }

        Ok(())
    }

    /// Populate internal configuration structures.
    ///
    /// This function assumes that the configuration has been validated.
    pub fn populate_internal(&mut self) {
        self.storage.populate_internal();
    }

    #[cfg(feature = "schemars")]
    pub fn generate_schema() -> schemars::schema::RootSchema {
        use schemars::schema::Schema;
        let mut schema =
            crate::schema_helpers::schema_generator().into_root_schema_for::<HostConfiguration>();

        // Because netplan-types currently does not support schemars, we have to
        // manually provide a placeholder schema for the netplan fields using
        // `schemars(with = "...")`. These are Option<> fields, but overriding
        // schematization using `with` removes this behavior. (is_option is a
        // "private" function in the JsonSchema trait) This means we have to
        // manually edit the schema to remove these two fields from the required
        // list.
        let remove_network = |schema: &mut schemars::schema::RootSchema, key: &str| {
            if let Some(Schema::Object(obj)) = schema.definitions.get_mut(key) {
                obj.object().required.remove("network");
            } else {
                panic!(
                    "Failed to remove 'network' from required fields from definition '{}'. Perhaps the API has changed?",
                    key
                );
            }
        };

        remove_network(&mut schema, "Os");
        remove_network(&mut schema, "ManagementOs");

        schema
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        config::{
            AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, FileSystemType, Image,
            ImageFormat, ImageSha256, MountOptions, MountPoint, Partition, PartitionTableType,
            PartitionType, VerityFileSystem,
        },
        constants::TRIDENT_DATASTORE_PATH_DEFAULT,
    };

    use super::*;

    #[test]
    fn test_validate_datastore_location() {
        // Datastore in default location
        HostConfiguration {
            storage: Storage {
                filesystems: vec![
                    FileSystem {
                        device_id: Some("root".into()),
                        mount_point: Some(MountPoint {
                            path: ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                    },
                    FileSystem {
                        device_id: Some("bar".into()),
                        mount_point: Some(MountPoint {
                            path: "/bar".into(),
                            options: MountOptions::defaults(),
                        }),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        }
        .validate_datastore_location()
        .unwrap();

        // Add AB Volume
        HostConfiguration {
            storage: Storage {
                filesystems: vec![
                    FileSystem {
                        device_id: Some("root".into()),
                        mount_point: Some(MountPoint {
                            path: ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                    },
                    FileSystem {
                        device_id: Some("bar".into()),
                        mount_point: Some(MountPoint {
                            path: "/bar".into(),
                            options: MountOptions::defaults(),
                        }),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![AbVolumePair {
                        id: "bar".into(),
                        volume_a_id: "barA".into(),
                        volume_b_id: "barB".into(),
                    }],
                }),
                ..Default::default()
            },
            ..Default::default()
        }
        .validate_datastore_location()
        .unwrap();

        // Make root an AB Volume, but move datastore to /bar
        HostConfiguration {
            storage: Storage {
                filesystems: vec![
                    FileSystem {
                        device_id: Some("root".into()),
                        mount_point: Some(MountPoint {
                            path: ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                    },
                    FileSystem {
                        device_id: Some("bar".into()),
                        mount_point: Some(MountPoint {
                            path: "/bar".into(),
                            options: MountOptions::defaults(),
                        }),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![AbVolumePair {
                        id: "root".into(),
                        volume_a_id: "roota".into(),
                        volume_b_id: "rootb".into(),
                    }],
                }),
                ..Default::default()
            },
            trident: Trident {
                datastore_path: Path::new("/bar/trident.sqlite").to_path_buf(),
                ..Default::default()
            },
            ..Default::default()
        }
        .validate_datastore_location()
        .unwrap();

        // Make root an AB Volume, but keep datastore in default location
        let err = HostConfiguration {
            storage: Storage {
                filesystems: vec![
                    FileSystem {
                        device_id: Some("root".into()),
                        mount_point: Some(MountPoint {
                            path: ROOT_MOUNT_POINT_PATH.into(),
                            options: MountOptions::defaults(),
                        }),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                    },
                    FileSystem {
                        device_id: Some("bar".into()),
                        mount_point: Some(MountPoint {
                            path: "/bar".into(),
                            options: MountOptions::defaults(),
                        }),
                        fs_type: FileSystemType::Ext4,
                        source: FileSystemSource::Create,
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![AbVolumePair {
                        id: "root".into(),
                        volume_a_id: "roota".into(),
                        volume_b_id: "rootb".into(),
                    }],
                }),
                ..Default::default()
            },
            ..Default::default()
        }
        .validate_datastore_location()
        .unwrap_err();

        assert_eq!(
            err,
            HostConfigurationStaticValidationError::DatastorePathInABUpdateVolume {
                datastore_path: TRIDENT_DATASTORE_PATH_DEFAULT.into(),
                volume_id: "root".into(),
            }
        );
    }

    #[test]
    fn test_validate_selinux_mode() {
        let mut host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "root".to_string(),
                            partition_type: PartitionType::Root,
                            size: 0x200000000.into(), // 8GiB
                        },
                        Partition {
                            id: "root-hash".to_string(),
                            partition_type: PartitionType::RootVerity,
                            size: 0x19000000.into(), // 400MiB
                        },
                    ],
                    adopted_partitions: vec![],
                }],
                verity_filesystems: vec![VerityFileSystem {
                    name: "root".into(),
                    data_device_id: "root".into(),
                    hash_device_id: "root-hash".into(),
                    data_image: Image {
                        url: "file:///trident_cdrom/data/verity_root.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "c2ce64662fbe2fa0b30a878c11aac71cb9f1ef27f59a157362ccc0881df47293"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                    },
                    hash_image: Image {
                        url: "file:///trident_cdrom/data/verity_roothash.rawzst".into(),
                        sha256: ImageSha256::Checksum(
                            "e875214b5ba8aac92203b72dbf0f78d673a16bd3c2a6f3577bcf4ed5d7c903af"
                                .into(),
                        ),
                        format: ImageFormat::RawZst,
                    },
                    fs_type: FileSystemType::Ext4,
                    mount_point: MountPoint {
                        path: "/".into(),
                        options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
                    },
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        // Check that 'enforcing' mode returns an error
        host_config.os.selinux.mode = Some(SelinuxMode::Enforcing);
        let validation_error = host_config.validate_selinux_mode().unwrap_err();
        assert_eq!(
            validation_error,
            HostConfigurationStaticValidationError::VerityAndSelinuxUnsupported {
                selinux_mode: SelinuxMode::Enforcing.to_string()
            },
            "{validation_error}"
        );

        // Check that 'permissive' mode does not return an error
        host_config.os.selinux.mode = Some(SelinuxMode::Permissive);
        host_config.validate_selinux_mode().unwrap();

        // Check that 'disabled' mode does not return an error
        host_config.os.selinux.mode = Some(SelinuxMode::Disabled);
        host_config.validate_selinux_mode().unwrap();
    }
}
