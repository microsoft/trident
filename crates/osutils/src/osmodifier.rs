// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Re-exports from the `osmodifier` crate for backwards compatibility.
//!
//! The types and functions have moved to the standalone `osmodifier` crate.
//! This module re-exports them so that existing `osutils::osmodifier::*`
//! imports continue to work during the migration.

pub use osmodifier::{
    BootConfig, CorruptionOption, IdentifiedPartition, MICPassword, MICUser, OSModifierConfig,
    Overlay, PasswordType, Verity,
};

use serde::Serialize;
use trident_api::config::Services;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MICServices {
    pub services: Services,
}
