use serde::{Deserialize, Serialize};
use serde_yaml::Value;

/// Definition of Trident's full configuration.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct ConfigFile {
    /// This field contains configuration for trident itself
    pub core: CoreConfig,

    /// This field is netplan configuration for the runtime OS.
    pub network: Option<Value>,

    /// This field is netplan configuration for the provisioning OS _ONLY_.
    #[serde(rename = "network-provision")]
    pub network_provision: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct CoreConfig {
    /// Port for the gRPC server.
    /// Default is 50051.
    pub listen_port: Option<u16>,

    /// Optional URL to reach out to when networking is up.
    pub phonehome: Option<String>,
}
