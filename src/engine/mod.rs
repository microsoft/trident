use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
    thread,
    time::Duration,
};

use chrono::Utc;
use log::{debug, error, info, warn};

use osutils::{dependencies::Dependency, path::join_relative};
use trident_api::{
    config::{HostConfiguration, Storage},
    constants::{self, internal_params::ENABLE_UKI_SUPPORT, ROOT_MOUNT_POINT_PATH},
    error::{ReportError, ServicingError, TridentError, TridentResultExt},
    status::{AbVolumeSelection, ServicingType},
    storage_graph::graph::StorageGraph,
    BlockDeviceId,
};

use crate::{
    engine::{boot::BootSubsystem, storage::StorageSubsystem},
    subsystems::{
        hooks::HooksSubsystem,
        initrd::InitrdSubsystem,
        management::ManagementSubsystem,
        network::NetworkSubsystem,
        osconfig::{MosConfigSubsystem, OsConfigSubsystem},
        selinux::SelinuxSubsystem,
    },
    TRIDENT_BACKGROUND_LOG_PATH, TRIDENT_METRICS_FILE_PATH,
};

// Engine functionality
pub mod bootentries;
mod clean_install;
mod context;
mod kexec;
mod newroot;
mod osimage;
pub mod provisioning_network;
pub mod rollback;
mod update;

// Trident Subsystems
pub mod boot;
pub mod storage;

// Helper modules
mod etc_overlay;

pub(crate) use clean_install::{clean_install, finalize_clean_install};
pub(crate) use context::EngineContext;
pub use newroot::NewrootMount;
pub(crate) use update::{finalize_update, update};

pub(crate) trait Subsystem: Send {
    fn name(&self) -> &'static str;

    fn writable_etc_overlay(&self) -> bool {
        true
    }

    // TODO: Implement dependencies
    // fn dependencies(&self) -> &'static [&'static str];

    /// Select the servicing type based on the Host Status and Host Configuration.
    fn select_servicing_type(
        &self,
        _ctx: &EngineContext,
    ) -> Result<Option<ServicingType>, TridentError> {
        Ok(None)
    }

    /// Validate the Host Configuration.
    fn validate_host_config(
        &self,
        _ctx: &EngineContext,
        _host_config: &HostConfiguration,
    ) -> Result<(), TridentError> {
        Ok(())
    }

    /// Perform non-destructive preparations for an update.
    fn prepare(&mut self, _ctx: &EngineContext) -> Result<(), TridentError> {
        Ok(())
    }

    /// Initialize state on the Runtime OS from the Provisioning OS, or migrate state from
    /// A-partition to B-partition (or vice versa).
    ///
    /// This method is called before the chroot is entered, and is used to perform any
    /// provisioning operations that need to be done before the chroot is entered.
    fn provision(&mut self, _ctx: &EngineContext, _mount_path: &Path) -> Result<(), TridentError> {
        Ok(())
    }

    /// Configure the system as specified by the Host Configuration, and update the Host Status
    /// accordingly.
    fn configure(&mut self, _ctx: &EngineContext, _exec_root: &Path) -> Result<(), TridentError> {
        Ok(())
    }
}

lazy_static::lazy_static! {
    static ref SUBSYSTEMS: Mutex<Vec<Box<dyn Subsystem>>> = Mutex::new(vec![
        Box::<MosConfigSubsystem>::default(),
        Box::<StorageSubsystem>::default(),
        Box::<BootSubsystem>::default(),
        Box::<NetworkSubsystem>::default(),
        Box::<OsConfigSubsystem>::default(),
        Box::<ManagementSubsystem>::default(),
        Box::<HooksSubsystem>::default(),
        Box::<InitrdSubsystem>::default(),
        Box::<SelinuxSubsystem>::default(),
    ]);
}

