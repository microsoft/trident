use std::path::PathBuf;

use trident_api::{
    config::HostConfiguration,
    status::{AbVolumeSelection, ServicingType},
    BlockDeviceId,
};

use crate::osimage::OsImage;

#[cfg_attr(any(test, feature = "functional-test"), derive(Clone, Default))]
pub struct EngineContext {
    pub spec: HostConfiguration,

    pub spec_old: HostConfiguration,

    /// Type of servicing that Trident is executing on the host.
    pub servicing_type: ServicingType,

    /// The path associated with each block device in the Host Configuration.
    pub block_device_paths: std::collections::BTreeMap<BlockDeviceId, PathBuf>,

    /// A/B update status.
    pub ab_active_volume: Option<AbVolumeSelection>,

    /// Stores the Disks UUID to ID mapping of the host.
    pub disks_by_uuid: std::collections::HashMap<uuid::Uuid, BlockDeviceId>,

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
    #[allow(dead_code)]
    pub os_image: Option<OsImage>,
}
impl EngineContext {
    /// Returns the update volume selection for all A/B volume pairs. The update volume is the one
    /// that is meant to be updated, based on the servicing in progress, if any.
    pub fn get_ab_update_volume(&self) -> Option<AbVolumeSelection> {
        match self.servicing_type {
            // If there is no servicing in progress, update volume is None.
            ServicingType::NoActiveServicing => None,
            // If host is executing a "normal" update, active and update volumes are the same.
            ServicingType::HotPatch
            | ServicingType::NormalUpdate
            | ServicingType::UpdateAndReboot => self.ab_active_volume,
            // If host is executing an A/B update, update volume is the opposite of active volume.
            ServicingType::AbUpdate => {
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

    /// Returns a reference to the Partition object within an AB volume pair that corresponds to the
    /// update partition, or the one to be updated.
    #[cfg(feature = "sysupdate")]
    pub fn get_ab_update_volume_partition(
        &self,
        block_device_id: &BlockDeviceId,
    ) -> Option<&trident_api::config::Partition> {
        if let Some(ab_update) = self.spec.storage.ab_update.as_ref() {
            let ab_volume = ab_update
                .volume_pairs
                .iter()
                .find(|v| &v.id == block_device_id);
            if let Some(v) = ab_volume {
                return self
                    .get_ab_update_volume()
                    .and_then(|selection| match selection {
                        AbVolumeSelection::VolumeA => {
                            self.spec.storage.get_partition(&v.volume_a_id)
                        }
                        AbVolumeSelection::VolumeB => {
                            self.spec.storage.get_partition(&v.volume_b_id)
                        }
                    });
            }
        }

        None
    }
}
