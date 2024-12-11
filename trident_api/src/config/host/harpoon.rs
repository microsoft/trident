use serde::{Deserialize, Serialize};
use url::Url;

use crate::primitives::version::SemverVersion;

/// Configuration for the Harpoon update client.
///
/// Harpoon is an Omaha client that can be used to poll updated Host
/// Configuration documents from an Omaha server.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HarpoonConfig {
    /// HTTP endpoint of the Omaha server.
    pub url: Url,

    /// The App ID of the Omaha application providing updates to this Host Configuration document.
    ///
    /// It is passed to the Omaha server as-is.
    pub app_id: String,

    /// The track or group to which this Host Configuration document belongs.
    ///
    /// This is used by the Omaha server to determine which updates to provide.
    /// It is passed to the server as-is.
    pub track: String,

    /// The version of this Host Configuration document. This is the version
    /// reported to the server when checking for updates.
    ///
    /// It MUST be valid semver. It is passed to the Omaha server as-is.
    pub document_version: SemverVersion,
}
