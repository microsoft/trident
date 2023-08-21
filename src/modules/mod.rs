use std::{fs, path::Path, sync::Mutex};

use anyhow::{bail, Context, Error};
use log::info;

use crate::{
    config::HostConfig,
    modules::{image::ImageModule, network::NetworkModule, partition::PartitionModule},
    status::{HostStatus, ReconcileState, UpdateKind},
};

pub mod image;
pub mod network;
pub mod partition;

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
        _host_config: &HostConfig,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Select the update kind based on the host status and host config.
    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfig,
    ) -> Option<UpdateKind> {
        Some(UpdateKind::HotPatch)
    }

    /// Migrate state from A-partition to B-partition (or vice versa).
    fn migrate(
        &mut self,
        _host_status: &mut HostStatus,
        _host_config: &HostConfig,
    ) -> Result<(), Error> {
        Ok(())
    }

    /// Reconcile the state of the system with the host config, and update the host status
    /// accordingly.
    fn reconcile(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfig,
    ) -> Result<(), Error>;
}

lazy_static::lazy_static! {
    pub static ref MODULES: Mutex<Vec<Box<dyn Module>>> = Mutex::new(vec![
        Box::<PartitionModule>::default(),
        Box::<ImageModule>::default(),
        Box::<NetworkModule>::default(),
    ]);
}

pub fn apply_host_config(host_config: &HostConfig, clean_install: bool) -> Result<(), Error> {
    // This is a safety check so that nobody accidentally formats their dev machine.
    if clean_install
        && !fs::read_to_string("/proc/cmdline")
            .context("Failed to read /proc/cmdline")?
            .contains("root=/dev/ram0")
    {
        bail!("Safety check failed! Requested clean install but not booted from ramdisk");
    }

    let mut modules = MODULES.lock().unwrap();

    // TODO: Persist the host status between runs
    let mut host_status = Default::default();

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

    if clean_install {
        PartitionModule::create_partitions(&mut host_status, host_config)
            .context("Failed to create disk partitions")?;

        image::stream_images(&mut host_status, host_config).context("Failed to stream images")?;

        // TODO: fstab updates and user creation should happen in modules (and not be hardcoded).
        image::chroot_exec(
            Path::new("/dev/disk/by-partlabel/mariner-root-a"),
            r#"sudo sh -c 'echo root:password | chpasswd'
            useradd -p $(openssl passwd -1 tink) -s /bin/bash -d /home/tink/ -m -G sudo tink
            "#,
        )
        .context("Failed to apply system config")?;
        host_status.reconcile_state = ReconcileState::CleanInstall;
    } else {
        let update_kind = modules
            .iter()
            .filter_map(|m| m.select_update_kind(&host_status, host_config))
            .max();
        host_status.reconcile_state = match update_kind {
            Some(k) => ReconcileState::UpdateInProgress(k),
            None => ReconcileState::Ready,
        }

        // TODO: Call pre-update workload hook.
    }

    match host_status.reconcile_state {
        ReconcileState::Ready => {
            info!("No updates required");
            return Ok(());
        }
        ReconcileState::CleanInstall => {
            info!("Performing clean install");
        }
        ReconcileState::UpdateInProgress(UpdateKind::HotPatch) => {
            info!("Performing hot patch update");
        }
        ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate) => {
            info!("Performing normal update");
        }
        ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot) => {
            info!("Performing update and reboot");
        }
        ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
            info!("Performing A/B update");

            // TODO: Download update
            // TODO: Write update

            for m in &mut *modules {
                m.migrate(&mut host_status, host_config)
                    .context(format!("Module '{}' failed during pause", m.name()))?;
            }
        }
        ReconcileState::UpdateInProgress(UpdateKind::Incompatible) => {
            bail!("Requested host config is not compatible with current install");
        }
    }

    match host_status.reconcile_state {
        ReconcileState::CleanInstall | ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
            // TODO: Properly decide whether to use A or B partition.
            image::chroot_run(Path::new("/dev/disk/by-partlabel/mariner-root-a"), || {
                for m in &mut *modules {
                    m.reconcile(&mut host_status, host_config)
                        .context(format!("Module '{}' failed during reconcile", m.name()))?;
                }
                Ok(())
            })
            .context("Failed to reconcile modules within chroot")?;
        }
        _ => {
            for m in &mut *modules {
                m.reconcile(&mut host_status, host_config)
                    .context(format!("Module '{}' failed during reconcile", m.name()))?;
            }
        }
    }

    // TODO: Call post-update workload hook.

    match host_status.reconcile_state {
        ReconcileState::CleanInstall
        | ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot)
        | ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
            info!("Rebooting");
            image::kexec(
                Path::new("/dev/disk/by-partlabel/mariner-root-a"),
                "console=tty1 console=ttyS0",
            )
            .context("Failed to perform kexec")?;
            unreachable!("kexec should never return");
        }
        ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate)
        | ReconcileState::UpdateInProgress(UpdateKind::HotPatch) => {
            info!("Update complete");
        }
        ReconcileState::Ready | ReconcileState::UpdateInProgress(UpdateKind::Incompatible) => {
            unreachable!()
        }
    }
    host_status.reconcile_state = ReconcileState::Ready;

    Ok(())
}
