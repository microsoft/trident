use std::collections::HashSet;

use log::warn;
use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{
    constants::{
        internal_params::SELF_UPGRADE_TRIDENT, DEFAULT_CONFEXT_DIRECTORY, DEFAULT_SYSEXT_DIRECTORY,
    },
    is_default,
    storage_graph::graph::StorageGraph,
};

pub(crate) mod error;
pub(crate) mod health;
pub(crate) mod image;
pub(crate) mod internal_params;
pub(crate) mod os;
pub(crate) mod scripts;
pub(crate) mod storage;
pub(crate) mod trident;

use error::HostConfigurationStaticValidationError;
use health::Health;
use image::OsImage;
use internal_params::InternalParams;
use os::{ManagementOs, Os, SelinuxMode};
use scripts::Scripts;
use storage::Storage;
use trident::Trident;

/// HostConfiguration is the configuration for a host. Trident agent will use this to configure the host.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct HostConfiguration {
    /// The Trident Management configuration controls the installation of the
    /// Trident agent onto the target OS.
    #[serde(default, skip_serializing_if = "is_default")]
    pub trident: Trident,

    /// Describes the storage configuration of the host.
    #[serde(default, skip_serializing_if = "is_default")]
    pub storage: Storage,

    /// Optional scripts to be run after different Trident stages have completed.
    #[serde(default, skip_serializing_if = "is_default")]
    pub scripts: Scripts,

    /// OS Configuration for the target OS.
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

    /// Data about the image to deploy on the host, including sourcing and
    /// integrity information.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<OsImage>,

    /// Health configuration for the target OS.
    #[serde(default, skip_serializing_if = "is_default")]
    pub health: Health,
}

impl HostConfiguration {
    pub fn validate(&self) -> Result<(), HostConfigurationStaticValidationError> {
        let require_root_mount_point = self.trident != Trident::default()
            || self.scripts != Scripts::default()
            || self.os != Os::default()
            || self.os.netplan.is_some();
        let graph = self.storage.validate(require_root_mount_point)?;
        self.os.validate()?;
        self.scripts.validate()?;
        self.management_os.validate()?;
        self.trident.validate()?;

        self.validate_root_verity_config(&graph)?;

        self.validate_datastore_location()?;

        self.validate_extension_images_locations(&graph)?;

        Ok(())
    }

    /// Returns whether this Host Configuration has adopted partitions install.
    pub fn has_adopted_partitions(&self) -> bool {
        self.storage
            .disks
            .iter()
            .any(|disk| !disk.adopted_partitions.is_empty())
    }

    /// Performs extra checks required when using root-verity.
    fn validate_root_verity_config(
        &self,
        graph: &StorageGraph,
    ) -> Result<(), HostConfigurationStaticValidationError> {
        if !graph.root_fs_is_verity() {
            return Ok(());
        }

        // If self-upgrade is requested, ensure that root is not a RO verity filesystem b/c Trident
        // will not be able to copy itself into the FS.
        if self.internal_params.get_flag(SELF_UPGRADE_TRIDENT) {
            return Err(HostConfigurationStaticValidationError::SelfUpgradeOnReadOnlyRootVerityFs);
        }

        // Warn if SELinux is not disabled.
        if let Some(selinux_mode) = self.os.selinux.mode {
            if selinux_mode != SelinuxMode::Disabled {
                warn!(
                    "The use of SELinux with root-verity and grub is not supported. This \
                    configuration will only work with a UKI-based image."
                );
            }
        }

        Ok(())
    }