/// Persists the Trident background log and metrics files to the updated runtime
/// OS, by copying the files at TRIDENT_BACKGROUND_LOG_PATH and
/// TRIDENT_METRICS_FILE_PATH to the directory adjacent to the datastore. On
/// failure, only prints out an error message.
///
/// In case of clean install, the files are persisted to the datastore path in
/// the new root, so newroot_path is provided.
fn persist_background_log_and_metrics(
    datastore_path: &Path,
    newroot_path: Option<&Path>,
    servicing_type: ServicingType,
) {
    // Generate the new log filename based on the servicing type and the current timestamp
    let new_background_log_filename = format!(
        "trident-{:?}-{}.log",
        servicing_type,
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );

    // Generate the new metrics filename based on the servicing type and the current timestamp
    let new_metrics_filename = format!(
        "trident-metrics-{:?}-{}.jsonl",
        servicing_type,
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );

    // Fetch the directory path from the full datastore path
    let Some(datastore_dir) = datastore_path.parent() else {
        warn!(
            "Failed to get parent directory for datastore path '{}'",
            datastore_path.display()
        );
        return;
    };

    // Create the full path for the new background log file
    let new_background_log_path: PathBuf = if let Some(new_root) = newroot_path {
        join_relative(new_root, datastore_dir).join(new_background_log_filename)
    } else {
        datastore_dir.join(new_background_log_filename)
    };

    debug!(
        "Persisting Trident background log from '{}' to '{}' ",
        TRIDENT_BACKGROUND_LOG_PATH,
        new_background_log_path.display()
    );

    // Create the full path for the new metrics file
    let new_metrics_path: PathBuf = if let Some(new_root) = newroot_path {
        join_relative(new_root, datastore_dir).join(new_metrics_filename)
    } else {
        datastore_dir.join(new_metrics_filename)
    };

    debug!(
        "Persisting Trident metrics from '{}' to '{}' ",
        TRIDENT_METRICS_FILE_PATH,
        new_metrics_path.display()
    );

    // Copy the background log file to the new location
    if let Err(log_error) = fs::copy(TRIDENT_BACKGROUND_LOG_PATH, &new_background_log_path) {
        warn!(
            "Failed to persist Trident background log from '{}' to '{}': {}",
            TRIDENT_BACKGROUND_LOG_PATH,
            new_background_log_path.display(),
            log_error
        );
    } else {
        debug!(
            "Successfully persisted Trident background log from '{}' to '{}'",
            TRIDENT_BACKGROUND_LOG_PATH,
            new_background_log_path.display()
        );
    }

    // Copy the metrics file to the new location
    if let Err(e) = fs::copy(TRIDENT_METRICS_FILE_PATH, &new_metrics_path) {
        warn!(
            "Failed to persist Trident metrics file from '{}' to '{}': {}",
            TRIDENT_METRICS_FILE_PATH,
            new_metrics_path.display(),
            e
        );
    } else {
        debug!(
            "Successfully persisted Trident metrics from '{}' to '{}' ",
            TRIDENT_METRICS_FILE_PATH,
            new_metrics_path.display()
        );
    }
}

/// Using the / mount point, figure out what should be used as a root block device.
pub(super) fn get_root_block_device_path(ctx: &EngineContext) -> Option<PathBuf> {
    ctx.spec
        .storage
        .path_to_mount_point(Path::new(ROOT_MOUNT_POINT_PATH))
        .and_then(|m| get_block_device_path(ctx, &m.target_id))
}

/// Returns the path of the block device with id `block_device_id`.
///
/// If the volume is part of an A/B Volume Pair this returns the update volume (i.e. the one that
/// isn't active).
pub(super) fn get_block_device_path(
    ctx: &EngineContext,
    block_device_id: &BlockDeviceId,
) -> Option<PathBuf> {
    if let Some(partition_path) = ctx.block_device_paths.get(block_device_id) {
        return Some(partition_path.clone());
    }

    if let Some(raid) = ctx
        .spec
        .storage
        .raid
        .software
        .iter()
        .find(|r| &r.id == block_device_id)
    {
        return Some(raid.device_path());
    }

    if let Some(encryption) = &ctx.spec.storage.encryption {
        if let Some(encrypted) = encryption.volumes.iter().find(|e| &e.id == block_device_id) {
            return Some(encrypted.device_path());
        }
    }

    if let Some(verity) = ctx
        .spec
        .storage
        .internal_verity
        .iter()
        .find(|v| &v.id == block_device_id)
    {
        return Some(verity.device_path());
    }

    get_ab_volume_block_device_id(ctx, block_device_id)
        .and_then(|child_block_device_id| get_block_device_path(ctx, child_block_device_id))
}

/// Returns the block device id for the update volume from the given A/B Volume Pair.
fn get_ab_volume_block_device_id<'a>(
    ctx: &'a EngineContext,
    block_device_id: &BlockDeviceId,
) -> Option<&'a BlockDeviceId> {
    if let Some(ab_update) = &ctx.spec.storage.ab_update {
        let ab_volume = ab_update
            .volume_pairs
            .iter()
            .find(|v| &v.id == block_device_id);
        if let Some(v) = ab_volume {
            let selection = ctx.get_ab_update_volume();
            // Return the appropriate BlockDeviceId based on the selection
            return selection.map(|sel| match sel {
                AbVolumeSelection::VolumeA => &v.volume_a_id,
                AbVolumeSelection::VolumeB => &v.volume_b_id,
            });
        };
    }
    None
}

