use std::{collections::BTreeMap, path::PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::PartitionType;

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct HostStatus {
    pub reconcile_state: ReconcileState,
    pub storage: Storage,
    pub imaging: Imaging,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub enum ReconcileState {
    /// A clean install is in progress.
    CleanInstall,
    /// An update is in progress.
    UpdateInProgress(UpdateKind),
    /// The system is running normally.
    #[default]
    Ready,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Storage {
    pub disks: BTreeMap<PathBuf, Disk>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Disk {
    pub uuid: Uuid,
    pub bus_path: PathBuf,
    pub capacity: Option<u64>,
    pub partitions: Vec<Partition>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Partition {
    pub path: PathBuf,
    pub start: u64,
    pub end: u64,
    pub ty: PartitionType,
    pub contents: PartitionContents,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub enum PartitionContents {
    #[default]
    Unknown,
    Zeroed,
    Image {
        sha256: String,
        length: u64,
    },
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Imaging {
    /// Map from sha256 to Image.
    pub images: BTreeMap<String, Image>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct Image {
    pub url: String,
}
