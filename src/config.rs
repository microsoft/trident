use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::path::PathBuf;

/// Definition of Trident's full configuration.
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub struct ConfigFile {
    /// Port for the gRPC server.
    /// Default is 50051.
    pub listen_port: Option<u16>,

    /// Optional URL to reach out to when networking is up.
    pub phonehome: Option<String>,

    /// The mode to run in.
    pub mode: Mode,

    /// Netplan configuration to use instead of what is specified in the host config.
    pub network_override: Option<Value>,

    /// The host config to use.
    pub host_config: Option<HostConfigSource>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Provision the network then listen for gRPC requests.
    #[default]
    Listen,

    /// Automatically provision the host based on the config file.
    AutoProvision,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(untagged)]
pub enum HostConfigSource {
    /// Use the host config file.
    File(PathBuf),

    /// Use the host config embedded in the config file.
    Embedded(HostConfig),
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub struct HostConfig {
    /// Netplan configuration for the provisioning OS _ONLY_.
    pub network_provision: Option<Value>,

    /// Netplan configuration for the runtime OS.
    pub network: Option<Value>,
}
