use harpoon::ServicingState as HarpoonServicingState;
use trident_api::status::ServicingState;

use crate::DataStore;

/// Maps the servicing state from the internal datastore representation to the Harpoon API representation.
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

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn test_servicing_state_from_datastore() {
        let test_cases = vec![
            (
                ServicingState::NotProvisioned,
                HarpoonServicingState::NotProvisioned,
            ),
            (
                ServicingState::CleanInstallStaged,
                HarpoonServicingState::InstallStaged,
            ),
            (
                ServicingState::AbUpdateStaged,
                HarpoonServicingState::UpdateAbStaged,
            ),
            (
                ServicingState::ManualRollbackAbStaged,
                HarpoonServicingState::ManualRollbackAbStaged,
            ),
            (
                ServicingState::ManualRollbackRuntimeStaged,
                HarpoonServicingState::ManualRollbackRuntimeStaged,
            ),
            (
                ServicingState::RuntimeUpdateStaged,
                HarpoonServicingState::UpdateRuntimeStaged,
            ),
            (
                ServicingState::CleanInstallFinalized,
                HarpoonServicingState::InstallFinalized,
            ),
            (
                ServicingState::AbUpdateFinalized,
                HarpoonServicingState::UpdateAbFinalized,
            ),
            (
                ServicingState::ManualRollbackAbFinalized,
                HarpoonServicingState::ManualRollbackAbFinalized,
            ),
            (
                ServicingState::Provisioned,
                HarpoonServicingState::Provisioned,
            ),
            (
                ServicingState::AbUpdateHealthCheckFailed,
                HarpoonServicingState::UpdateAbHealthCheckFailed,
            ),
        ];

        for (input, expected) in test_cases {
            let temp_dir = TempDir::new().unwrap();
            let mut datastore =
                DataStore::open_or_create(&temp_dir.path().join("datastore.sqlite")).unwrap();
            datastore
                .with_host_status(|hs| hs.servicing_state = input)
                .unwrap();

            let result = servicing_state_from_datastore(&datastore);
            assert_eq!(
                result, expected,
                "Failed for input: {:?}, result: {:?}, expected: {:?}",
                input, result, expected
            );
        }
    }
}
