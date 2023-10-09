use std::{fs, path::Path, sync::Mutex};

use anyhow::{bail, Context, Error};
use log::info;

use trident_api::{
    config::{HostConfiguration, Operations, TridentConfiguration},
    status::{HostStatus, ReconcileState, UpdateKind},
};

use crate::modules::{
    image::ImageModule, management::ManagementModule, network::NetworkModule,
    osconfig::OsConfigModule, scripts::PostInstallScriptsModule, storage::StorageModule,
};
use crate::{datastore::DataStore, mount, TRIDENT_DATASTORE_PATH};

pub mod image;
pub mod management;
pub mod network;
pub mod osconfig;
pub mod scripts;
pub mod storage;

trait Module: Send {
    fn name(&self) -> &'static str;

    // // TODO: Implement dependencies
    // fn dependencies(&self) -> &'static [&'static str];

    /// Refresh the host status.
    fn refresh_host_status(&mut self, host_status: &mut HostStatus) -> Result<(), Error>;

    /// Validate the host config.
    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Select the update kind based on the host status and host config.
    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        None
    }

    /// Initialize state on the Runtime OS from the Provisioning OS, or migrate state from
    /// A-partition to B-partition (or vice versa).
    ///
    /// This method is called before the chroot is entered, and is used to perform any
    /// provisioning operations that need to be done before the chroot is entered.
    fn migrate(
        &mut self,
        _host_status: &mut HostStatus,
        _host_config: &HostConfiguration,
        _mount_path: &Path,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Reconcile the state of the system with the host config, and update the host status
    /// accordingly.
    fn reconcile(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error>;
}

lazy_static::lazy_static! {
    static ref MODULES: Mutex<Vec<Box<dyn Module>>> = Mutex::new(vec![
        Box::<StorageModule>::default(),
        Box::<ImageModule>::default(),
        Box::<NetworkModule>::default(),
        Box::<OsConfigModule>::default(),
        Box::<ManagementModule>::default(),
        Box::<PostInstallScriptsModule>::default(),
    ]);
}

pub(super) fn provision(
    host_config: &HostConfiguration,
    trident: &TridentConfiguration,
) -> Result<(), Error> {
    // This is a safety check so that nobody accidentally formats their dev machine.
    if !fs::read_to_string("/proc/cmdline")
        .context("Failed to read /proc/cmdline")?
        .contains("root=/dev/ram0")
    {
        bail!("Safety check failed! Requested clean install but not booted from ramdisk");
    }

    let mut modules = MODULES.lock().unwrap();
    let mut state = DataStore::new();
    state.with_host_status(|s| s.reconcile_state = ReconcileState::CleanInstall)?;

    refresh_host_status(&mut modules, &mut state)?;
    validate_host_config(&modules, &state, host_config)?;

    if !trident.allowed_operations.contains(Operations::Update) {
        info!("Update not requested, skipping reconcile");
        return Ok(());
    }

    // TODO: We should have a way to indicate which modules setup the root mount point, and which
    // depend on it being in place. Right now we just depend on the "storage" and "image" modules
    // being the first ones to run.
    let mount_path = Path::new("/partitionMount");
    migrate(&mut modules, &mut state, host_config, mount_path)?;

    let chroot = mount::enter_chroot(mount_path)?;
    state.persist(
        host_config
            .management
            .datastore_path
            .as_deref()
            .unwrap_or(Path::new(TRIDENT_DATASTORE_PATH)),
    )?;
    reconcile(&mut modules, &mut state, host_config)?;

    let root_device_path = state
        .host_status()
        .imaging
        .root_device_path
        .clone()
        .context("Failed to get root device path")?;

    drop(state);
    chroot.exit().context("Failed to exit chroot")?;

    if !trident.allowed_operations.contains(Operations::Transition) {
        info!("Transition not requested, skipping transition");
        mount::unmount_target_volumes(mount_path).context("Failed to unmount target volumes")?;
        return Ok(());
    }

    transition(mount_path, &root_device_path)?;

    Ok(())
}

pub(super) fn update(
    host_config: &HostConfiguration,
    trident: &TridentConfiguration,
    mut state: DataStore,
) -> Result<(), Error> {
    let mut modules = MODULES.lock().unwrap();

    refresh_host_status(&mut modules, &mut state)?;
    validate_host_config(&modules, &state, host_config)?;

    if !trident.allowed_operations.contains(Operations::Update) {
        info!("Update not requested, skipping reconcile");
        return Ok(());
    }

    let update_kind = modules
        .iter()
        .filter_map(|m| m.select_update_kind(state.host_status(), host_config))
        .max();
    state.try_with_host_status(|s| {
        s.reconcile_state = match update_kind {
            Some(k) => ReconcileState::UpdateInProgress(k),
            None => ReconcileState::Ready,
        };
        Ok(())
    })?;

    match update_kind {
        None => {
            info!("No updates required");
            return Ok(());
        }
        Some(UpdateKind::HotPatch) => info!("Performing hot patch update"),
        Some(UpdateKind::NormalUpdate) => info!("Performing normal update"),
        Some(UpdateKind::UpdateAndReboot) => info!("Performing update and reboot"),
        Some(UpdateKind::AbUpdate) => info!("Performing A/B update"),
        Some(UpdateKind::Incompatible) => {
            bail!("Requested host config is not compatible with current install")
        }
    }

    let mut chroot = None;
    let mount_path = Path::new("/partitionMount");

    if let Some(UpdateKind::AbUpdate) = update_kind {
        migrate(&mut modules, &mut state, host_config, mount_path)?;
        chroot = Some(mount::enter_chroot(mount_path)?);
    }

    reconcile(&mut modules, &mut state, host_config)?;

    if let Some(chroot) = chroot {
        chroot.exit().context("Failed to exit chroot")?;
    }

    match update_kind {
        Some(UpdateKind::UpdateAndReboot) | Some(UpdateKind::AbUpdate) => {
            let root_block_device_path = state
                .host_status()
                .imaging
                .root_device_path
                .clone()
                .context("Failed to get root device path")?;

            drop(state);

            if !trident.allowed_operations.contains(Operations::Transition) {
                info!("Transition not requested, skipping transition");
                mount::unmount_target_volumes(mount_path)
                    .context("Failed to unmount target volumes")?;
                return Ok(());
            }

            transition(mount_path, &root_block_device_path)?;
            Ok(())
        }
        Some(UpdateKind::NormalUpdate) | Some(UpdateKind::HotPatch) => {
            state.with_host_status(|s| s.reconcile_state = ReconcileState::Ready)?;
            info!("Update complete");
            Ok(())
        }
        None | Some(UpdateKind::Incompatible) => {
            unreachable!()
        }
    }
}

fn refresh_host_status(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
) -> Result<(), Error> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.refresh_host_status(s).context(format!(
                "Module '{}' failed to refresh host status",
                m.name()
            ))
        })?;
    }
    Ok(())
}

