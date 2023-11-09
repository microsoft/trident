use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{bail, Context, Error};
use log::info;

use trident_api::{
    config::{BlockDeviceId, HostConfiguration, Operations, TridentConfiguration},
    status::{
        AbVolumeSelection, BlockDeviceInfo, HostStatus, Partition, ReconcileState, UpdateKind,
    },
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
    state: &mut DataStore,
) -> Result<(), Error> {
    // This is a safety check so that nobody accidentally formats their dev machine.
    if !fs::read_to_string("/proc/cmdline")
        .context("Failed to read /proc/cmdline")?
        .contains("root=/dev/ram0")
    {
        bail!("Safety check failed! Requested clean install but not booted from ramdisk");
    }

    let mut modules = MODULES.lock().unwrap();
    state.with_host_status(|s| s.reconcile_state = ReconcileState::CleanInstall)?;

    refresh_host_status(&mut modules, state)?;
    validate_host_config(&modules, state, host_config)?;

    if !trident.allowed_operations.contains(Operations::Update) {
        info!("Update not requested, skipping reconcile");
        return Ok(());
    }

    // TODO: We should have a way to indicate which modules setup the root mount point, and which
    // depend on it being in place. Right now we just depend on the "storage" and "image" modules
    // being the first ones to run.
    let mount_path = Path::new("/partitionMount");
    migrate(&mut modules, state, host_config, mount_path)?;

    let chroot = mount::enter_chroot(mount_path)?;
    state.persist(
        host_config
            .management
            .datastore_path
            .as_deref()
            .unwrap_or(Path::new(TRIDENT_DATASTORE_PATH)),
    )?;
    reconcile(&mut modules, state, host_config)?;

    let root_device_path = get_root_block_device_path(state.host_status())
        .context("Failed to get root block device")?;

    state.close();
    chroot.exit().context("Failed to exit chroot")?;

    if !trident.allowed_operations.contains(Operations::Transition) {
        info!("Transition not requested, skipping transition");
        mount::unmount_target_volumes(mount_path).context("Failed to unmount target volumes")?;
        return Ok(());
    }

    info!("Root device path: {:#?}", root_device_path);

    transition(mount_path, &root_device_path)?;

    Ok(())
}

