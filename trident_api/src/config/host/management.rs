use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::is_default;

/// The Management configuration controls the installation of the Trident agent onto
/// the runtime OS.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Management {
    /// When set to `true`, prevents Trident from being enabled on the runtime OS.
    /// In that case, the remaining fields are ignored.
    #[serde(default)]
    pub disable: bool,

    /// (FOR DEBUGGING ONLY) a boolean flag that indicates whether Trident should
    /// upgrade itself. If set to `true`, Trident will replicate itself into the
    /// runtime OS prior to transitioning. This is useful during development to
    /// ensure the matching version of Trident is used. Defaults to `false`.
    #[serde(default)]
    pub self_upgrade: bool,

    /// Whether Trident should start a gRPC server to listen for commands when the runtime OS boots.
    /// Defaults to `false`.
    #[serde(default, skip_serializing_if = "is_default")]
    pub enable_grpc: bool,

    /// Describes where to place the datastore Trident will use to store its state.
    /// Defaults to `/var/lib/trident/datastore.sqlite`. Needs to end with
    /// `.sqlite`, cannot be an existing file and cannot reside on a read-only
    /// filesystem or A/B volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datastore_path: Option<PathBuf>,

    /// URL to reach out to when runtime OS networking is up, so Trident can report
    /// its status. If not specified, the value from the Trident configuration will
    /// be used. This is useful for debugging and monitoring purposes, say by an
    /// orchestrator.
    pub phonehome: Option<String>,
}
