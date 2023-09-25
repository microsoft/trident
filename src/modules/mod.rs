use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{bail, Context, Error};
use log::info;

use trident_api::{
    config::{
        DatastoreConfiguration, HostConfiguration, HostConfigurationSource, LocalConfigFile,
        Operations, TridentConfiguration,
    },
    status::{BlockDeviceInfo, HostStatus, ReconcileState, UpdateKind},
};

use crate::{
    datastore::DataStore,
    get_block_device,
    modules::{osconfig::OsConfigModule, storage::path_to_mount_point},
    mount::enter_chroot,
    TRIDENT_BINARY_PATH, TRIDENT_DATASTORE_PATH,
};
use crate::{
    modules::{image::ImageModule, network::NetworkModule, storage::StorageModule},
    mount::UpdateTargetEnvironment,
};
use crate::{
    mount::{setup_root_chroot, unmount_target_volumes},
    TRIDENT_LOCAL_CONFIG_PATH,
};

pub mod image;
pub mod network;
pub mod osconfig;
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
        None
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
        Box::<OsConfigModule>::default(),
    ]);
}

pub(crate) fn provision(
    host_config: &HostConfiguration,
    trident_config: &TridentConfiguration,
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

    if !trident_config
        .allowed_operations
        .contains(Operations::Update)
    {
        info!("Update not requested, skipping reconcile");
        return Ok(());
    }

    StorageModule::create_partitions(&mut host_status, host_config)
        .context("Failed to create disk partitions")?;

    image::refresh_ab_volumes(&mut host_status, host_config);

    image::stream_images(&mut host_status, host_config).context("Failed to stream images")?;

    let datastore_path = validate_datastore_location(trident_config, host_config)?;

    let mut chroot_env = setup_root_chroot(host_config, &host_status, false)
        .context("Failed to setup target root chroot")?;

    if let Some(chroot_env) = chroot_env.as_mut() {
        if trident_config.self_upgrade {
            // for development only, copy provisioning OS Trident binary to the runtime OS
            // to ensure we are using the latest bits
            fs::copy(
                TRIDENT_BINARY_PATH,
                chroot_env.mount_path.join(&TRIDENT_BINARY_PATH[1..]),
            )
            .context("Failed to copy Trident binary to runtime OS")?;
        }

        chroot_env.chroot = Some(enter_chroot(&chroot_env.mount_path)?);

        if datastore_path.exists() {
            bail!("Datastore already exists");
        }
        fs::create_dir_all(datastore_path.parent().unwrap())
            .context("Failed to create trident datastore directory")?;
        let mut state = DataStore::create(datastore_path.as_path(), host_status)?;

        for m in &mut *modules {
            state.with_host_status(|s| {
                m.reconcile(s, host_config)
                    .context(format!("Module '{}' failed during reconcile", m.name()))
            })?;
        }

        inject_trident_config(
            trident_config.phonehome.clone(),
            datastore_path.as_path(),
            host_config,
        )?;

        // TODO: Call post-update workload hook.
        drop(state);
    }

    if !trident_config
        .allowed_operations
        .contains(Operations::Transition)
    {
        info!("Transition not requested, skipping transition");
        if let Some(chroot_env) = chroot_env {
            chroot_env
                .chroot
                .context("Failed to enter chroot")?
                .exit()
                .context("Failed to exit chroot")?;
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
    trident_config: &TridentConfiguration,
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

    if !trident_config
        .allowed_operations
        .contains(Operations::Update)
    {
        info!("Update not requested, skipping reconcile");
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
        Some(UpdateKind::AbUpdate) => {
            info!("Performing A/B update");
            state.with_host_status(|s| {
                image::stream_images(s, host_config).context("Failed to stream images")?;
                Ok(())
            })?;
        }
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

        chroot_env = setup_root_chroot(host_config, state.host_status(), false)
            .context("Failed to setup root chroot")?;
        should_reconcile = chroot_env.is_some();
    }

    if should_reconcile {
        let datastore_path = validate_datastore_location(trident_config, host_config)?;

        if update_kind == Some(UpdateKind::AbUpdate) {
            if let Some(chroot_env) = chroot_env.as_mut() {
                if trident_config.self_upgrade {
                    // development only, copy provisioning OS Trident binary to the runtime OS
                    // to ensure we are using the latest bits
                    fs::copy(
                        TRIDENT_BINARY_PATH,
                        chroot_env.mount_path.join(&TRIDENT_BINARY_PATH[1..]),
                    )
                    .context("Failed to copy Trident binary to runtime OS")?;
                }

                chroot_env.chroot = Some(enter_chroot(&chroot_env.mount_path)?);
            }
        }

        for m in &mut *modules {
            state.with_host_status(|s| {
                m.reconcile(s, host_config)
                    .context(format!("Module '{}' failed during reconcile", m.name()))
            })?;
        }

        if update_kind == Some(UpdateKind::AbUpdate) {
            inject_trident_config(
                trident_config.phonehome.clone(),
                datastore_path.as_path(),
                host_config,
            )?;
        }
    }

    // TODO: Call post-update workload hook.

    match update_kind {
        Some(UpdateKind::UpdateAndReboot) | Some(UpdateKind::AbUpdate) => {
            drop(state);

            if !trident_config
                .allowed_operations
                .contains(Operations::Transition)
            {
                info!("Transition not requested, skipping transition");
                if let Some(chroot_env) = chroot_env {
                    chroot_env
                        .chroot
                        .context("Failed to enter chroot")?
                        .exit()
                        .context("Failed to exit chroot")?;
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

fn validate_datastore_location(
    trident_config: &TridentConfiguration,
    host_config: &HostConfiguration,
) -> Result<PathBuf, Error> {
    let datastore_path = match &trident_config.datastore {
        Some(DatastoreConfiguration::Create { create_path }) => create_path.clone(),
        Some(DatastoreConfiguration::Load { load_path }) => load_path.clone(),
        None => PathBuf::from(TRIDENT_DATASTORE_PATH),
    };
    datastore_path
        .extension()
        .and_then(|ext| if ext == "sqlite" { Some(()) } else { None })
        .ok_or(anyhow::anyhow!(
            "Datastore path must end with '.sqlite' but received '{}'",
            datastore_path.display()
        ))?;

    let datastore_block_device_id = &path_to_mount_point(host_config, datastore_path.as_path())
        .map(|mp| &mp.target_id)
        .context("Failed to find mount point for datastore")?;

    if host_config
        .imaging
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
    Ok(datastore_path)
}

fn inject_trident_config(
    orchestrator_url: Option<String>,
    datastore_path: &Path,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    fs::create_dir_all(Path::new(TRIDENT_LOCAL_CONFIG_PATH).parent().unwrap())
        .context("Failed to create trident config directory")?;
    let trident_config = LocalConfigFile {
        trident_config: TridentConfiguration {
            datastore: Some(DatastoreConfiguration::Load {
                load_path: datastore_path.to_path_buf(),
            }),
            phonehome: orchestrator_url,
            ..Default::default()
        },
        host_config_source: HostConfigurationSource::Embedded(Box::new(host_config.clone())),
    };
    fs::write(
        TRIDENT_LOCAL_CONFIG_PATH,
        serde_yaml::to_string(&trident_config).context("Failed to serialize trident config")?,
    )
    .context("Failed to write trident local config")?;
    Ok(())
}

fn transition(
    update_target_environment_option: Option<UpdateTargetEnvironment>,
) -> Result<(), Error> {
    match update_target_environment_option {
        Some(update_target_environment) => {
            update_target_environment
                .chroot
                .context("Failed to enter chroot")?
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
                        .context(format!(
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
) -> Option<BlockDeviceInfo> {
    host_config.storage.mount_points.iter().find_map(|mp| {
        if mp.path == Path::new("/") {
            get_block_device(host_status, &mp.target_id)
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use indoc::indoc;
    use trident_api::config::{HostConfiguration, TridentConfiguration};

    use super::validate_datastore_location;

    #[test]
    fn test_validate_datastore_location() {
        let trident_config_yaml = indoc! {r#"
            datastore: 
              create-path: /trident.sqlite
        "#};
        let trident_config: TridentConfiguration =
            serde_yaml::from_str(trident_config_yaml).unwrap();

        let host_config_yaml = indoc! {r#"
            storage:
              disks:
              mount-points:
                - path: /
                  target-id: sda1
                  filesystem: ext4
                  options: []
            imaging:
              images:
        "#};
        let host_config: HostConfiguration = serde_yaml::from_str(host_config_yaml).unwrap();

        assert_eq!(
            validate_datastore_location(&trident_config, &host_config).unwrap(),
            Path::new("/trident.sqlite")
        );

        let trident_config_yaml = indoc! {r#"
            datastore: 
              create-path: /trident.
        "#};
        let trident_config: TridentConfiguration =
            serde_yaml::from_str(trident_config_yaml).unwrap();

        // expect failure as the datastore path needs to end with .sqlite
        assert!(validate_datastore_location(&trident_config, &host_config).is_err());

        let trident_config_yaml = indoc! {r#"
            datastore: 
              load-path: /foo/trident.sqlite
        "#};
        let trident_config: TridentConfiguration =
            serde_yaml::from_str(trident_config_yaml).unwrap();

        assert_eq!(
            validate_datastore_location(&trident_config, &host_config).unwrap(),
            Path::new("/foo/trident.sqlite")
        );

        let trident_config_yaml = indoc! {r#"
        "#};
        let trident_config: TridentConfiguration =
            serde_yaml::from_str(trident_config_yaml).unwrap();

        assert_eq!(
            validate_datastore_location(&trident_config, &host_config).unwrap(),
            Path::new("/var/lib/trident/datastore.sqlite")
        );

        let trident_config_yaml = indoc! {r#"
            datastore: 
              create-path: /foo/bar/trident.sqlite
        "#};
        let trident_config: TridentConfiguration =
            serde_yaml::from_str(trident_config_yaml).unwrap();

        let host_config_yaml = indoc! {r#"
            storage:
              disks:
              mount-points:
                - path: /
                  target-id: sda1
                  filesystem: ext4
                  options: []
                - path: /bar
                  target-id: sda2
                  filesystem: ext4
                  options: []
            imaging:
              images:
        "#};
        let host_config: HostConfiguration = serde_yaml::from_str(host_config_yaml).unwrap();

        assert_eq!(
            validate_datastore_location(&trident_config, &host_config).unwrap(),
            Path::new("/foo/bar/trident.sqlite")
        );

        let host_config_yaml = indoc! {r#"
            storage:
              disks:
              mount-points:
                - path: /
                  target-id: sda1
                  filesystem: ext4
                  options: []
                - path: /bar
                  target-id: sda2
                  filesystem: ext4
                  options: []
            imaging:
              images:
              ab-update:
                volume-pairs:
                    - id: sda2
                      volume-a-id: sda1
                      volume-b-id: sda2
                    - id: sda2
                      volume-a-id: sda2
                      volume-b-id: sda1
        "#};
        let host_config: HostConfiguration = serde_yaml::from_str(host_config_yaml).unwrap();

        assert_eq!(
            validate_datastore_location(&trident_config, &host_config).unwrap(),
            Path::new("/foo/bar/trident.sqlite")
        );

        let trident_config_yaml = indoc! {r#"
            datastore: 
              load-path: /bar/foo/trident.sqlite
        "#};
        let trident_config: TridentConfiguration =
            serde_yaml::from_str(trident_config_yaml).unwrap();

        let host_config_yaml = indoc! {r#"
            storage:
              disks:
              mount-points:
                - path: /
                  target-id: sda1
                  filesystem: ext4
                  options: []
                - path: /bar
                  target-id: sda2
                  filesystem: ext4
                  options: []
            imaging:
              images:
              ab-update:
                volume-pairs:
                    - id: sda1
                      volume-a-id: sda1
                      volume-b-id: sda2
                    - id: sda1
                      volume-a-id: sda2
                      volume-b-id: sda1
        "#};
        let host_config: HostConfiguration = serde_yaml::from_str(host_config_yaml).unwrap();

        assert_eq!(
            validate_datastore_location(&trident_config, &host_config).unwrap(),
            Path::new("/bar/foo/trident.sqlite")
        );

        let host_config_yaml = indoc! {r#"
            storage:
              disks:
              mount-points:
                - path: /
                  target-id: sda1
                  filesystem: ext4
                  options: []
                - path: /bar
                  target-id: sda2
                  filesystem: ext4
                  options: []
            imaging:
              images:
              ab-update:
                volume-pairs:
                    - id: sda1
                      volume-a-id: sda1
                      volume-b-id: sda2
                    - id: sda2
                      volume-a-id: sda2
                      volume-b-id: sda1
        "#};
        let host_config: HostConfiguration = serde_yaml::from_str(host_config_yaml).unwrap();

        // expect failure, as we cannot land on A/B volume
        assert!(validate_datastore_location(&trident_config, &host_config).is_err());
    }
}
