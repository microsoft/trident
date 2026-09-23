use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Error};
use filesystem::FileSystemData;
use log::{debug, trace};

use osutils::osrelease::{Distro, OsRelease};
use trident_api::{
    config::{HostConfiguration, Partition, VerityDevice},
    constants::{
        internal_params::{
            STREAM_SLOW_SPEED_REPORTING_INTERVAL_SECONDS,
            STREAM_SLOW_SPEED_REPORTING_THRESHOLD_MBPS,
        },
        ROOT_MOUNT_POINT_PATH, USR_MOUNT_POINT_PATH,
    },
    error::{InternalError, ReportError, TridentError},
    status::{AbVolumeSelection, ServicingType},
    storage_graph::graph::StorageGraph,
    BlockDeviceId,
};

use crate::{
    osimage::OsImage, STREAM_SLOW_SPEED_REPORTING_INTERVAL_SECONDS_DEFAULT,
    STREAM_SLOW_SPEED_REPORTING_THRESHOLD_MBPS_DEFAULT,
};

#[allow(dead_code)]
pub mod filesystem;
pub(crate) mod image;
#[cfg(test)]
mod test_utils;

/// Helper struct to consolidate the info on the A/B volume pair. Contains the paths and block
/// device IDs for both volumes.
#[derive(Debug, PartialEq)]
pub(crate) struct AbVolumePairInfo {
    pub volume_a_path: PathBuf,
    pub volume_b_path: PathBuf,
    pub volume_a_id: BlockDeviceId,
    pub volume_b_id: BlockDeviceId,
}

/// Parameters for constructing an [`EngineContext`]. All fields must be specified explicitly;
/// `filesystems` is intentionally excluded and will be initialized to an empty vector.
pub struct EngineContextParams {
    pub spec: HostConfiguration,
    pub spec_old: HostConfiguration,
    pub servicing_type: ServicingType,
    pub is_stream_image: bool,
    pub partition_paths: BTreeMap<BlockDeviceId, PathBuf>,
    pub ab_active_volume: Option<AbVolumeSelection>,
    pub disk_uuids: HashMap<BlockDeviceId, uuid::Uuid>,
    pub install_index: usize,
    pub image: Option<OsImage>,
    pub is_uki: Option<bool>,
}

#[cfg_attr(any(test, feature = "functional-test"), derive(Clone))]
pub struct EngineContext {
    pub spec: HostConfiguration,

    pub spec_old: HostConfiguration,

    /// Type of servicing that Trident is executing on the host.
    pub servicing_type: ServicingType,

    /// Whether the engine is running a stream-image install from a raw COSI
    /// image. When true, the image is deployed as-is and host-specific
    /// post-install mutations (bootloader, initrd, SELinux, fstab, etc.) are
    /// skipped.
    pub is_stream_image: bool,

    /// The path associated with each partition in the Host Configuration.
    pub partition_paths: BTreeMap<BlockDeviceId, PathBuf>,

    /// A/B update status.
    pub ab_active_volume: Option<AbVolumeSelection>,

    /// Stores the Disks UUID to ID mapping of the host.
    pub disk_uuids: HashMap<BlockDeviceId, uuid::Uuid>,

    /// Index of the current Azure Linux install. Used to distinguish between
    /// different installs of Azure Linux on the same host.
    ///
    /// An AzL "install" is the result of a deployment of Azure Linux (e.g. with
    /// Trident), and encompasses the entire deployment, including both A/B
    /// volumes (when present).
    ///
    /// Indexes are assigned sequentially, starting from 0. On a clean install,
    /// Trident will determine the next available index and use it for the new
    /// install.
    pub install_index: usize,

    /// The OS image that Trident is using to service the host.
    pub image: Option<OsImage>,

    /// The storage graph representing the storage configuration of the host.
    pub storage_graph: StorageGraph,

    /// All of the filesystems in the system.
    pub filesystems: Vec<FileSystemData>,

    /// Whether the image will use a UKI or not.
    pub is_uki: Option<bool>,

    /// The mount path of the ESP partition.
    pub esp_mount_path: PathBuf,

    /// Host OsRelease
    pub host_os_release: OsRelease,
}

#[cfg(any(test, feature = "functional-test"))]
impl Default for EngineContext {
    fn default() -> Self {
        use trident_api::constants::DEFAULT_ESP_MOUNT_POINT_PATH;
        Self {
            esp_mount_path: PathBuf::from(DEFAULT_ESP_MOUNT_POINT_PATH),
            spec: Default::default(),
            spec_old: Default::default(),
            servicing_type: Default::default(),
            is_stream_image: false,
            partition_paths: Default::default(),
            ab_active_volume: Default::default(),
            disk_uuids: Default::default(),
            install_index: Default::default(),
            image: Default::default(),
            storage_graph: Default::default(),
            filesystems: Default::default(),
            is_uki: Default::default(),
            host_os_release: Default::default(),
        }
    }
}

