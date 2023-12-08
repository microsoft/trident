use netplan_types::NetworkConfig;
use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::is_default;

pub(super) mod management;
pub(super) mod network;
pub(super) mod osconfig;
pub(super) mod scripts;
pub(super) mod storage;

use management::Management;
use osconfig::OsConfig;
use scripts::Scripts;
use storage::Storage;

/// HostConfiguration is the configuration for a host. Trident agent will use this to configure the host.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct HostConfiguration {
    /// The Management configuration controls the installation of the Trident agent onto
    /// the runtime OS.
    #[serde(default)]
    pub management: Management,

    /// Describes the storage configuration of the host.
    #[serde(default)]
    pub storage: Storage,

    /// Netplan network configuration for the provisioning OS _ONLY_.
    ///
    /// See [Netplan YAML Configuration](https://netplan.readthedocs.io/en/stable/netplan-yaml/) for more information.
    ///
    /// When provided, this configuration will be used to configure the network
    /// on the provisioning OS. When not provided, the network configuration from
    /// the runtime OS will be used instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "network::schema_helpers::make_placeholder_netplan_schema")
    )]
    pub network_provision: Option<NetworkConfig>,

    /// Netplan network configuration for the runtime OS.
    ///
    /// See [Netplan YAML Configuration](https://netplan.readthedocs.io/en/stable/netplan-yaml/) for more information.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "network::schema_helpers::make_placeholder_netplan_schema")
    )]
    pub network: Option<NetworkConfig>,

    /// Optional scripts to be run after different Trident stages have completed.
    #[serde(default, skip_serializing_if = "is_default")]
    pub scripts: Scripts,

    /// OS Configuration for the runtime OS.
    #[serde(default, skip_serializing_if = "is_default")]
    pub osconfig: OsConfig,
}
