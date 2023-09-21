use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::{BlockDeviceId, PartitionType};

/// HostStatus is the status of a host. Reflects the current provisioning state
/// of the host and any encountered errors.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct HostStatus {
    pub reconcile_state: ReconcileState,
    pub storage: Storage,
    pub imaging: Imaging,
}

/// ReconcileState is the state of the host's reconciliation process. Through
/// the ReconcileState, the Trident agent communicates what operations are in progress.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub enum ReconcileState {
    /// A clean install is in progress.
    CleanInstall,
    /// An update is in progress.
    UpdateInProgress(UpdateKind),
    /// The system is running normally.
    #[default]
    Ready,
}

/// UpdateKind is the kind of update that is in progress.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub enum UpdateKind {
    /// Update that can be applied without pausing the workload.
    HotPatch = 0,
    /// Update that requires pausing the workload.
    NormalUpdate = 1,
    /// Update that requires rebooting the host.
    UpdateAndReboot = 2,
    /// Update that requires switching to a different root partition and rebooting.
    AbUpdate = 3,
    /// Update that cannot be applied given the current state of the system.
    Incompatible = 4,
}

/// Storage status of a host.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct Storage {
    pub disks: BTreeMap<BlockDeviceId, Disk>,
    pub mount_points: BTreeMap<BlockDeviceId, MountPoint>,
}

/// Per disk status.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct Disk {
    pub uuid: Uuid,
    pub path: PathBuf,
    pub capacity: Option<u64>,
    pub partitions: Vec<Partition>,
    pub contents: BlockDeviceContents,
}

/// Per partition status.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct Partition {
    pub id: BlockDeviceId,

    pub path: PathBuf,
    pub start: u64,
    pub end: u64,
    #[serde(rename = "type")]
    pub ty: PartitionType,
    pub contents: BlockDeviceContents,
    pub uuid: Uuid,
}

/// Status of contents of a block device.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub enum BlockDeviceContents {
    #[default]
    Unknown,
    Zeroed,
    Image {
        sha256: String,
        length: u64,
        url: String,
    },
    Initialized,
}

/// Mount point status.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct MountPoint {
    pub path: PathBuf,
    pub filesystem: String,
    pub options: Vec<String>,
}

/// Imaging status of a host.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct Imaging {
    /// A/B update status.
    pub ab_update: Option<AbUpdate>,
    /// Path to the root block device.
    pub root_device_path: Option<PathBuf>,
}

/// A/B update status. Carries information about the A/B update volume pairs and
/// the currently active volume. Note that all pairs will have at any point in
/// time the same volume (A or B) active. The volume to update is determined by
/// the ReconcileState and active_volume.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct AbUpdate {
    /// Map from AB volume pair block device id to the AB volume pair.
    pub volume_pairs: BTreeMap<BlockDeviceId, AbVolumePair>,
    /// Determines which set of volumes are currently active.
    pub active_volume: Option<AbVolumeSelection>,
}

/// A/B volume selection. Determines which set of volumes are currently
/// active/used by the OS.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub enum AbVolumeSelection {
    VolumeA,
    VolumeB,
}

/// Per A/B update volume pair status.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct AbVolumePair {
    pub id: BlockDeviceId,

    pub volume_a_id: BlockDeviceId,
    pub volume_b_id: BlockDeviceId,
}

/// Block device information. Carries information about the block device path
/// and size, used for imaging. Abstracts the difference between specific block
/// device types.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct BlockDeviceInfo {
    pub path: PathBuf,
    pub size: u64,
    pub contents: BlockDeviceContents,
}