#[tracing::instrument(skip_all)]
fn validate_host_config(
    subsystems: &[Box<dyn Subsystem>],
    ctx: &EngineContext,
    host_config: &HostConfiguration,
) -> Result<(), TridentError> {
    info!("Starting step 'Validate'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Validate' for subsystem '{}'",
            subsystem.name()
        );
        subsystem
            .validate_host_config(ctx, host_config)
            .message(format!(
                "Step 'Validate' failed for subsystem '{}'",
                subsystem.name()
            ))?;
    }
    debug!("Finished step 'Validate'");
    Ok(())
}

fn prepare(subsystems: &mut [Box<dyn Subsystem>], ctx: &EngineContext) -> Result<(), TridentError> {
    info!("Starting step 'Prepare'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Prepare' for subsystem '{}'",
            subsystem.name()
        );
        subsystem.prepare(ctx).message(format!(
            "Step 'Prepare' failed for subsystem '{}'",
            subsystem.name()
        ))?;
    }
    debug!("Finished step 'Prepare'");
    Ok(())
}

fn provision(
    subsystems: &mut [Box<dyn Subsystem>],
    ctx: &EngineContext,
    new_root_path: &Path,
) -> Result<(), TridentError> {
    // If verity is present, it means that we are currently doing root
    // verity. For now, we can assume that /etc is readonly, so we setup
    // a writable overlay for it.
    let use_overlay = !ctx.spec.storage.internal_verity.is_empty();

    info!("Starting step 'Provision'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Provision' for subsystem '{}'",
            subsystem.name()
        );
        let _etc_overlay_mount = if use_overlay {
            Some(etc_overlay::create(
                Path::new(new_root_path),
                subsystem.writable_etc_overlay(),
            )?)
        } else {
            None
        };
        subsystem.provision(ctx, new_root_path).message(format!(
            "Step 'Provision' failed for subsystem '{}'",
            subsystem.name()
        ))?;
    }
    debug!("Finished step 'Provision'");
    Ok(())
}

fn configure(
    subsystems: &mut [Box<dyn Subsystem>],
    ctx: &EngineContext,
    exec_root: &Path,
) -> Result<(), TridentError> {
    // UKI support currently assumes root verity without a writable overlay. Many module's configure
    // methods would fail in this case, so we skip all of them.
    //
    // TODO: More granular logic for which configure operations to skip. At a minimum,
    // post-configuration scripts should still run. Additionally, errors should be generated for any
    // customizations requested in the Host Configuration that would be skipped.
    if ctx.spec.internal_params.get_flag(ENABLE_UKI_SUPPORT) {
        return Ok(());
    }

    // If verity is present, it means that we are currently doing root
    // verity. For now, we can assume that /etc is readonly, so we setup
    // a writable overlay for it.
    let use_overlay = (ctx.servicing_type == ServicingType::CleanInstall
        || ctx.servicing_type == ServicingType::AbUpdate)
        && !ctx.spec.storage.internal_verity.is_empty();

    info!("Starting step 'Configure'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Configure' for subsystem '{}'",
            subsystem.name()
        );
        // unmount on drop
        let _etc_overlay_mount = if use_overlay {
            Some(etc_overlay::create(
                Path::new("/"),
                subsystem.writable_etc_overlay(),
            )?)
        } else {
            None
        };
        subsystem.configure(ctx, exec_root).message(format!(
            "Step 'Configure' failed for subsystem '{}'",
            subsystem.name()
        ))?;
    }
    debug!("Finished step 'Configure'");

    Ok(())
}

pub fn reboot() -> Result<(), TridentError> {
    // Sync all writes to the filesystem.
    info!("Syncing filesystem");
    nix::unistd::sync();

    // This trace event will be used with the trident_start event to track the
    // total time taken for the reboot
    tracing::info!(metric_name = "trident_system_reboot");
    info!("Rebooting system");
    Dependency::Systemctl
        .cmd()
        .env("SYSTEMD_IGNORE_CHROOT", "true")
        .arg("reboot")
        .run_and_check()
        .structured(ServicingError::Reboot)?;

    thread::sleep(Duration::from_secs(600));

    error!("Waited for reboot for 10 minutes, but nothing happened, aborting");
    Err(TridentError::new(ServicingError::RebootTimeout))
}