fn validate_host_config(
    modules: &[Box<dyn Module>],
    state: &DataStore,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    for m in modules {
        m.validate_host_config(state.host_status(), host_config)
            .context(format!(
                "Module '{}' failed to validate host config",
                m.name()
            ))?;
    }
    info!("Host config validated");
    Ok(())
}

fn migrate(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
    mount_point: &Path,
) -> Result<(), Error> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.migrate(s, host_config, mount_point)
                .context(format!("Module '{}' failed to migrate", m.name()))
        })?;
    }
    Ok(())
}

fn reconcile(
    modules: &mut [Box<dyn Module>],
    state: &mut DataStore,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    for m in modules {
        state.try_with_host_status(|s| {
            m.reconcile(s, host_config)
                .context(format!("Module '{}' failed during reconcile", m.name()))
        })?;
    }
    Ok(())
}

fn transition(mount_path: &Path, root_block_device_path: &Path) -> Result<(), Error> {
    let root_block_device_path = root_block_device_path.to_str().context(format!(
        "Failed to convert root device path {:?} to string",
        root_block_device_path
    ))?;

    info!("Performing soft reboot");
    image::kexec(
        mount_path,
        &format!("console=tty1 console=ttyS0 root={root_block_device_path}"),
    )
    .context("Failed to perform kexec")
}
