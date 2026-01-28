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

    use strum::IntoEnumIterator;
    use tempfile::TempDir;

    /// An overly-paranoid test to ensure that all variants of ServicingState
    /// are covered in servicing_state_from_datastore function and that the
    /// mapping is correct. It is essentially a re-implementation of the mapping
    /// in the function, but in different form, meaning that any error would
    /// need to be typed twice for this test to pass.
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

        // Track which ServicingState variants have been tested for coverage.
        let mut coverage_assertions = ServicingState::iter().collect::<Vec<_>>();

        for (input, expected) in test_cases {
            // Remove input from coverage assertions
            coverage_assertions.retain(|item| *item != input);

            let temp_dir = TempDir::new().unwrap();
            let mut datastore =
                DataStore::open_or_create(&temp_dir.path().join("datastore.sqlite")).unwrap();
            datastore
                .with_host_status(|hs| hs.servicing_state = input)
                .unwrap();

            let result = servicing_state_from_datastore(&datastore);
            assert_eq!(
                result, expected,
                "Failed for input: {input:?}, result: {result:?}, expected: {expected:?}"
            );
        }

        // Ensure all ServicingState variants were tested, coverage_assertions should be empty.
        assert!(
            coverage_assertions.is_empty(),
            "Not all ServicingState variants were tested: {:?}",
            coverage_assertions
        );
    }
}
