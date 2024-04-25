//! Module in charge of configuring the Trident agent on the runtime OS.

use std::{
    fs::{self},
    path::Path,
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, info};
use trident_api::{
    config::{HostConfiguration, LocalConfigFile},
    status::{HostStatus, ReconcileState},
};

use crate::{
    modules::Module, TRIDENT_BINARY_PATH, TRIDENT_DATASTORE_PATH, TRIDENT_LOCAL_CONFIG_PATH,
};

#[derive(Default, Debug)]
pub struct ManagementModule;
impl Module for ManagementModule {
    fn name(&self) -> &'static str {
        "management"
    }

    fn validate_host_config(
        &self,
        host_status: &HostStatus,
        host_config: &HostConfiguration,
        _planned_update: ReconcileState,
    ) -> Result<(), Error> {
        if host_config.trident.disable {
            return Ok(());
        }

        let datastore_path = host_config
            .trident
            .datastore_path
            .as_deref()
            .unwrap_or(Path::new(TRIDENT_DATASTORE_PATH));
        if let Some(ref current_datastore_path) = host_status.trident.datastore_path {
            ensure!(
                current_datastore_path == datastore_path,
                "Datastore path cannot be changed"
            );
        }

        validate_datastore_location(datastore_path, host_config)?;

        Ok(())
    }

    fn provision(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
        mount_path: &Path,
    ) -> Result<(), Error> {
        if host_config.trident.disable {
            info!("Not provisioning management module as it is disabled");
            return Ok(());
        }

        host_status.trident.datastore_path = Some(
            host_config
                .trident
                .datastore_path
                .as_deref()
                .unwrap_or(Path::new(TRIDENT_DATASTORE_PATH))
                .to_owned(),
        );
        debug!("Datastore path: {:?}", host_status.trident.datastore_path);

        if host_config.trident.self_upgrade {
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
        _exec_root: &Path,
    ) -> Result<(), Error> {
        if host_config.trident.disable {
            return Ok(());
        }

        fs::create_dir_all(Path::new(TRIDENT_LOCAL_CONFIG_PATH).parent().unwrap())
            .context("Failed to create trident config directory")?;

        let datastore_path = host_status
            .trident
            .datastore_path
            .as_ref()
            .context("Datastore path missing from host status")?;

        create_trident_config(
            datastore_path,
            host_config,
            Path::new(TRIDENT_LOCAL_CONFIG_PATH),
        )?;
        debug!("Trident config created");

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
        .with_phonehome(host_config.trident.phonehome.clone())
        .with_grpc(if host_config.trident.enable_grpc {
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

    let datastore_block_device_id = &host_config
        .storage
        .path_to_mount_point(datastore_path)
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

#[cfg(test)]
mod tests {
    use trident_api::{
        config::{AbUpdate, FileSystemType, MountPoint, Storage},
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
                    filesystem: FileSystemType::Ext4,
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
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    },
                    MountPoint {
                        path: "/bar".into(),
                        target_id: "sda2".into(),
                        filesystem: FileSystemType::Ext4,
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
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    },
                    MountPoint {
                        path: "/bar".into(),
                        target_id: "sda2".into(),
                        filesystem: FileSystemType::Ext4,
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
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    },
                    MountPoint {
                        path: "/bar".into(),
                        target_id: "sda2".into(),
                        filesystem: FileSystemType::Ext4,
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
                        filesystem: FileSystemType::Ext4,
                        options: vec![],
                    },
                    MountPoint {
                        path: "/bar".into(),
                        target_id: "sda2".into(),
                        filesystem: FileSystemType::Ext4,
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
