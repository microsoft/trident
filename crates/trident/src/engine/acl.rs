//! ACL (Azure Container Linux) specific re-exports and helpers.
//!
//! The verity root hash type and utilities live in `osutils::verity_roothash`.
//! ACL PARTUUID constants live in `trident_api::constants`.
//! This module re-exports both for convenience within the engine.

pub use osutils::verity_roothash::VerityRootHash;
pub use trident_api::constants::{ACL_USR_A_PARTUUID, ACL_USR_B_PARTUUID};
