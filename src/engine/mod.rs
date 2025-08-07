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
    config::Storage,
    constants,
    error::{InternalError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{ServicingState, ServicingType},
    storage_graph::graph::StorageGraph,
};

use crate::{
    engine::boot::BootSubsystem,
    subsystems::{
        esp::EspSubsystem,
        hooks::HooksSubsystem,
        initrd::InitrdSubsystem,
        management::ManagementSubsystem,
        network::NetworkSubsystem,
        osconfig::{MosConfigSubsystem, OsConfigSubsystem},
        selinux::SelinuxSubsystem,
        storage::StorageSubsystem,
    },
    TRIDENT_BACKGROUND_LOG_PATH, TRIDENT_METRICS_FILE_PATH,
};

// Engine functionality
pub mod bootentries;
mod clean_install;
mod context;
mod kexec;
mod newroot;
pub mod provisioning_network;
pub mod rollback;
mod update;

// Trident Subsystems
pub mod boot;
pub mod storage;

// Helper modules
mod etc_overlay;
pub(crate) mod install_index;

pub(crate) use clean_install::{clean_install, finalize_clean_install};
pub(crate) use context::{filesystem, EngineContext};
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

    /// Validate that the Host Configuration in `ctx.spec` can be applied on the system.
    ///
    /// Implementations should consider the previous Host Configuration in `ctx.spec_old` and the
    /// servicing type in `ctx.servicing_type`.
    fn validate_host_config(&self, _ctx: &EngineContext) -> Result<(), TridentError> {
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
    fn configure(&mut self, _ctx: &EngineContext) -> Result<(), TridentError> {
        Ok(())
    }
}

lazy_static::lazy_static! {
    static ref SUBSYSTEMS: Mutex<Vec<Box<dyn Subsystem>>> = Mutex::new(vec![
        Box::<MosConfigSubsystem>::default(),
        Box::<EspSubsystem>::default(),
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

/// Persists the Trident background log and metrics files to the updated runtime OS, by copying the
/// TRIDENT_BACKGROUND_LOG_PATH and TRIDENT_METRICS_FILE_PATH to the directory adjacent to the
/// datastore. On failure, only prints out an error message.
///
/// Each copy is named following the format "trident-<servicing_state>-<timestamp>.log", where
/// <servicing_state> is the state in Host Status set when the logs were copied. So, e.g., the logs
/// for the staging of an A/B update would be named `trident-ab-update-staged-<timestamp>.log`.
///
/// In case of clean install, the files are persisted to the datastore path in the new root, so
/// newroot_path is provided.
fn persist_background_log_and_metrics(
    datastore_path: &Path,
    newroot_path: Option<&Path>,
    servicing_state: ServicingState,
) {
    // Generate the new log filename
    let new_background_log_filename = format!(
        "trident-{:?}-{}.log",
        servicing_state,
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );

    // Generate the new metrics filename
    let new_metrics_filename = format!(
        "trident-metrics-{:?}-{}.jsonl",
        servicing_state,
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

#[tracing::instrument(skip_all)]
fn validate_host_config(
    subsystems: &[Box<dyn Subsystem>],
    ctx: &EngineContext,
) -> Result<(), TridentError> {
    info!("Starting step 'Validate'");
    for subsystem in subsystems {
        debug!(
            "Starting step 'Validate' for subsystem '{}'",
            subsystem.name()
        );
        subsystem.validate_host_config(ctx).message(format!(
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
    // In root-verity, we can assume that /etc is readonly, so we setup
    // a writable overlay for it.
    let use_overlay = ctx.storage_graph.root_fs_is_verity();

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
) -> Result<(), TridentError> {
    // Root verity means /etc is readonly, so in the non-UKI configuration we setup a writable
    // overlay for it.
    let use_overlay = (ctx.servicing_type == ServicingType::CleanInstall
        || ctx.servicing_type == ServicingType::AbUpdate)
        && ctx.storage_graph.root_fs_is_verity()
        && !ctx.is_uki()?;

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
        subsystem.configure(ctx).message(format!(
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
/// empty/default graph is returned, without returning an error.
pub(super) fn build_storage_graph(storage: &Storage) -> Result<StorageGraph, TridentError> {
    debug!("Rebuilding storage graph for engine context");

    // Temporarily override the log level to only show warnings and above to
    // avoid producing the graph building logs again. We can safely do this
    // because at this point we are only running in a single thread, but this
    // should be re-visited if we ever go more async/multithreaded.
    let old_level = log::max_level();
    log::set_max_level(log::LevelFilter::Warn);

    // Build the storage graph
    let graph_res = storage.build_graph();

    // Reset the log level to the original level
    log::set_max_level(old_level);

    graph_res.map_err(|e| TridentError::new(InternalError::from(e)))
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use pytest_gen::functional_test;

    use tempfile::tempdir;

    /// Helper function to check if the persisted background log and metrics file exist in the log
    /// directory.
    fn persisted_log_and_metrics_exist(dir: &Path, servicing_state: ServicingState) -> bool {
        let files = fs::read_dir(dir).unwrap();

        let log_prefix = format!("trident-{servicing_state:?}-");
        let metrics_prefix = format!("trident-metrics-{servicing_state:?}-");

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

        // ENSURE THE LOG AND METRICS FILES EXIST
        fs::write(
            TRIDENT_BACKGROUND_LOG_PATH,
            "{\"message\":\"This is a mock background log file.\"}",
        )
        .unwrap();
        fs::write(
            TRIDENT_METRICS_FILE_PATH,
            "{\"metric\":\"This is a mock metrics file.\"}",
        )
        .unwrap();

        // Compose the log dir
        let log_dir = join_relative(newroot_path, datastore_dir);
        fs::create_dir_all(&log_dir).unwrap();

        // Persist the background log and metrics file
        let servicing_state = ServicingState::AbUpdateFinalized;
        persist_background_log_and_metrics(&datastore_path, Some(newroot_path), servicing_state);

        assert!(
            persisted_log_and_metrics_exist(&log_dir, servicing_state),
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

        // ENSURE THE LOG AND METRICS FILES DO NOT EXIST
        if Path::new(TRIDENT_BACKGROUND_LOG_PATH).exists() {
            fs::remove_file(TRIDENT_BACKGROUND_LOG_PATH).unwrap();
        }
        if Path::new(TRIDENT_METRICS_FILE_PATH).exists() {
            fs::remove_file(TRIDENT_METRICS_FILE_PATH).unwrap();
        }

        // Persist the background log and metrics file
        let servicing_state = ServicingState::AbUpdateFinalized;
        persist_background_log_and_metrics(&datastore_path, None, servicing_state);

        assert!(
            !persisted_log_and_metrics_exist(datastore_dir, servicing_state),
            "Trident background log and metrics should not be persisted."
        );
    }
}