impl EngineContext {
    /// Creates a new `EngineContext` from the given [`EngineContextParams`]. The
    /// `filesystems` field is initialized to an empty vector and should be
    /// populated later if needed via [`EngineContext::populate_filesystems`].
    pub fn new(params: EngineContextParams) -> Result<Self, TridentError> {
        let graph = super::build_storage_graph(&params.spec.storage)?;

        // Get the ESP mount path from the storage graph. This should be
        // guaranteed to exist in validated Host configurations.
        let esp_mount_path =
            graph
                .esp_mount_path()
                .ok_or(TridentError::new(InternalError::Internal(
                    "Storage graph does not contain an ESP mount path, the Host Configuration was not validated properly.",
                )))?
                .to_path_buf();

        let os_release = OsRelease::read().structured(InternalError::Internal(
            "Failed to read OS release information",
        ))?;
        Ok(Self {
            spec: params.spec,
            spec_old: params.spec_old,
            servicing_type: params.servicing_type,
            is_stream_image: params.is_stream_image,
            ab_active_volume: params.ab_active_volume,
            partition_paths: params.partition_paths,
            disk_uuids: params.disk_uuids,
            install_index: params.install_index,
            image: params.image,
            storage_graph: graph,
            filesystems: Vec::new(),
            is_uki: params.is_uki,
            esp_mount_path,
            host_os_release: os_release,
        })
    }

    /// Returns the update volume selection for all A/B volume pairs. The update volume is the one
    /// that is meant to be updated, based on the servicing in progress, if any.
    pub fn get_ab_update_volume(&self) -> Option<AbVolumeSelection> {
        match self.servicing_type {
            // If there is no servicing in progress, update volume is None.
            ServicingType::NoActiveServicing => None,
            // If host is executing a manual rollback for a runtime update, active and update
            // volumes are the same.
            ServicingType::ManualRollbackRuntime
            // If host is executing a runtime update, active and update volumes are the same.
            | ServicingType::RuntimeUpdate => self.ab_active_volume,

            // If host is executing a manual rollback for an A/B update, update volume
            // is the opposite of the active volume.
            ServicingType::ManualRollbackAb
            // If host is executing an A/B update, update volume is the opposite of active volume.
            | ServicingType::AbUpdate => {
                if self.ab_active_volume == Some(AbVolumeSelection::VolumeA) {
                    Some(AbVolumeSelection::VolumeB)
                } else {
                    Some(AbVolumeSelection::VolumeA)
                }
            }

            // If host is executing a clean install, update volume is always A.
            ServicingType::CleanInstall => Some(AbVolumeSelection::VolumeA),
        }
    }

    /// Using the `/` mount point, fetches the root block device ID.
    pub(super) fn get_root_block_device_id(&self) -> Option<BlockDeviceId> {
        self.spec
            .storage
            .path_to_filesystem(ROOT_MOUNT_POINT_PATH)
            .and_then(|f| f.device_id.clone())
    }

    /// Using the `/` mount point, fetches the root block device path.
    pub(super) fn get_root_block_device_path(&self) -> Option<PathBuf> {
        self.get_root_block_device_id()
            .and_then(|id| self.get_block_device_path(&id))
    }

    /// Using the `/usr` mount point, fetches the usr block device ID.
    pub(super) fn get_usr_block_device_id(&self) -> Option<BlockDeviceId> {
        self.spec
            .storage
            .path_to_filesystem(USR_MOUNT_POINT_PATH)
            .and_then(|f| f.device_id.clone())
    }

    /// Returns the path of the block device with id `block_device_id`.
    ///
    /// If the volume is part of an A/B volume pair, this function returns the update volume, i.e.
    /// the one that isn't active.
    pub(crate) fn get_block_device_path(&self, block_device_id: &BlockDeviceId) -> Option<PathBuf> {
        if let Some(partition_path) = self.partition_paths.get(block_device_id) {
            return Some(partition_path.clone());
        }

        if let Some(raid) = self
            .spec
            .storage
            .raid
            .software
            .iter()
            .find(|r| &r.id == block_device_id)
        {
            return Some(raid.device_path());
        }

        if let Some(encryption) = &self.spec.storage.encryption {
            if let Some(encrypted) = encryption.volumes.iter().find(|e| &e.id == block_device_id) {
                return Some(encrypted.device_path());
            }
        }

        if let Some(verity) = self
            .spec
            .storage
            .verity
            .iter()
            .find(|v| &v.id == block_device_id)
        {
            return Some(verity.device_path());
        }

        self.get_ab_volume_block_device_id(block_device_id)
            .and_then(|child_block_device_id| self.get_block_device_path(child_block_device_id))
    }