/// Builds the storage graph for the given storage configuration. Since graph v2 is still in its
/// experimental phase, any errors that occur during the graph building process are logged, and an
/// empty/default graph is returned, without returing an error.
pub(super) fn build_storage_graph(storage: &Storage) -> StorageGraph {
    debug!("EXPERIMENTAL GRAPHv2: Using graph2 for storage graph building.");
    match storage.build_graph2() {
        Ok(graph) => {
            debug!("EXPERIMENTAL GRAPHv2: Storage graph built successfully.");
            graph
        }
        Err(err) => {
            error!(
                "EXPERIMENTAL GRAPHv2: Failed to build storage graph: {}",
                err
            );
            Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use maplit::btreemap;

    use trident_api::config::{
        self, AbUpdate, AbVolumePair, Disk, FileSystemType, Partition, PartitionType,
    };

    #[test]
    fn test_get_root_block_device_path() {
        let ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![Disk {
                        id: "foo".to_owned(),
                        device: PathBuf::from("/dev/sda"),
                        partitions: vec![
                            Partition {
                                id: "boot".to_owned(),
                                size: 2.into(),
                                partition_type: PartitionType::Esp,
                            },
                            Partition {
                                id: "root".to_owned(),
                                size: 7.into(),
                                partition_type: PartitionType::Root,
                            },
                        ],
                        ..Default::default()
                    }],
                    internal_mount_points: vec![
                        config::InternalMountPoint {
                            target_id: "boot".to_owned(),
                            filesystem: FileSystemType::Vfat,
                            options: vec![],
                            path: PathBuf::from("/boot"),
                        },
                        config::InternalMountPoint {
                            target_id: "root".to_owned(),
                            filesystem: FileSystemType::Ext4,
                            options: vec![],
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
                "foo".to_owned() => PathBuf::from("/dev/sda"),
                "boot".to_owned() => PathBuf::from("/dev/sda1"),
                "root".to_owned() => PathBuf::from("/dev/sda2"),
            },
            ..Default::default()
        };

        assert_eq!(
            get_root_block_device_path(&ctx),
            Some(PathBuf::from("/dev/sda2"))
        );
    }

    /// Validates that the `get_block_device_for_update` function works as expected for
    /// disks, partitions and ab volumes.
    #[test]
    fn test_get_block_device_for_update() {
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![
                        Disk {
                            id: "os".to_owned(),
                            device: PathBuf::from("/dev/disk/by-bus/foobar"),
                            partitions: vec![
                                Partition {
                                    id: "efi".to_owned(),
                                    size: 100.into(),
                                    partition_type: PartitionType::Esp,
                                },
                                Partition {
                                    id: "root".to_owned(),
                                    size: 900.into(),
                                    partition_type: PartitionType::Root,
                                },
                                Partition {
                                    id: "rootb".to_owned(),
                                    size: 9000.into(),
                                    partition_type: PartitionType::Root,
                                },
                            ],
                            ..Default::default()
                        },
                        Disk {
                            id: "data".to_owned(),
                            device: PathBuf::from("/dev/disk/by-bus/foobar"),
                            partitions: vec![],
                            ..Default::default()
                        },
                    ],
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "osab".to_string(),
                            volume_a_id: "root".to_string(),
                            volume_b_id: "rootb".to_string(),
                        }],
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
                "os".to_owned() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "rootb".to_owned() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "data".to_owned() => PathBuf::from("/dev/disk/by-bus/foobar"),
            },
            servicing_type: ServicingType::NoActiveServicing,
            ..Default::default()
        };

        assert_eq!(
            get_block_device_path(&ctx, &"os".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-bus/foobar")
        );
        assert_eq!(
            get_block_device_path(&ctx, &"efi".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp1")
        );
        assert_eq!(
            get_block_device_path(&ctx, &"root".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp2")
        );
        assert_eq!(get_block_device_path(&ctx, &"foobar".to_owned()), None);
        assert_eq!(
            get_block_device_path(&ctx, &"data".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-bus/foobar")
        );

        // Now, set ab_active_volume to VolumeA.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(get_block_device_path(&ctx, &"osab".to_owned()), None);
        assert_eq!(
            get_ab_volume_block_device_id(&ctx, &"osab".to_owned()),
            None
        );

        // Now, set servicing type to AbUpdate.
        ctx.servicing_type = ServicingType::AbUpdate;
        assert_eq!(
            get_block_device_path(&ctx, &"osab".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp3")
        );
        assert_eq!(
            get_ab_volume_block_device_id(&ctx, &"osab".to_owned()),
            Some(&"rootb".to_owned())
        );

        // When active volume is VolumeB, should return VolumeA
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_block_device_path(&ctx, &"osab".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp2")
        );
        assert_eq!(
            get_ab_volume_block_device_id(&ctx, &"osab".to_owned()),
            Some(&"root".to_owned())
        );

        // If target block device id does not exist, should return None.
        assert_eq!(
            get_ab_volume_block_device_id(&ctx, &"non-existent".to_owned()),
            None
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use tempfile::tempdir;

    /// Helper function to check if the persisted background log and metrics
    /// file, i.e. 'trident-<servicingType>-<timeStamp>.log' and
    /// `trident-metrics-<servicingType>-<timeStamp>.jsonl`, exists in the log
    /// directory.
    fn persisted_log_and_metrics_exists(dir: &Path, servicing_type: ServicingType) -> bool {
        let files = fs::read_dir(dir).unwrap();
        let log_prefix = format!("trident-{:?}-", servicing_type);
        let metrics_prefix = format!("trident-metrics-{:?}-", servicing_type);
        let (mut log_found, mut metrics_found) = (false, false);
        for entry in files {
            let entry = entry.unwrap();
            let file_name = entry.file_name().into_string().unwrap();

            // Check if any file starts with the correct prefix
            if file_name.starts_with(&log_prefix) {
                log_found = true;
            } else if file_name.starts_with(&metrics_prefix) {
                metrics_found = true;
            }
            if log_found && metrics_found {
                return true;
            }
        }
        false
    }

    #[functional_test]
    fn test_persist_background_log_and_metrics_success() {
        // Create a tempdir for mock datastore path
        let temp_dir_datastore = tempdir().unwrap();
        let datastore_dir = temp_dir_datastore.path();
        let datastore_path = datastore_dir.join("datastore");

        // Create a tempdir for mock new root path
        let temp_dir_newroot = tempdir().unwrap();
        let newroot_path = temp_dir_newroot.path();

        // Create mock datastore directory and log file
        fs::create_dir_all(&datastore_path).unwrap();

        // Compose the log dir
        let log_dir = join_relative(newroot_path, datastore_dir);
        fs::create_dir_all(&log_dir).unwrap();

        // Persist the background log and metrics file
        let servicing_type = ServicingType::CleanInstall;
        persist_background_log_and_metrics(&datastore_path, Some(newroot_path), servicing_type);

        assert!(
            persisted_log_and_metrics_exists(&log_dir, servicing_type),
            "Trident background log and metrics should be persisted successfully."
        );
    }

    #[functional_test(feature = "helpers", negative = true)]
    fn test_persist_background_log_and_metrics_failure() {
        // Create a tempdir for mock datastore path
        let temp_dir_datastore = tempdir().unwrap();
        let datastore_dir = temp_dir_datastore.path();
        let datastore_path = datastore_dir.join("datastore");

        // Create mock datastore directory and log file
        fs::create_dir_all(&datastore_path).unwrap();

        // Create a temp copy of TRIDENT_BACKGROUND_LOG_PATH
        let temp_log_path = TRIDENT_BACKGROUND_LOG_PATH.to_owned() + ".temp";
        fs::copy(TRIDENT_BACKGROUND_LOG_PATH, &temp_log_path).unwrap();
        // Remove TRIDENT_BACKGROUND_LOG_PATH
        fs::remove_file(TRIDENT_BACKGROUND_LOG_PATH).unwrap();

        // Create a temp copy of TRIDENT_METRICS_FILE_PATH
        let temp_metrics_path = TRIDENT_METRICS_FILE_PATH.to_owned() + ".temp";
        fs::copy(TRIDENT_METRICS_FILE_PATH, &temp_metrics_path).unwrap();
        // Remove TRIDENT_METRICS_FILE_PATH
        fs::remove_file(TRIDENT_METRICS_FILE_PATH).unwrap();

        // Persist the background log and metrics file
        let servicing_type = ServicingType::AbUpdate;
        persist_background_log_and_metrics(&datastore_path, None, servicing_type);

        assert!(
            !persisted_log_and_metrics_exists(datastore_dir, servicing_type),
            "Trident background log and metrics should not be persisted."
        );

        // Re-create TRIDENT_BACKGROUND_LOG_PATH by copying from the temp file
        fs::copy(&temp_log_path, TRIDENT_BACKGROUND_LOG_PATH).unwrap();

        // Re-create TRIDENT_METRICS_FILE_PATH by copying from the temp file
        fs::copy(&temp_metrics_path, TRIDENT_METRICS_FILE_PATH).unwrap();
    }
}