    fn validate_datastore_location(&self) -> Result<(), HostConfigurationStaticValidationError> {
        // Nothing to do if Trident is disabled on the target OS.
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

    /// Ensure that if A/B volumes are configured, any extension images are
    /// placed on an A/B volume and not on a shared partition.
    fn validate_extension_images_locations(
        &self,
        graph: &StorageGraph,
    ) -> Result<(), HostConfigurationStaticValidationError> {
        // This check is not required if no A/B volumes are configured.
        if self.storage.ab_update.is_none() {
            return Ok(());
        }

        // Find all directories in which sysexts or confexts will be placed.
        let mut dirs = HashSet::new();
        dirs.extend(
            self.os
                .sysexts
                .iter()
                .map(|ext| ext.path.clone().unwrap_or(DEFAULT_SYSEXT_DIRECTORY.into())),
        );
        dirs.extend(
            self.os
                .confexts
                .iter()
                .map(|ext| ext.path.clone().unwrap_or(DEFAULT_CONFEXT_DIRECTORY.into())),
        );

        for dir_path in dirs {
            let Some(fs_device_id) = self
                .storage
                .path_to_mount_point_info(&dir_path)
                .and_then(|mp| mp.device_id)
            else {
                return Err(
                    HostConfigurationStaticValidationError::ExtensionImageNotOnABVolume {
                        path: dir_path.display().to_string(),
                    },
                );
            };

            // Ensure that the extension image path is on an A/B update volume.
            // Sysexts and confexts must not be placed on shared partitions
            // since this may lead to unexpected behavior after an A/B update.
            if !graph.has_ab_capabilities(fs_device_id).unwrap_or(false) {
                return Err(
                    HostConfigurationStaticValidationError::ExtensionImageNotOnABVolume {
                        path: dir_path.display().to_string(),
                    },
                );
            }
        }
        Ok(())
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
                    "Failed to remove 'network' from required fields from definition '{key}'. Perhaps the API has changed?"
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
    use super::*;

    use std::path::{Path, PathBuf};

    use url::Url;

    use crate::{
        config::{
            AbUpdate, AbVolumePair, Disk, Extension, FileSystem, FileSystemSource, MountOptions,
            MountPoint, NewFileSystemType, Partition, PartitionTableType, PartitionType,
            VerityDevice,
        },
        constants::{
            internal_params::SELF_UPGRADE_TRIDENT, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH,
            TRIDENT_DATASTORE_PATH_DEFAULT,
        },
        primitives::hash::Sha384Hash,
    };

    #[test]
    fn test_validate_extension_image_location_success() {
        // Validate that validation passes with an empty Host Configuration
        let mut host_config = HostConfiguration::default();
        let graph = host_config.storage.build_graph().unwrap();
        host_config
            .validate_extension_images_locations(&graph)
            .unwrap();

        host_config.os.sysexts = vec![
            Extension {
                url: Url::parse("https://example.com/sysext1.raw").unwrap(),
                sha384: Sha384Hash::from("a".repeat(96)),
                path: None, // Defaults to a file inside /var/lib/extensions
            },
            Extension {
                url: Url::parse("https://example.com/sysext2.raw").unwrap(),
                sha384: Sha384Hash::from("b".repeat(96)),
                path: Some(PathBuf::from("/etc/extensions/sysext2.raw")),
            },
        ];
        host_config.os.confexts = vec![
            Extension {
                url: Url::parse("https://example.com/confext1.raw").unwrap(),
                sha384: Sha384Hash::from("c".repeat(96)),
                path: None, // Defaults to a file inside /var/lib/confexts
            },
            Extension {
                url: Url::parse("https://example.com/confext2.raw").unwrap(),
                sha384: Sha384Hash::from("d".repeat(96)),
                path: Some(PathBuf::from("/usr/lib/confexts/confext2.raw")),
            },
        ];

        // Validation should pass if no A/B volumes are configured.
        let graph = host_config.storage.build_graph().unwrap();
        host_config
            .validate_extension_images_locations(&graph)
            .unwrap();

        // Configure A/B volumes and ensure that /var/lib/extensions/ and
        // /var/lib/confexts are on A/B volumes.
        host_config.storage = Storage {
            disks: vec![Disk {
                id: "os".to_string(),
                device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                partition_table_type: PartitionTableType::Gpt,
                partitions: vec![
                    Partition {
                        id: "root-a".to_string(),
                        partition_type: PartitionType::Root,
                        size: 0x200000000.into(), // 8GiB
                    },
                    Partition {
                        id: "root-b".to_string(),
                        partition_type: PartitionType::Root,
                        size: 0x200000000.into(), // 8GiB
                    },
                    Partition {
                        id: "data-a".to_string(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: 0x200000000.into(), // 8GiB
                    },
                    Partition {
                        id: "data-b".to_string(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: 0x200000000.into(), // 8GiB
                    },
                ],
                adopted_partitions: vec![],
            }],
            filesystems: vec![
                FileSystem {
                    device_id: Some("root".to_owned()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/"),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("data".to_owned()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/data"),
                        options: MountOptions::empty(),
                    }),
                },
            ],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![
                    AbVolumePair {
                        id: "root".to_owned(),
                        volume_a_id: "root-a".to_owned(),
                        volume_b_id: "root-b".to_owned(),
                    },
                    AbVolumePair {
                        id: "data".to_owned(),
                        volume_a_id: "data-a".to_owned(),
                        volume_b_id: "data-b".to_owned(),
                    },
                ],
            }),
            ..Default::default()
        };

        // Validation passes with A/B volumes configured
        let graph = host_config.storage.build_graph().unwrap();
        host_config
            .validate_extension_images_locations(&graph)
            .unwrap();
    }

