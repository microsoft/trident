use std::{fs, path::Path, sync::Mutex};

use anyhow::{bail, Context, Error};
use log::info;

use trident_api::{
    config::{HostConfiguration, OperationType},
    status::{HostStatus, ReconcileState, UpdateKind},
};

use crate::mount::{self, setup_root_chroot, unmount_target_volumes};
use crate::{datastore::DataStore, get_block_device};
use crate::{
    modules::{image::ImageModule, network::NetworkModule, storage::StorageModule},
    mount::UpdateTargetEnvironment,
};

pub mod image;
pub mod network;
pub mod storage;

pub trait Module: Send {
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
        Some(UpdateKind::HotPatch)
    }

    /// Migrate state from A-partition to B-partition (or vice versa).
    fn migrate(
        &mut self,
        _host_status: &mut HostStatus,
        _host_config: &HostConfiguration,
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
    pub static ref MODULES: Mutex<Vec<Box<dyn Module>>> = Mutex::new(vec![
        Box::<StorageModule>::default(),
        Box::<ImageModule>::default(),
        Box::<NetworkModule>::default(),
    ]);
}

pub(crate) fn provision(
    host_config: &HostConfiguration,
    allowed_operations: OperationType,
) -> Result<(), Error> {
    // This is a safety check so that nobody accidentally formats their dev machine.
    if !fs::read_to_string("/proc/cmdline")
        .context("Failed to read /proc/cmdline")?
        .contains("root=/dev/ram0")
    {
        bail!("Safety check failed! Requested clean install but not booted from ramdisk");
    }

    let mut modules = MODULES.lock().unwrap();
    let mut host_status = HostStatus {
        reconcile_state: ReconcileState::CleanInstall,
        ..Default::default()
    };

    for m in &mut *modules {
        m.refresh_host_status(&mut host_status).context(format!(
            "Module '{}' failed to refresh host status",
            m.name()
        ))?;
    }
    info!("Host status: {:#?}", host_status);

    for m in &*modules {
        m.validate_host_config(&host_status, host_config)
            .context(format!(
                "Module '{}' failed to validate host config",
                m.name()
            ))?;
    }
    info!("Host config validated");

    if allowed_operations == OperationType::RefreshOnly {
        info!("Pause requested, skipping reconcile");
        return Ok(());
    }

    StorageModule::create_partitions(&mut host_status, host_config)
        .context("Failed to create disk partitions")?;

    image::refresh_ab_volumes(&mut host_status, host_config);

    image::stream_images(&mut host_status, host_config).context("Failed to stream images")?;

    let chroot_env = setup_root_chroot(host_config, &host_status)
        .context("Failed to setup target root chroot")?;

    if chroot_env.is_some() {
        let mut state = DataStore::create(Path::new("/trident.sqlite"), host_status)?;

        // TODO: user creation should happen in modules (and not be hardcoded).
        mount::run_script(
            r#"sudo sh -c 'echo root:password | chpasswd'
            useradd -p $(openssl passwd -1 password) -s /bin/bash -d /home/mariner_user/ -m -G sudo mariner_user"#
        ).context("Failed to apply system config")?;

        for m in &mut *modules {
            state.with_host_status(|s| {
                m.reconcile(s, host_config)
                    .context(format!("Module '{}' failed during reconcile", m.name()))
            })?;
        }

        // TODO: Call post-update workload hook.
        drop(state);
    }

    if allowed_operations == OperationType::Update {
        info!("Only Update requested, skipping transition");
        if let Some(chroot_env) = chroot_env {
            chroot_env.chroot.exit().context("Failed to exit chroot")?;
            unmount_target_volumes(chroot_env.mount_path.as_path())
                .context("Failed to unmount target volumes")?;
        }
        return Ok(());
    }

    transition(chroot_env)?;

    Ok(())
}