pub(super) fn update(
    host_config: &HostConfiguration,
    trident: &TridentConfiguration,
    state: &mut DataStore,
) -> Result<(), Error> {
    let mut modules = MODULES.lock().unwrap();

    refresh_host_status(&mut modules, state)?;
    validate_host_config(&modules, state, host_config)?;

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
        migrate(&mut modules, state, host_config, mount_path)?;
        chroot = Some(mount::enter_chroot(mount_path)?);
    }

    reconcile(&mut modules, state, host_config)?;

    if let Some(chroot) = chroot {
        chroot.exit().context("Failed to exit chroot")?;
    }

    match update_kind {
        Some(UpdateKind::UpdateAndReboot) | Some(UpdateKind::AbUpdate) => {
            let root_block_device_path = get_root_block_device_path(state.host_status())
                .context("Failed to get root block device")?;

            state.close();

            if !trident.allowed_operations.contains(Operations::Transition) {
                info!("Transition not requested, skipping transition");
                mount::unmount_target_volumes(mount_path)
                    .context("Failed to unmount target volumes")?;
                return Ok(());
            }

            info!("Root device path: {:#?}", root_block_device_path);

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

/// Using the / mount point, figure out what should be used as a root block device.
fn get_root_block_device_path(host_status: &HostStatus) -> Option<PathBuf> {
    host_status
        .storage
        .mount_points
        .iter()
        .find(|(_, mp)| mp.path == Path::new("/"))
        .and_then(|(target_id, _)| Some(get_block_device(host_status, target_id, false)?.path))
}

fn get_disk(host_status: &HostStatus, block_device_id: &BlockDeviceId) -> Option<BlockDeviceInfo> {
    host_status
        .storage
        .disks
        .get(block_device_id)
        .map(|d| d.to_block_device())
}

fn get_partition(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<BlockDeviceInfo> {
    host_status
        .storage
        .disks
        .iter()
        .flat_map(|(_block_device_id, disk)| &disk.partitions)
        .find(|p| p.id == *block_device_id)
        .map(Partition::to_block_device)
}

fn get_raid_array(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
) -> Option<BlockDeviceInfo> {
    host_status
        .storage
        .raid_arrays
        .get(block_device_id)
        .map(|r| r.to_block_device())
}

/// Returns a block device info for a block device referenced by the
/// `block_device_id`. If the volume is part of an AB Volume Pair and active is
/// true it returns the active volume, and if active is false it returns the
/// update volume (i.e. the one that isn't active).
fn get_block_device(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
    active: bool,
) -> Option<BlockDeviceInfo> {
    get_disk(host_status, block_device_id)
        .or_else(|| get_partition(host_status, block_device_id))
        .or_else(|| get_ab_volume(host_status, block_device_id, active))
        .or_else(|| get_raid_array(host_status, block_device_id))
}

/// Returns a block device info for a volume from the given AB Volume Pair. If
/// active is true it returns the active volume, and if active is false it
/// returns the update volume (i.e. the one that isn't active).
fn get_ab_volume(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
    active: bool,
) -> Option<BlockDeviceInfo> {
    if let Some(ab_update) = &host_status.imaging.ab_update {
        let ab_volume = ab_update
            .volume_pairs
            .iter()
            .find(|v| v.0 == block_device_id);
        if let Some(v) = ab_volume {
            return get_ab_update_volume(host_status, active).and_then(
                |selection| match selection {
                    AbVolumeSelection::VolumeA => {
                        get_block_device(host_status, &v.1.volume_a_id, false)
                    }
                    AbVolumeSelection::VolumeB => {
                        get_block_device(host_status, &v.1.volume_b_id, false)
                    }
                },
            );
        }
    }

    None
}

/// Returns the volume selection for all AB Volume Pairs. This is used to
/// determine which volumes are currently active and which are meant for
/// updating. In addition, if active is true and an A/B update is in progress,
/// the active volume selection will be returned. If active is false, the volume
/// selection corresponding to the volumes to be updated will be returned.
fn get_ab_update_volume(host_status: &HostStatus, active: bool) -> Option<AbVolumeSelection> {
    let active_volume = &host_status.imaging.ab_update.as_ref()?.active_volume;
    match &host_status.reconcile_state {
        ReconcileState::UpdateInProgress(UpdateKind::HotPatch)
        | ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate)
        | ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot) => *active_volume,
        ReconcileState::UpdateInProgress(UpdateKind::AbUpdate) => {
            if active {
                *active_volume
            } else {
                Some(if *active_volume == Some(AbVolumeSelection::VolumeA) {
                    AbVolumeSelection::VolumeB
                } else {
                    AbVolumeSelection::VolumeA
                })
            }
        }
        ReconcileState::UpdateInProgress(UpdateKind::Incompatible) => None,
        ReconcileState::Ready => {
            if active {
                *active_volume
            } else {
                None
            }
        }
        ReconcileState::CleanInstall => Some(AbVolumeSelection::VolumeA),
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

    // TODO: Solve hard reboot(), so that the firmware chooses the correct boot
    // partition to boot from, after each A/B update. Related ADO task:
    // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6169.
    //
    // info!("Performing hard reboot");
    // image::reboot().context("Failed to perform hard reboot")
}

#[cfg(test)]
mod test {
    use trident_api::status::BlockDeviceContents;

    use super::*;
    use indoc::indoc;

    #[test]
    fn test_get_root_block_device_path() {
        let host_status_yaml = indoc::indoc! {r#"
            storage:
              disks:
                foo: 
                  uuid: 00000000-0000-0000-0000-000000000000
                  path: /dev/sda
                  capacity: 10
                  contents: initialized
                  partitions:
                    - uuid: 00000000-0000-0000-0000-000000000001
                      path: /dev/sda1
                      id: boot
                      start: 1
                      end: 3
                      type: esp
                      contents: initialized
                    - uuid: 00000000-0000-0000-0000-000000000002
                      path: /dev/sda2
                      id: root
                      start: 4
                      end: 10
                      type: root
                      contents: initialized
              raid-arrays: {}
              mount-points:
                boot:
                  path: /boot
                  filesystem: fat32
                  options: []
                root:
                  path: /
                  filesystem: ext4
                  options: []
            reconcile-state: clean-install
            imaging:
            "#};
        let host_status: HostStatus = serde_yaml::from_str(host_status_yaml).unwrap();

        assert_eq!(
            get_root_block_device_path(&host_status),
            Some(PathBuf::from("/dev/sda2"))
        );
    }

    /// Validates that the `get_block_device_for_update` function works as expected for
    /// disks, partitions and ab volumes.
    #[test]
    fn test_get_block_device_for_update() {
        let host_status_yaml = indoc! {r#"
            storage:
                mount-points:
                disks:
                    os:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: 0
                        contents: unknown
                        partitions:
                          - id: efi
                            path: /dev/disk/by-partlabel/osp1
                            contents: unknown
                            start: 0
                            end: 0
                            type: esp
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: root
                            path: /dev/disk/by-partlabel/osp2
                            contents: unknown
                            start: 100
                            end: 1000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: rootb
                            path: /dev/disk/by-partlabel/osp3
                            contents: unknown
                            start: 1000
                            end: 10000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                    data:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: 1000
                        contents: unknown
                        partitions: []
                raid-arrays:
            imaging:
                ab-update:
                    volume-pairs:
                        osab:
                            volume-a-id: root
                            volume-b-id: rootb
            reconcile-state: clean-install
        "#};
        let mut host_status: HostStatus = serde_yaml::from_str(host_status_yaml).unwrap();

        assert_eq!(
            get_block_device(&host_status, &"os".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"efi".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"root".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"foobar".to_owned(), false),
            None
        );
        assert_eq!(
            get_block_device(&host_status, &"data".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 1000,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            super::get_block_device(&host_status, &"osab".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), false).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                size: 9000,
                contents: BlockDeviceContents::Unknown,
            }
        );

        assert_eq!(
            get_block_device(&host_status, &"osab".to_owned(), true).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                size: 900,
                contents: BlockDeviceContents::Unknown,
            }
        );
    }

    /// Validates logic for determining which A/B volume to use
    fn test_get_ab_update_volume(active: bool) -> HostStatus {
        let host_status_yaml = indoc! {r#"
            storage:
                disks:
                mount-points:
                raid-arrays:
            imaging:
                ab-update:
                    volume-pairs:
            reconcile-state: clean-install
        "#};
        let mut host_status: HostStatus = serde_yaml::from_str(host_status_yaml).unwrap();

        // test that clean-install will always use volume A for updates
        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);

        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeA)
        );

        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);

        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeA)
        );

        // test that UpdateInProgress(HostPatch, NormalUpdate, UpdateAndReboot)
        // will always use the active volume for updates
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::HotPatch);
        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeB)
        );
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::NormalUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeB)
        );
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::UpdateAndReboot);
        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(
            get_ab_update_volume(&host_status, active),
            Some(AbVolumeSelection::VolumeA)
        );

        // test that UpdateInProgress(Incompatible) will return None
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::Incompatible);
        assert_eq!(get_ab_update_volume(&host_status, active), None);

        host_status
    }

    /// Validates logic for determining which A/B volume to update
    #[test]
    fn test_get_ab_update_volume_update() {
        let mut host_status = test_get_ab_update_volume(false);

        // test that Ready will return the None
        host_status.reconcile_state = ReconcileState::Ready;
        assert_eq!(get_ab_update_volume(&host_status, false), None);

        // test that UpdateInProgress(AbUpdate) will use the opposite volume
        // for updates
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status, false),
            Some(AbVolumeSelection::VolumeB)
        );
        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_ab_update_volume(&host_status, false),
            Some(AbVolumeSelection::VolumeA)
        );
    }

    /// Validates logic for determining which A/B volume is active
    #[test]
    fn test_get_ab_update_volume_active() {
        let mut host_status = test_get_ab_update_volume(true);

        // test that Ready will return the active volume
        host_status.reconcile_state = ReconcileState::Ready;
        assert_eq!(
            get_ab_update_volume(&host_status, true),
            Some(AbVolumeSelection::VolumeA)
        );

        // test that UpdateInProgress(AbUpdate) will use the active volume
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        assert_eq!(
            get_ab_update_volume(&host_status, true),
            Some(AbVolumeSelection::VolumeA)
        );
        host_status
            .imaging
            .ab_update
            .as_mut()
            .unwrap()
            .active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            get_ab_update_volume(&host_status, true),
            Some(AbVolumeSelection::VolumeB)
        );
    }

    /// Validates logic for querying disks and partitions.
    #[test]
    fn test_get_disk_partition() {
        let host_status_yaml = indoc! {r#"
            storage:
                mount-points:
                disks:
                    os:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: 0
                        contents: unknown
                        partitions:
                          - id: efi
                            path: /dev/disk/by-partlabel/osp1
                            contents: unknown
                            start: 0
                            end: 0
                            type: esp
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: root
                            path: /dev/disk/by-partlabel/osp2
                            contents: unknown
                            start: 100
                            end: 1000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: rootb
                            path: /dev/disk/by-partlabel/osp3
                            contents: unknown
                            start: 1000
                            end: 10000
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                raid-arrays:
            imaging:
                ab-update:
                    volume-pairs:
            reconcile-state: clean-install
        "#};
        let host_status: HostStatus = serde_yaml::from_str(host_status_yaml).unwrap();

        assert_eq!(
            get_disk(&host_status, &"os".to_owned()).unwrap(),
            BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-bus/foobar"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            }
        );
        assert_eq!(get_disk(&host_status, &"efi".to_owned()), None);
        assert_eq!(get_partition(&host_status, &"os".to_owned()), None);
        assert_eq!(
            get_partition(&host_status, &"efi".to_owned()),
            Some(BlockDeviceInfo {
                path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                size: 0,
                contents: BlockDeviceContents::Unknown,
            })
        );
    }
}