    #[test]
    fn test_validate_extension_image_location_failure() {
        let mut host_config = HostConfiguration::default();
        host_config.os.sysexts = vec![Extension {
            url: Url::parse("https://example.com/sysext1.raw").unwrap(),
            sha384: Sha384Hash::from("a".repeat(96)),
            path: None, // Defaults to a file inside /var/lib/extensions
        }];

        // /var/lib/extensions/ is not on a shared partition
        host_config.storage = Storage {
            disks: vec![Disk {
                id: "os".to_string(),
                device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                partition_table_type: PartitionTableType::Gpt,
                partitions: vec![
                    Partition {
                        id: "root-a".to_string(),
                        partition_type: PartitionType::Root,
                        size: 0x200000000.into(), // 8GiB
                    },
                    Partition {
                        id: "root-b".to_string(),
                        partition_type: PartitionType::Root,
                        size: 0x200000000.into(), // 8GiB
                    },
                    Partition {
                        id: "shared".to_string(),
                        partition_type: PartitionType::LinuxGeneric,
                        size: 0x200000000.into(), // 8GiB
                    },
                ],
                adopted_partitions: vec![],
            }],
            filesystems: vec![
                FileSystem {
                    device_id: Some("root".to_owned()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/"),
                        options: MountOptions::empty(),
                    }),
                },
                FileSystem {
                    device_id: Some("shared".to_owned()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/var/lib/extensions"),
                        options: MountOptions::empty(),
                    }),
                },
            ],
            ab_update: Some(AbUpdate {
                volume_pairs: vec![AbVolumePair {
                    id: "root".to_owned(),
                    volume_a_id: "root-a".to_owned(),
                    volume_b_id: "root-b".to_owned(),
                }],
            }),
            ..Default::default()
        };

        let graph = host_config.storage.build_graph().unwrap();
        assert_eq!(
            host_config
                .validate_extension_images_locations(&graph)
                .unwrap_err(),
            HostConfigurationStaticValidationError::ExtensionImageNotOnABVolume {
                path: "/var/lib/extensions/".to_string()
            }
        );
    }

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
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                    },
                    FileSystem {
                        device_id: Some("bar".into()),
                        mount_point: Some(MountPoint {
                            path: "/bar".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
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
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                    },
                    FileSystem {
                        device_id: Some("bar".into()),
                        mount_point: Some(MountPoint {
                            path: "/bar".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
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
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                    },
                    FileSystem {
                        device_id: Some("bar".into()),
                        mount_point: Some(MountPoint {
                            path: "/bar".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
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
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                    },
                    FileSystem {
                        device_id: Some("bar".into()),
                        mount_point: Some(MountPoint {
                            path: "/bar".into(),
                            options: MountOptions::defaults(),
                        }),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
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
    fn test_validate_root_verity_config() {
        // Empty host config
        let hc = HostConfiguration::default();
        let graph = hc.storage.build_graph().unwrap();
        hc.validate_root_verity_config(&graph)
            .expect("Empty host config should not return an error");

        // Host config with root-verity
        let mut host_config = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "os".to_string(),
                    device: "/dev/disk/by-path/pci-0000:00:1f.2-ata-2.0".into(),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "root-data".to_string(),
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
                verity: vec![VerityDevice {
                    id: "root".into(),
                    data_device_id: "root-data".into(),
                    hash_device_id: "root-hash".into(),
                    name: "root".into(),
                    ..Default::default()
                }],
                filesystems: vec![FileSystem {
                    device_id: Some("root".into()),
                    source: FileSystemSource::Image,
                    mount_point: Some(MountPoint {
                        path: ROOT_MOUNT_POINT_PATH.into(),
                        options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
                    }),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let graph = host_config.storage.build_graph().unwrap();

        // Check that if self-upgrade internal parameter is set, we return an error
        host_config
            .internal_params
            .set_flag(SELF_UPGRADE_TRIDENT.into());
        let validation_error = host_config.validate_root_verity_config(&graph).unwrap_err();
        assert_eq!(
            validation_error,
            HostConfigurationStaticValidationError::SelfUpgradeOnReadOnlyRootVerityFs
        );

        // Check that if self-upgrade internal parameter is not set, no error is returned
        host_config
            .internal_params
            .set_flag_false(SELF_UPGRADE_TRIDENT.into());
        host_config.validate_root_verity_config(&graph).unwrap();
    }
}