pub(crate) fn update(
    host_config: &HostConfiguration,
    allowed_operations: OperationType,
    mut state: DataStore,
) -> Result<(), Error> {
    let mut modules = MODULES.lock().unwrap();

    for m in &mut *modules {
        state.with_host_status(|s| {
            m.refresh_host_status(s).context(format!(
                "Module '{}' failed to refresh host status",
                m.name()
            ))
        })?;
    }

    info!("Host status: {:#?}", state.host_status());

    for m in &*modules {
        m.validate_host_config(state.host_status(), host_config)
            .context(format!(
                "Module '{}' failed to validate host config",
                m.name()
            ))?;
    }
    info!("Host config validated");

    if allowed_operations == OperationType::RefreshOnly {
        info!("Only status refresh requested, skipping reconcile");
        return Ok(());
    }

    let update_kind = modules
        .iter()
        .filter_map(|m| m.select_update_kind(state.host_status(), host_config))
        .max();
    state.with_host_status(|s| {
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

    // TODO: Call pre-update workload hook.

    let mut chroot_env = None;
    let mut should_reconcile = true;

    if let Some(UpdateKind::AbUpdate) = update_kind {
        // TODO: Download update
        // TODO: Write update

        for m in &mut *modules {
            state.with_host_status(|s| {
                m.migrate(s, host_config)
                    .context(format!("Module '{}' failed during pause", m.name()))
            })?;
        }

        chroot_env = setup_root_chroot(host_config, state.host_status())
            .context("Failed to setup root chroot")?;
        should_reconcile = chroot_env.is_some();
    }

    if should_reconcile {
        for m in &mut *modules {
            state.with_host_status(|s| {
                m.reconcile(s, host_config)
                    .context(format!("Module '{}' failed during reconcile", m.name()))
            })?;
        }
    }

    // TODO: Call post-update workload hook.

    match update_kind {
        Some(UpdateKind::UpdateAndReboot) | Some(UpdateKind::AbUpdate) => {
            drop(state);

            if allowed_operations == OperationType::Update {
                info!("Only update requested, skipping transition");
                if let Some(chroot_env) = chroot_env {
                    chroot_env.chroot.exit().context("Failed to exit chroot")?;
                    unmount_target_volumes(chroot_env.mount_path.as_path())
                        .context("Failed to unmount target volumes")?;
                }
                return Ok(());
            }
            transition(chroot_env)?;
            Ok(())
        }
        Some(UpdateKind::NormalUpdate) | Some(UpdateKind::HotPatch) => {
            state.with_host_status(|s| {
                s.reconcile_state = ReconcileState::Ready;
                Ok(())
            })?;
            info!("Update complete");
            Ok(())
        }
        None | Some(UpdateKind::Incompatible) => {
            unreachable!()
        }
    }
}

fn transition(
    update_target_environment_option: Option<UpdateTargetEnvironment>,
) -> Result<(), Error> {
    match update_target_environment_option {
        Some(update_target_environment) => {
            update_target_environment
                .chroot
                .exit()
                .context("Failed to exit chroot")?;

            info!("Performing soft reboot");
            image::kexec(
                &update_target_environment.mount_path,
                format!(
                    "console=tty1 console=ttyS0 root={}",
                    update_target_environment
                        .root_block_device
                        .path
                        .to_str()
                        .ok_or(anyhow::anyhow!(
                            "Failed to convert root device path {:?} to string",
                            update_target_environment.root_block_device.path
                        ))?
                )
                .as_str(),
            )
            .context("Failed to perform kexec")?;

            unreachable!("kexec should never return")
        }
        None => {
            info!("No root block device found, performing reboot");
            image::reboot().context("Failed to perform reboot")?;

            unreachable!("reboot should never return");
        }
    }
}

/// Using the / mount point, figure out what should be used as a root block device.
pub fn get_root_block_device(
    host_config: &HostConfiguration,
    host_status: &HostStatus,
) -> Result<Option<trident_api::status::BlockDeviceInfo>, Error> {
    host_config
        .storage
        .mount_points
        .iter()
        .find(|mp| mp.path == Path::new("/"))
        .map(|mp| get_block_device(host_status, &mp.target_id))
        .transpose()
        .context("Failed to find root block device")
}
