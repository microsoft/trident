use harpoon::ServicingState as HarpoonServicingState;
use trident_api::status::ServicingState;

use crate::DataStore;

pub(super) fn servicing_state_from_datastore(datastore: &DataStore) -> HarpoonServicingState {
    match datastore.host_status().servicing_state {
        ServicingState::NotProvisioned => HarpoonServicingState::NotProvisioned,
        ServicingState::CleanInstallStaged => HarpoonServicingState::InstallStaged,
        ServicingState::AbUpdateStaged => HarpoonServicingState::UpdateAbStaged,
        ServicingState::ManualRollbackAbStaged => HarpoonServicingState::ManualRollbackAbStaged,
        ServicingState::ManualRollbackRuntimeStaged => {
            HarpoonServicingState::ManualRollbackRuntimeStaged
        }
        ServicingState::RuntimeUpdateStaged => HarpoonServicingState::UpdateRuntimeStaged,
        ServicingState::CleanInstallFinalized => HarpoonServicingState::InstallFinalized,
        ServicingState::AbUpdateFinalized => HarpoonServicingState::UpdateAbFinalized,
        ServicingState::ManualRollbackAbFinalized => {
            HarpoonServicingState::ManualRollbackAbFinalized
        }
        ServicingState::Provisioned => HarpoonServicingState::Provisioned,
        ServicingState::AbUpdateHealthCheckFailed => {
            HarpoonServicingState::UpdateAbHealthCheckFailed
        }
    }
}
