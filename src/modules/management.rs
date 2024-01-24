//! Module in charge of configuring the Trident agent on the runtime OS.

use std::{
    fs::{self, File},
    io::Write,
    os::unix::ffi::OsStrExt,
    path::Path,
};

use anyhow::{bail, ensure, Context, Error};
use log::{info, warn};
use trident_api::{
    config::{HostConfiguration, LocalConfigFile},
    error::{DatastoreError, ReportError, TridentError},
    status::{HostStatus, ReconcileState, UpdateKind},
};

use crate::{
    modules::Module, TRIDENT_BINARY_PATH, TRIDENT_DATASTORE_PATH, TRIDENT_LOCAL_CONFIG_PATH,
};

use super::storage::path_to_mount_point;

#[derive(Default, Debug)]
pub struct ManagementModule;
impl Module for ManagementModule {
    fn name(&self) -> &'static str {
        "management"
    }

    fn refresh_host_status(&mut self, _host_status: &mut HostStatus) -> Result<(), Error> {
        Ok(())
    }

    fn validate_host_config(
        &self,
        host_status: &HostStatus,
        host_config: &HostConfiguration,
        _planned_update: ReconcileState,
    ) -> Result<(), Error> {
        if host_config.management.disable {
            return Ok(());
        }

        let datastore_path = host_config
            .management
            .datastore_path
            .as_deref()
            .unwrap_or(Path::new(TRIDENT_DATASTORE_PATH));
        if let Some(ref current_datastore_path) = host_status.management.datastore_path {
            ensure!(
                current_datastore_path == datastore_path,
                "Datastore path cannot be changed"
            );
        }

        validate_datastore_location(datastore_path, host_config)?;

        Ok(())
    }

    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        None
    }

    fn provision(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        mount_path: &Path,
    ) -> Result<(), Error> {
        if host_config.management.disable {
            return Ok(());
        }

        host_status.management.datastore_path = Some(
            host_config
                .management
                .datastore_path
                .as_deref()
                .unwrap_or(Path::new(TRIDENT_DATASTORE_PATH))
                .to_owned(),
        );

        if host_config.management.self_upgrade {
            info!("Copying Trident binary to runtime OS");
            fs::copy(
                TRIDENT_BINARY_PATH,
                mount_path.join(&TRIDENT_BINARY_PATH[1..]),
            )
            .context("Failed to copy Trident binary to runtime OS")?;
        }

        Ok(())
    }

    fn configure(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        if host_config.management.disable {
            return Ok(());
        }

        fs::create_dir_all(Path::new(TRIDENT_LOCAL_CONFIG_PATH).parent().unwrap())
            .context("Failed to create trident config directory")?;

        let datastore_path = host_status
            .management
            .datastore_path
            .as_ref()
            .context("Datastore path missing from host status")?;

        create_trident_config(
            datastore_path,
            host_config,
            Path::new(TRIDENT_LOCAL_CONFIG_PATH),
        )?;
        Ok(())
    }
}

pub(super) fn create_trident_config(
    datastore_path: &Path,
    host_config: &HostConfiguration,
    trident_config_path: &Path,
) -> Result<(), Error> {
    let trident_config = LocalConfigFile::default()
        .with_datastore(datastore_path.to_path_buf())
        .with_phonehome(host_config.management.phonehome.clone())
        .with_grpc(if host_config.management.enable_grpc {
            Some(Default::default())
        } else {
            None
        })
        .with_host_configuration(host_config.clone());
    fs::write(
        trident_config_path,
        serde_yaml::to_string(&trident_config).context("Failed to serialize trident config")?,
    )
    .context("Failed to write Trident Configuration")?;

    Ok(())
}