    /// Returns the block device id for the update volume from the given A/B volume pair.
    pub(super) fn get_ab_volume_block_device_id(
        &self,
        block_device_id: &BlockDeviceId,
    ) -> Option<&BlockDeviceId> {
        if let Some(ab_update) = &self.spec.storage.ab_update {
            let ab_volume = ab_update
                .volume_pairs
                .iter()
                .find(|v| &v.id == block_device_id);
            if let Some(v) = ab_volume {
                let selection = self.get_ab_update_volume();
                // Return the appropriate BlockDeviceId based on the selection
                return selection.map(|sel| match sel {
                    AbVolumeSelection::VolumeA => &v.volume_a_id,
                    AbVolumeSelection::VolumeB => &v.volume_b_id,
                });
            };
        }
        None
    }

    /// Returns A/B volume pair info based on a given block device ID.
    pub(crate) fn get_ab_volume_pair(
        &self,
        device_id: &BlockDeviceId,
    ) -> Result<AbVolumePairInfo, Error> {
        let ab_volume_pair = self
            .spec
            .storage
            .ab_update
            .as_ref()
            .context("No A/B update configuration found")?
            .volume_pairs
            .iter()
            .find(|p| &p.id == device_id)
            .context(format!(
                "No volume pair for block device ID '{device_id}' found"
            ))?;

        debug!(
            "A/B volume pair with block device ID '{}': {:?}",
            device_id, ab_volume_pair
        );

        let volume_a_path = self
            .get_block_device_path(&ab_volume_pair.volume_a_id)
            .context(format!(
                "Failed to get block device path for volume A with ID '{}'",
                ab_volume_pair.volume_a_id
            ))?;
        let volume_b_path = self
            .get_block_device_path(&ab_volume_pair.volume_b_id)
            .context(format!(
                "Failed to get block device path for volume B with ID '{}'",
                ab_volume_pair.volume_b_id
            ))?;

        Ok(AbVolumePairInfo {
            volume_a_path,
            volume_b_path,
            volume_a_id: ab_volume_pair.volume_a_id.clone(),
            volume_b_id: ab_volume_pair.volume_b_id.clone(),
        })
    }

    /// Returns the configuration for the verity device for the given block device ID.
    pub(crate) fn get_verity_config(
        &self,
        device_id: &BlockDeviceId,
    ) -> Result<VerityDevice, Error> {
        let verity_device_config = self
            .spec
            .storage
            .verity
            .iter()
            .find(|vd| &vd.id == device_id)
            .cloned()
            .context(format!(
                "Failed to find configuration for verity device '{device_id}'"
            ))?;

        trace!(
            "Config for verity device '{}': {:?}",
            device_id,
            verity_device_config
        );

        Ok(verity_device_config)
    }

