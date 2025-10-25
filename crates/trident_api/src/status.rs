use std::{
    collections::{BTreeMap, HashMap},
    fmt::{Display, Formatter, Result},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;
use uuid::Uuid;

use crate::{config::HostConfiguration, is_default, BlockDeviceId};

/// HostStatus is the status of a host. Reflects the current state of the host and any encountered
/// errors.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostStatus {
    pub spec: HostConfiguration,

    /// If the host is currently in AbUpdateStaged or AbUpdateFinalized state, this holds the
    /// previous Host Configuration, from before the A/B update servicing has started.
    #[serde(default, skip_serializing_if = "is_default")]
    pub spec_old: HostConfiguration,

    /// Current state of the servicing that Trident is executing on the host.
    pub servicing_state: ServicingState,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<serde_yaml::Value>,

    /// The device paths of each partition.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub partition_paths: BTreeMap<BlockDeviceId, PathBuf>,

    /// A/B update status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ab_active_volume: Option<AbVolumeSelection>,

    /// The UUID for each disk.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub disk_uuids: HashMap<BlockDeviceId, Uuid>,

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

    /// Whether this HostStatus is stored on the management OS.
    #[serde(default, skip_serializing_if = "is_default")]
    pub is_management_os: bool,
}

/// Servicing type is the type of servicing that the Trident agent is executing on the host.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ServicingType {
    /// Update that can be applied without pausing the workload.
    HotPatch = 0,
    /// Update that requires pausing the workload.
    NormalUpdate = 1,
    /// Update that requires rebooting the host.
    UpdateAndReboot = 2,
    /// Update that requires switching to a different root partition and rebooting.
    AbUpdate = 3,
    /// Clean install of the target OS image when the host is booted from the provisioning OS.
    CleanInstall = 4,
    /// No servicing is currently in progress.
    #[default]
    NoActiveServicing = 5,
}

/// Servicing state describes the progress of the servicing that the Trident agent is executing on
/// the host. The host will transition through a different sequence of servicing states while
/// servicing the host.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ServicingState {
    /// The host is running from the provisioning OS and has not yet been provisioned by Trident.
    #[default]
    NotProvisioned,
    /// Clean install has been staged, i.e., the initial target OS images have been deployed onto
    /// block devices.
    CleanInstallStaged,
    /// A/B update has been staged. The new target OS images have been deployed onto block devices.
    AbUpdateStaged,
    /// Clean install has been finalized, i.e., UEFI variables have been set, so that firmware boots
    /// from the target OS image after reboot.
    CleanInstallFinalized,
    /// A/B update has been finalized. For the next boot, the firmware will boot from the updated
    /// target OS image.
    AbUpdateFinalized,
    /// Servicing has been completed, and the host successfully booted from the updated target OS
    /// image. Trident is ready to begin a new servicing.
    Provisioned,
    /// A/B update has been completed, the host booted into the target OS but the Health Checks failed.
    AbUpdateHealthCheckFailed,
}

/// A/B volume selection. Determines which set of volumes are currently
/// active/used by the OS.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, EnumIter)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum AbVolumeSelection {
    VolumeA,
    VolumeB,
}

impl Display for AbVolumeSelection {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result {
        match self {
            AbVolumeSelection::VolumeA => write!(f, "Volume A"),
            AbVolumeSelection::VolumeB => write!(f, "Volume B"),
        }
    }
}