fn validate_datastore_location(
    datastore_path: &Path,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    datastore_path
        .extension()
        .and_then(|ext| if ext == "sqlite" { Some(()) } else { None })
        .ok_or(anyhow::anyhow!(
            "Datastore path must end with '.sqlite' but received '{}'",
            datastore_path.display()
        ))?;

    let datastore_block_device_id = &path_to_mount_point(host_config, datastore_path)
        .map(|mp| &mp.target_id)
        .context("Failed to find mount point for datastore")?;

    if host_config
        .storage
        .ab_update
        .as_ref()
        .and_then(|ab_update| {
            ab_update
                .volume_pairs
                .iter()
                .find(|p| &p.id == *datastore_block_device_id)
        })
        .is_some()
    {
        bail!("Datastore cannot be on an A/B update volume");
    }
    Ok(())
}

/// Write the location of the datastore to the open file handle.
///
/// This function is used to record the location of the datastore on the provisioning OS's
/// filesystem so that the `trident get` command knows where to find it.
pub(super) fn record_datastore_location(
    host_status: &HostStatus,
    datastore_path: &Path,
    mut datastore_ref: File,
) -> Result<(), TridentError> {
    info!("Recording datastore location");
    let (device, relative_path) = host_status
        .storage
        .get_mount_point_and_relative_path(datastore_path)
        .structured(DatastoreError::RecordDatastoreLocation)?;
    let Some(partition) = &host_status.storage.get_partition_ref(&device.target_id) else {
        // TODO(6623, 6624): Handle datastore being on RAID arrays or encrypted volumes.
        warn!("Datastore is not on a partition, cannot record location");
        return Ok(());
    };
    datastore_ref
        .write_all(partition.path.as_os_str().as_bytes())
        .structured(DatastoreError::RecordDatastoreLocation)?;
    datastore_ref
        .write_all(b"\n")
        .structured(DatastoreError::RecordDatastoreLocation)?;
    datastore_ref
        .write_all(relative_path.as_os_str().as_bytes())
        .structured(DatastoreError::RecordDatastoreLocation)?;
    datastore_ref
        .sync_all()
        .structured(DatastoreError::RecordDatastoreLocation)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use trident_api::{
        config::{AbUpdate, MountPoint, Storage},
        constants,
    };

    use super::*;

    #[test]
    fn test_validate_datastore_location() {
        let host_config = HostConfiguration {
            storage: Storage {
                mount_points: vec![MountPoint {
                    path: constants::ROOT_MOUNT_POINT_PATH.into(),
                    target_id: "sda1".into(),
                    filesystem: "ext4".into(),
                    options: vec![],
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        validate_datastore_location(Path::new("/trident.sqlite"), &host_config).unwrap();
        validate_datastore_location(Path::new("/foo/trident.sqlite"), &host_config).unwrap();
        validate_datastore_location(Path::new("/var/lib/trident/datastore.sqlite"), &host_config)
            .unwrap();

        // expect failure as the datastore path needs to end with .sqlite
        assert!(validate_datastore_location(Path::new("/trident"), &host_config).is_err());

        let host_config = HostConfiguration {
            storage: Storage {
                mount_points: vec![
                    MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        target_id: "sda1".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                    MountPoint {
                        path: "/bar".into(),
                        target_id: "sda2".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        validate_datastore_location(Path::new("/foo/bar/trident.sqlite"), &host_config).unwrap();

        let host_config = HostConfiguration {
            storage: Storage {
                mount_points: vec![
                    MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        target_id: "sda1".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                    MountPoint {
                        path: "/bar".into(),
                        target_id: "sda2".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![
                        trident_api::config::AbVolumePair {
                            id: "sda2".into(),
                            volume_a_id: "sda1".into(),
                            volume_b_id: "sda2".into(),
                        },
                        trident_api::config::AbVolumePair {
                            id: "sda2".into(),
                            volume_a_id: "sda2".into(),
                            volume_b_id: "sda1".into(),
                        },
                    ],
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        validate_datastore_location(Path::new("/foo/bar/trident.sqlite"), &host_config).unwrap();

        let host_config = HostConfiguration {
            storage: Storage {
                mount_points: vec![
                    MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        target_id: "sda1".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                    MountPoint {
                        path: "/bar".into(),
                        target_id: "sda2".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![
                        trident_api::config::AbVolumePair {
                            id: "sda1".into(),
                            volume_a_id: "sda1".into(),
                            volume_b_id: "sda2".into(),
                        },
                        trident_api::config::AbVolumePair {
                            id: "sda1".into(),
                            volume_a_id: "sda2".into(),
                            volume_b_id: "sda1".into(),
                        },
                    ],
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        validate_datastore_location(Path::new("/bar/foo/trident.sqlite"), &host_config).unwrap();

        let host_config = HostConfiguration {
            storage: Storage {
                mount_points: vec![
                    MountPoint {
                        path: constants::ROOT_MOUNT_POINT_PATH.into(),
                        target_id: "sda1".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                    MountPoint {
                        path: "/bar".into(),
                        target_id: "sda2".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                ],
                ab_update: Some(AbUpdate {
                    volume_pairs: vec![
                        trident_api::config::AbVolumePair {
                            id: "sda1".into(),
                            volume_a_id: "sda1".into(),
                            volume_b_id: "sda2".into(),
                        },
                        trident_api::config::AbVolumePair {
                            id: "sda2".into(),
                            volume_a_id: "sda2".into(),
                            volume_b_id: "sda1".into(),
                        },
                    ],
                }),
                ..Default::default()
            },
            ..Default::default()
        };

        // expect failure, as we cannot land on A/B volume
        assert!(validate_datastore_location(
            Path::new(constants::ROOT_MOUNT_POINT_PATH),
            &host_config
        )
        .is_err());
    }
}

#[cfg(feature = "functional-tests")]
#[cfg_attr(not(test), allow(unused_imports))]
mod functional_tests {
    use super::*;
    use maplit::btreemap;
    use pytest_gen::pytest;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;
    use trident_api::{
        config::PartitionType,
        status::{BlockDeviceContents, Disk, MountPoint, Partition, Storage},
    };
    use uuid::Uuid;

    #[pytest()]
    fn test_record_datastore_location() {
        let host_status = HostStatus {
            storage: Storage {
                disks: btreemap! {
                    "os".into() => Disk {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        uuid: Uuid::nil(),
                        capacity: 0,
                        contents: BlockDeviceContents::Unknown,
                        partitions: vec![
                            Partition {
                                id: "efi".to_string(),
                                path: PathBuf::from("/dev/disk/by-partlabel/a"),
                                contents: BlockDeviceContents::Unknown,
                                start: 0,
                                end: 0,
                                ty: PartitionType::Esp,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "root".to_string(),
                                path: PathBuf::from("/dev/disk/by-partlabel/b"),
                                contents: BlockDeviceContents::Unknown,
                                start: 100,
                                end: 1000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                            Partition {
                                id: "var".to_string(),
                                path: PathBuf::from("/dev/disk/by-partlabel/c"),
                                contents: BlockDeviceContents::Unknown,
                                start: 1000,
                                end: 10000,
                                ty: PartitionType::Root,
                                uuid: Uuid::nil(),
                            },
                        ],
                    },
                },
                mount_points: btreemap! {
                    PathBuf::from("/") => MountPoint {
                        target_id: "root".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                    PathBuf::from("/var") => MountPoint {
                        target_id: "var".into(),
                        filesystem: "ext4".into(),
                        options: vec![],
                    },
                    PathBuf::from("/boot/efi") => MountPoint {
                        target_id: "efi".into(),
                        filesystem: "vfat".into(),
                        options: vec![],
                    },
                },
                ..Default::default()
            },
            reconcile_state: ReconcileState::CleanInstall,
            ..Default::default()
        };

        let datastore_ref = NamedTempFile::new().unwrap();

        record_datastore_location(
            &host_status,
            Path::new(crate::TRIDENT_DATASTORE_PATH),
            datastore_ref.reopen().unwrap(),
        )
        .unwrap();

        let contents = fs::read(datastore_ref.path()).unwrap();
        assert_eq!(
            contents,
            b"/dev/disk/by-partlabel/c\nlib/trident/datastore.sqlite"
        )
    }
}