    /// Returns the first partition that backs the given block device, or Err if the block device ID
    /// does not correspond to a partition or software RAID array.
    pub(crate) fn get_first_backing_partition<'a>(
        &'a self,
        block_device_id: &BlockDeviceId,
    ) -> Result<&'a Partition, Error> {
        if let Some(partition) = self.spec.storage.get_partition(block_device_id) {
            Ok(partition)
        } else if let Some(array) = self
            .spec
            .storage
            .raid
            .software
            .iter()
            .find(|r| &r.id == block_device_id)
        {
            let partition_id = array
                .devices
                .first()
                .context(format!("RAID array '{}' has no partitions", array.id))?;

            self.spec
                .storage
                .get_partition(partition_id)
                .context(format!(
                    "RAID array '{block_device_id}' doesn't reference partition"
                ))
        } else {
            bail!("Block device '{block_device_id}' is not a partition or RAID array")
        }
    }

    /// Returns the estimated size of the block device holding the filesystem that contains the
    /// given path. If the path is not mounted anywhere, or if the block device size cannot be
    /// estimated, returns None.
    pub(crate) fn filesystem_block_device_size(&self, path: impl AsRef<Path>) -> Option<u64> {
        let device = self
            .spec
            .storage
            .path_to_mount_point_info(path)
            .and_then(|mp| mp.device_id)?;

        self.storage_graph.block_device_size(device)
    }

    /// Convience method to check if the current context is a UKI context and return a suitable
    /// error if the flag isn't set.
    #[track_caller]
    pub(crate) fn is_uki(&self) -> Result<bool, TridentError> {
        self.is_uki.structured(InternalError::Internal(
            "is_uki() called without it being set",
        ))
    }

    /// Returns the zstd max window log required for decompression of files
    /// coming from the OS image, if available.
    pub(crate) fn image_zstd_max_window_log(&self) -> Option<u32> {
        self.image
            .as_ref()?
            .zstd_decompression_parameters()
            .and_then(|p| p.max_window_log)
    }

    /// Returns the threshold and interval for reporting slow streaming speed.
    pub(crate) fn read_monitor_params(&self) -> Result<(f64, Duration), TridentError> {
        Ok((
            self.spec
                .internal_params
                .get::<f64>(STREAM_SLOW_SPEED_REPORTING_THRESHOLD_MBPS)
                .transpose()
                .map_err(TridentError::new)?
                .unwrap_or(STREAM_SLOW_SPEED_REPORTING_THRESHOLD_MBPS_DEFAULT),
            Duration::from_secs(
                self.spec
                    .internal_params
                    .get_u64(STREAM_SLOW_SPEED_REPORTING_INTERVAL_SECONDS)
                    .transpose()
                    .map_err(TridentError::new)?
                    .unwrap_or(STREAM_SLOW_SPEED_REPORTING_INTERVAL_SECONDS_DEFAULT),
            ),
        ))
    }

    /// Retrieves os-release data from the image.
    pub(crate) fn image_os_release(&self) -> &OsRelease {
        self.image
            .as_ref()
            .map(|img| img.os_release())
            .unwrap_or(&OsRelease::EMPTY)
    }

    /// Retrieves the distribution of the OS image.
    pub(crate) fn image_distro(&self) -> Distro {
        self.image_os_release().get_distro()
    }

    /// Trace feature usage for this servicing operation.
    ///
    /// Update Host Configurations often only specify a delta (e.g. just a new
    /// `image`), leaving unrelated fields at their defaults. For any field
    /// left at its default in `spec`, fall back to `spec_old` so the metric
    /// reflects what's actually configured on the host, rather than just
    /// what this invocation's payload happened to restate. On a clean
    /// install, `spec_old` is `HostConfiguration::default()`, so this falls
    /// back to `spec` for every field, matching prior behavior.
    pub fn feature_tracing(&self) {
        // Prefers `new`'s value; falls back to `old`'s value only when
        // `new`'s value is still the type's default (i.e. likely just
        // unspecified in this invocation's Host Configuration).
        fn effective<T: Default + PartialEq + Clone>(new: &T, old: &T) -> T {
            if *new != T::default() {
                new.clone()
            } else {
                old.clone()
            }
        }

        let (new, old) = (&self.spec, &self.spec_old);

        let verity = effective(&new.storage.verity, &old.storage.verity);

        tracing::info!(
            netplan = effective(&new.os.netplan, &old.os.netplan).is_some(),
            selinux = match effective(&new.os.selinux.mode, &old.os.selinux.mode) {
                Some(mode) => mode.to_string(),
                _ => "none".to_string(),
            },
            modules = !effective(&new.os.modules, &old.os.modules).is_empty(),
            sysexts = !effective(&new.os.sysexts, &old.os.sysexts).is_empty(),
            confexts = !effective(&new.os.confexts, &old.os.confexts).is_empty(),
            services_enabled =
                !effective(&new.os.services.enable, &old.os.services.enable).is_empty(),
            services_disabled =
                !effective(&new.os.services.disable, &old.os.services.disable).is_empty(),
            kernel_command_line_options = !effective(
                &new.os.kernel_command_line.extra_command_line,
                &old.os.kernel_command_line.extra_command_line,
            )
            .is_empty(),
            uefi_fallback_mode =
                Into::<&str>::into(effective(&new.os.uefi_fallback, &old.os.uefi_fallback)),
            post_configure_scripts =
                !effective(&new.scripts.post_configure, &old.scripts.post_configure).is_empty(),
            pre_servicing_scripts =
                !effective(&new.scripts.pre_servicing, &old.scripts.pre_servicing).is_empty(),
            post_provision_scripts =
                !effective(&new.scripts.post_provision, &old.scripts.post_provision).is_empty(),
            encryption = match effective(&new.storage.encryption, &old.storage.encryption) {
                Some(encryption) => encryption
                    .pcrs
                    .iter()
                    .map(|pcr| pcr.to_num().to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                _ => "".to_string(),
            },
            ab_update = effective(&new.storage.ab_update, &old.storage.ab_update).is_some(),
            software_raid =
                !effective(&new.storage.raid.software, &old.storage.raid.software).is_empty(),
            usr_verity = verity.iter().any(|v| v.name == "usr"),
            root_verity = verity.iter().any(|v| v.name == "root"),
            internal_params = new.internal_params.get_set_params().join(","),
            metric_name = "host_config_feature_usage",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::str::FromStr;

    use const_format::formatcp;
    use maplit::btreemap;

    use osutils::testutils::repart::TEST_DISK_DEVICE_PATH;

    use trident_api::config::{
        self, AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
        MountOptions, MountPoint, Partition, PartitionSize, PartitionType, Raid, RaidLevel,
        SoftwareRaidArray, Storage, VerityDevice,
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
                                uuid: None,
                                label: None,
                            },
                            Partition {
                                id: "root".to_owned(),
                                size: 7.into(),
                                partition_type: PartitionType::Root,
                                uuid: None,
                                label: None,
                            },
                        ],
                        ..Default::default()
                    }],
                    filesystems: vec![
                        FileSystem {
                            device_id: Some("boot".to_owned()),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from("/boot"),
                                options: MountOptions::empty(),
                            }),
                            source: FileSystemSource::Image,
                            is_esp: false,
                        },
                        FileSystem {
                            device_id: Some("root".to_owned()),
                            mount_point: Some(MountPoint {
                                path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                                options: MountOptions::empty(),
                            }),
                            source: FileSystemSource::Image,
                            is_esp: false,
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "foo".to_owned() => PathBuf::from("/dev/sda"),
                "boot".to_owned() => PathBuf::from("/dev/sda1"),
                "root".to_owned() => PathBuf::from("/dev/sda2"),
            },
            ..Default::default()
        };

        assert_eq!(
            ctx.get_root_block_device_path(),
            Some(PathBuf::from("/dev/sda2"))
        );
    }

    /// Validates that get_block_device_for_update() works as expected for disks, partitions, and
    /// A/B volumes.
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
                                    uuid: None,
                                    label: None,
                                },
                                Partition {
                                    id: "root".to_owned(),
                                    size: 900.into(),
                                    partition_type: PartitionType::Root,
                                    uuid: None,
                                    label: None,
                                },
                                Partition {
                                    id: "rootb".to_owned(),
                                    size: 9000.into(),
                                    partition_type: PartitionType::Root,
                                    uuid: None,
                                    label: None,
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
            partition_paths: btreemap! {
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
            ctx.get_block_device_path(&"os".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-bus/foobar")
        );
        assert_eq!(
            ctx.get_block_device_path(&"efi".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp1")
        );
        assert_eq!(
            ctx.get_block_device_path(&"root".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp2")
        );
        assert_eq!(ctx.get_block_device_path(&"foobar".to_owned()), None);
        assert_eq!(
            ctx.get_block_device_path(&"data".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-bus/foobar")
        );

        // Now, set ab_active_volume to VolumeA.
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert_eq!(ctx.get_block_device_path(&"osab".to_owned()), None);
        assert_eq!(ctx.get_ab_volume_block_device_id(&"osab".to_owned()), None);

        // Now, set servicing type to AbUpdate.
        ctx.servicing_type = ServicingType::AbUpdate;
        assert_eq!(
            ctx.get_block_device_path(&"osab".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp3")
        );
        assert_eq!(
            ctx.get_ab_volume_block_device_id(&"osab".to_owned()),
            Some(&"rootb".to_owned())
        );

        // When active volume is VolumeB, should return VolumeA
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert_eq!(
            ctx.get_block_device_path(&"osab".to_owned()).unwrap(),
            PathBuf::from("/dev/disk/by-partlabel/osp2")
        );
        assert_eq!(
            ctx.get_ab_volume_block_device_id(&"osab".to_owned()),
            Some(&"root".to_owned())
        );

        // If target block device id does not exist, should return None.
        assert_eq!(
            ctx.get_ab_volume_block_device_id(&"non-existent".to_owned()),
            None
        );
    }

    /// Validates that get_ab_volume_pair() correctly returns the A/B volume pair.
    #[test]
    fn test_get_ab_volume_pair() {
        let mut ctx = EngineContext {
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            spec: HostConfiguration {
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case #1: If there is no A/B update configuration provided, returns an error.
        assert_eq!(
            ctx.get_ab_volume_pair(&"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No A/B update configuration found"
        );

        ctx.spec.storage.ab_update = Some(AbUpdate {
            volume_pairs: vec![AbVolumePair {
                id: "root".to_string(),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }],
        });

        // Test case #2: If an A/B volume pair with the given ID does not exist, returns an error.
        assert_eq!(
            ctx.get_ab_volume_pair(&"non-existent".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "No volume pair for block device ID 'non-existent' found"
        );

        // Test case #3.1: If there are no block devices defined, returns an error.
        assert_eq!(
            ctx.get_ab_volume_pair(&"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume A with ID 'root-a'"
        );

        ctx.partition_paths.insert(
            "root-a".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
        );

        // Test case #3.2: If there are no block devices defined, returns an error.
        assert_eq!(
            ctx.get_ab_volume_pair(&"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to get block device path for volume B with ID 'root-b'"
        );

        ctx.partition_paths.insert(
            "root-b".to_string(),
            PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
        );

        // Test case #4: When information is complete, returns the volume pair paths.
        assert_eq!(
            ctx.get_ab_volume_pair(&"root".to_string()).unwrap(),
            AbVolumePairInfo {
                volume_a_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                volume_b_path: PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}2")),
                volume_a_id: "root-a".to_string(),
                volume_b_id: "root-b".to_string(),
            }
        );
    }

    #[test]
    fn test_get_verity_config() {
        let mut ctx = EngineContext {
            spec: HostConfiguration {
                storage: config::Storage {
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case #0: If there is no internal verity device configuration, returns an error.
        assert_eq!(
            ctx.get_verity_config(&"root".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for verity device 'root'"
        );

        // Test case #1. Add a verity device config and ensure it is returned.
        ctx.spec.storage.verity = vec![VerityDevice {
            id: "root".to_string(),
            name: "root".to_string(),
            data_device_id: "root-data".to_string(),
            hash_device_id: "root-hash".to_string(),
            ..Default::default()
        }];

        assert_eq!(
            ctx.get_verity_config(&"root".to_owned()).unwrap(),
            VerityDevice {
                id: "root".to_string(),
                name: "root".to_string(),
                data_device_id: "root-data".to_string(),
                hash_device_id: "root-hash".to_string(),
                ..Default::default()
            }
        );

        // Test case #2: Requesting config for a non-existent device should return an error.
        assert_eq!(
            ctx.get_verity_config(&"non-existent".to_owned())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find configuration for verity device 'non-existent'"
        );
    }

    #[test]
    fn test_filesystem_block_device_size() {
        let ctx = EngineContext::default().with_spec(HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "disk1".to_owned(),
                    device: PathBuf::from("/dev/sda"),
                    partitions: vec![Partition {
                        id: "part1".to_owned(),
                        size: 4096.into(),
                        partition_type: PartitionType::Root,
                        uuid: None,
                        label: None,
                    }],
                    ..Default::default()
                }],
                filesystems: vec![FileSystem {
                    device_id: Some("part1".to_owned()),
                    mount_point: Some("/data".into()),
                    source: FileSystemSource::Image,
                    is_esp: false,
                }],
                ..Default::default()
            },
            ..Default::default()
        });

        assert_eq!(ctx.filesystem_block_device_size("/data"), Some(4096));

        assert_eq!(ctx.filesystem_block_device_size("/data/subdir"), Some(4096));

        assert_eq!(ctx.filesystem_block_device_size("/nonexistent"), None);
    }

    #[test]
    fn test_get_first_backing_partition() {
        let ctx = EngineContext {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        partitions: vec![
                            Partition {
                                id: "esp".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("1G").unwrap(),
                                uuid: None,
                                label: None,
                            },
                            Partition {
                                id: "root".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("8G").unwrap(),
                                uuid: None,
                                label: None,
                            },
                            Partition {
                                id: "rootb".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("8G").unwrap(),
                                uuid: None,
                                label: None,
                            },
                        ],
                        ..Default::default()
                    }],
                    raid: Raid {
                        software: vec![SoftwareRaidArray {
                            id: "root-raid1".to_owned(),
                            devices: vec!["root".to_string(), "rootb".to_string()],
                            name: "raid1".to_string(),
                            level: RaidLevel::Raid1,
                        }],
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            ctx.get_first_backing_partition(&"esp".to_owned()).unwrap(),
            &ctx.spec.storage.disks[0].partitions[0]
        );
        assert_eq!(
            ctx.get_first_backing_partition(&"root".to_owned()).unwrap(),
            &ctx.spec.storage.disks[0].partitions[1]
        );
        assert_eq!(
            ctx.get_first_backing_partition(&"rootb".to_owned())
                .unwrap(),
            &ctx.spec.storage.disks[0].partitions[2]
        );
        assert_eq!(
            ctx.get_first_backing_partition(&"root-raid1".to_owned())
                .unwrap(),
            &ctx.spec.storage.disks[0].partitions[1]
        );
        ctx.get_first_backing_partition(&"os".to_owned())
            .unwrap_err();
        ctx.get_first_backing_partition(&"non-existant".to_owned())
            .unwrap_err();
    }

    mod feature_tracing_tests {
        use std::{
            collections::BTreeMap,
            sync::{Arc, Mutex},
        };

        use netplan_types::NetworkConfig;
        use tracing::{field::Visit, Event, Subscriber};
        use tracing_subscriber::{
            layer::{Context, SubscriberExt},
            registry::LookupSpan,
            Layer, Registry,
        };
        use url::Url;

        use sysdefs::tpm2::Pcr;
        use trident_api::{
            config::{
                self, Encryption, Extension, KernelCommandLine, Module, Os, Script, Scripts,
                Selinux, SelinuxMode, Services, SoftwareRaidArray as ConfigSoftwareRaidArray,
                UefiFallbackMode,
            },
            primitives::hash::Sha384Hash,
        };

        use super::*;

        #[derive(Default)]
        struct MetricVisitor {
            fields: BTreeMap<String, String>,
        }

        impl Visit for MetricVisitor {
            fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
                if field.name() != "message" {
                    self.fields
                        .insert(field.name().to_string(), value.to_string());
                }
            }

            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name() != "message" {
                    self.fields
                        .insert(field.name().to_string(), value.to_string());
                }
            }

            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() != "message" {
                    self.fields
                        .insert(field.name().to_string(), format!("{value:?}"));
                }
            }
        }

        #[derive(Clone, Default)]
        struct MetricsCaptureLayer {
            events: Arc<Mutex<BTreeMap<String, String>>>,
        }

        impl<S> Layer<S> for MetricsCaptureLayer
        where
            S: Subscriber + for<'a> LookupSpan<'a>,
        {
            fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
                let mut visitor = MetricVisitor::default();
                event.record(&mut visitor);

                let mut events = self
                    .events
                    .lock()
                    .expect("metric events mutex should not be poisoned");

                for (key, value) in visitor.fields {
                    events.insert(key, value);
                }
            }
        }

        fn trace_feature_metrics(execute: impl FnOnce()) -> BTreeMap<String, String> {
            let layer = MetricsCaptureLayer::default();
            let events = Arc::clone(&layer.events);

            {
                let subscriber = Registry::default().with(layer);
                tracing::subscriber::with_default(subscriber, || {
                    execute();
                });
            }

            let result = events
                .lock()
                .expect("metric events mutex should not be poisoned")
                .clone();
            result
        }

        #[test]
        fn test_feature_tracing_defaults() {
            let ctx = EngineContext::default();
            let metrics = trace_feature_metrics(|| ctx.feature_tracing());

            let expected = BTreeMap::from([
                (
                    "metric_name".to_string(),
                    "host_config_feature_usage".to_string(),
                ),
                ("ab_update".to_string(), "false".to_string()),
                ("confexts".to_string(), "false".to_string()),
                ("encryption".to_string(), "".to_string()),
                ("internal_params".to_string(), "".to_string()),
                (
                    "kernel_command_line_options".to_string(),
                    "false".to_string(),
                ),
                ("modules".to_string(), "false".to_string()),
                ("netplan".to_string(), "false".to_string()),
                ("post_configure_scripts".to_string(), "false".to_string()),
                ("post_provision_scripts".to_string(), "false".to_string()),
                ("pre_servicing_scripts".to_string(), "false".to_string()),
                ("root_verity".to_string(), "false".to_string()),
                ("selinux".to_string(), "none".to_string()),
                ("services_disabled".to_string(), "false".to_string()),
                ("services_enabled".to_string(), "false".to_string()),
                ("software_raid".to_string(), "false".to_string()),
                ("sysexts".to_string(), "false".to_string()),
                ("uefi_fallback_mode".to_string(), "conservative".to_string()),
                ("usr_verity".to_string(), "false".to_string()),
            ]);

            assert_eq!(metrics, expected);
        }

        /// Builds a fully-populated Host Configuration exercising every field
        /// checked by `feature_tracing`.
        fn non_default_host_config() -> HostConfiguration {
            let mut config = HostConfiguration {
                os: Os {
                    selinux: Selinux {
                        mode: Some(SelinuxMode::Enforcing),
                    },
                    modules: vec![Module {
                        name: "loop".to_string(),
                        ..Default::default()
                    }],
                    services: Services {
                        enable: vec!["sshd".to_string()],
                        disable: vec!["debug-shell".to_string()],
                    },
                    kernel_command_line: KernelCommandLine {
                        extra_command_line: vec!["console=ttyS0".to_string()],
                    },
                    uefi_fallback: UefiFallbackMode::Disabled,
                    sysexts: vec![Extension {
                        url: Url::parse("http://example.com/ext1.raw").unwrap(),
                        sha384: Sha384Hash::from("a".repeat(96)),
                        path: None,
                    }],
                    confexts: vec![Extension {
                        url: Url::parse("http://example.com/ext2.raw").unwrap(),
                        sha384: Sha384Hash::from("b".repeat(96)),
                        path: None,
                    }],
                    netplan: Some(NetworkConfig {
                        version: 2,
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                scripts: Scripts {
                    pre_servicing: vec![Script::default()],
                    post_provision: vec![Script::default()],
                    post_configure: vec![Script::default()],
                },
                storage: Storage {
                    encryption: Some(Encryption {
                        pcrs: vec![Pcr::Pcr7, Pcr::Pcr11],
                        ..Default::default()
                    }),
                    ab_update: Some(config::AbUpdate {
                        volume_pairs: vec![],
                    }),
                    raid: config::Raid {
                        software: vec![ConfigSoftwareRaidArray {
                            id: "raid0".into(),
                            name: "md0".to_string(),
                            level: config::RaidLevel::Raid1,
                            devices: vec!["disk-a".into(), "disk-b".into()],
                        }],
                        sync_timeout: None,
                    },
                    verity: vec![
                        config::VerityDevice {
                            id: "usr".into(),
                            name: "usr".to_string(),
                            data_device_id: "usr-data".into(),
                            hash_device_id: "usr-hash".into(),
                            ..Default::default()
                        },
                        config::VerityDevice {
                            id: "root".into(),
                            name: "root".to_string(),
                            data_device_id: "root-data".into(),
                            hash_device_id: "root-hash".into(),
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            };
            config.internal_params.set_flag("preview-feature-flag");
            config
        }

        fn non_default_expected_metrics() -> BTreeMap<String, String> {
            BTreeMap::from([
                (
                    "metric_name".to_string(),
                    "host_config_feature_usage".to_string(),
                ),
                ("ab_update".to_string(), "true".to_string()),
                ("confexts".to_string(), "true".to_string()),
                ("encryption".to_string(), "7,11".to_string()),
                (
                    "internal_params".to_string(),
                    "preview-feature-flag".to_string(),
                ),
                (
                    "kernel_command_line_options".to_string(),
                    "true".to_string(),
                ),
                ("modules".to_string(), "true".to_string()),
                ("netplan".to_string(), "true".to_string()),
                ("post_configure_scripts".to_string(), "true".to_string()),
                ("post_provision_scripts".to_string(), "true".to_string()),
                ("pre_servicing_scripts".to_string(), "true".to_string()),
                ("root_verity".to_string(), "true".to_string()),
                ("selinux".to_string(), "enforcing".to_string()),
                ("services_disabled".to_string(), "true".to_string()),
                ("services_enabled".to_string(), "true".to_string()),
                ("software_raid".to_string(), "true".to_string()),
                ("sysexts".to_string(), "true".to_string()),
                ("uefi_fallback_mode".to_string(), "disabled".to_string()),
                ("usr_verity".to_string(), "true".to_string()),
            ])
        }

        /// A clean install has `spec_old == HostConfiguration::default()`, so
        /// `spec` alone must drive every field.
        #[test]
        fn test_feature_tracing_non_defaults_install() {
            let ctx = EngineContext {
                spec: non_default_host_config(),
                spec_old: HostConfiguration::default(),
                ..Default::default()
            };

            let metrics = trace_feature_metrics(|| ctx.feature_tracing());
            assert_eq!(metrics, non_default_expected_metrics());
        }

        /// An update whose Host Configuration fully restates every field
        /// should behave identically to install: `spec` alone drives the
        /// metric.
        #[test]
        fn test_feature_tracing_non_defaults_update_fully_specified() {
            let ctx = EngineContext {
                spec: non_default_host_config(),
                spec_old: non_default_host_config(),
                ..Default::default()
            };

            let metrics = trace_feature_metrics(|| ctx.feature_tracing());
            assert_eq!(metrics, non_default_expected_metrics());
        }

        /// An update whose Host Configuration only specifies a new `image`
        /// (leaving every other field at its default) must fall back to
        /// `spec_old` for those fields, since they weren't actually reset --
        /// they're simply unspecified in this update's delta. `internal_params`
        /// is the one exception (see next test): it's per-invocation and
        /// intentionally does NOT fall back.
        #[test]
        fn test_feature_tracing_update_falls_back_to_spec_old() {
            let ctx = EngineContext {
                spec: HostConfiguration::default(),
                spec_old: non_default_host_config(),
                ..Default::default()
            };

            let mut expected = non_default_expected_metrics();
            expected.insert("internal_params".to_string(), "".to_string());

            let metrics = trace_feature_metrics(|| ctx.feature_tracing());
            assert_eq!(metrics, expected);
        }

        /// internal_params is per-invocation (e.g. one-off preview flags) and
        /// must NOT fall back to spec_old -- an update that doesn't restate a
        /// preview flag should report it as unset for this invocation.
        #[test]
        fn test_feature_tracing_internal_params_does_not_fall_back() {
            let ctx = EngineContext {
                spec: HostConfiguration::default(),
                spec_old: non_default_host_config(),
                ..Default::default()
            };

            let metrics = trace_feature_metrics(|| ctx.feature_tracing());
            assert_eq!(metrics.get("internal_params").unwrap(), "");
        }
    }
}
