use trident_api::{config::ImageSha384, primitives::hash::Sha384Hash};

use crate::DataStore;

/// Returns the stored image hash from the datastore, if it exists.
pub(super) fn stored_image_hash(datastore: &DataStore) -> Option<Sha384Hash> {
    match datastore
        .host_status()
        .spec
        .image
        .as_ref()
        .map(|image| &image.sha384)
    {
        Some(ImageSha384::Checksum(hash)) => Some(hash.clone()),
        _ => None,
    }
}

// Re-export preview functions for convenience and keeping the path stable once they graduate to stable.
#[cfg(feature = "grpc-preview")]
pub(super) use preview::servicing_state_from_datastore;

#[cfg(feature = "grpc-preview")]
mod preview {
    use super::*;

    use trident_api::status::ServicingState;
    use trident_proto::v1preview::ServicingState as ProtoServicingState;

    /// Maps the servicing state from the internal datastore representation to the Proto API representation.
    pub fn servicing_state_from_datastore(datastore: &DataStore) -> ProtoServicingState {
        match datastore.host_status().servicing_state {
            ServicingState::NotProvisioned => ProtoServicingState::NotProvisioned,
            ServicingState::CleanInstallStaged => ProtoServicingState::InstallStaged,
            ServicingState::AbUpdateStaged => ProtoServicingState::UpdateAbStaged,
            ServicingState::ManualRollbackAbStaged => ProtoServicingState::ManualRollbackAbStaged,
            ServicingState::ManualRollbackRuntimeStaged => {
                ProtoServicingState::ManualRollbackRuntimeStaged
            }
            ServicingState::RuntimeUpdateStaged => ProtoServicingState::UpdateRuntimeStaged,
            ServicingState::CleanInstallFinalized => ProtoServicingState::InstallFinalized,
            ServicingState::AbUpdateFinalized => ProtoServicingState::UpdateAbFinalized,
            ServicingState::ManualRollbackAbFinalized => {
                ProtoServicingState::ManualRollbackAbFinalized
            }
            ServicingState::Provisioned => ProtoServicingState::Provisioned,
            ServicingState::AbUpdateHealthCheckFailed => {
                ProtoServicingState::UpdateAbHealthCheckFailed
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
                    ProtoServicingState::NotProvisioned,
                ),
                (
                    ServicingState::CleanInstallStaged,
                    ProtoServicingState::InstallStaged,
                ),
                (
                    ServicingState::AbUpdateStaged,
                    ProtoServicingState::UpdateAbStaged,
                ),
                (
                    ServicingState::ManualRollbackAbStaged,
                    ProtoServicingState::ManualRollbackAbStaged,
                ),
                (
                    ServicingState::ManualRollbackRuntimeStaged,
                    ProtoServicingState::ManualRollbackRuntimeStaged,
                ),
                (
                    ServicingState::RuntimeUpdateStaged,
                    ProtoServicingState::UpdateRuntimeStaged,
                ),
                (
                    ServicingState::CleanInstallFinalized,
                    ProtoServicingState::InstallFinalized,
                ),
                (
                    ServicingState::AbUpdateFinalized,
                    ProtoServicingState::UpdateAbFinalized,
                ),
                (
                    ServicingState::ManualRollbackAbFinalized,
                    ProtoServicingState::ManualRollbackAbFinalized,
                ),
                (
                    ServicingState::Provisioned,
                    ProtoServicingState::Provisioned,
                ),
                (
                    ServicingState::AbUpdateHealthCheckFailed,
                    ProtoServicingState::UpdateAbHealthCheckFailed,
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
}
