use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

use netplan_types::NetworkConfig;

/// Definition of Trident's full configuration.
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub struct LocalConfigFile {
    /// Optional URL to reach out to when networking is up.
    pub phonehome: Option<String>,

    /// Directory containing the datastore, or None during initial provisioning.
    pub datastore: Option<PathBuf>,

    /// Netplan configuration to use instead of what is specified in the host config.
    pub network_override: Option<NetworkConfig>,

    /// The host config to use.
    #[serde(flatten, default)]
    pub host_config_source: HostConfigSource,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum HostConfigSource {
    /// Use the host config file.
    #[serde(rename = "host-config-file")]
    File(PathBuf),

    /// Use the host config embedded in the config file.
    #[serde(rename = "host-config")]
    Embedded(Box<HostConfiguration>),

    #[serde(rename = "grpc")]
    GrpcCommand {
        /// Port for the gRPC server (default is 50051)
        listen_port: Option<u16>,
    },
}
impl Default for HostConfigSource {
    fn default() -> Self {
        HostConfigSource::GrpcCommand { listen_port: None }
    }
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct HostConfiguration {
    pub storage: Storage,

    pub imaging: Imaging,

    /// Netplan configuration for the provisioning OS _ONLY_.
    pub network_provision: Option<NetworkConfig>,

    /// Netplan configuration for the runtime OS.
    pub network: Option<NetworkConfig>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Storage {
    pub disks: Vec<Disk>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Disk {
    /// The path to the disk. For instance, "/dev/sda" or
    /// "/dev/disk/by-path/pci-0000:00:1f.2-ata-1".
    pub device: PathBuf,

    /// The partition table type, which currently must be GPT.
    pub partition_table_type: PartitionTableType,

    pub partitions: Vec<Partition>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub enum PartitionTableType {
    #[default]
    Gpt,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Partition {
    #[serde(rename = "type")]
    pub partition_type: PartitionType,
    pub size: String,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub enum PartitionType {
    Esp,
    RootA,
    RootB,
    Raid,
}
impl PartitionType {
    pub fn to_label_str(&self) -> &'static str {
        match self {
            PartitionType::Esp => "mariner-esp",
            PartitionType::RootA => "mariner-root-a",
            PartitionType::RootB => "mariner-root-b",
            PartitionType::Raid => "mariner-raid",
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Imaging {
    pub images: HashMap<PartImageType, Image>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub url: String,
    pub sha256: String,
    pub format: ImageFormat,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub enum ImageFormat {
    RawZstd,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct PartImage {
    #[serde(rename = "type")]
    pub ty: PartImageType,
    pub uuid: String,
}

#[derive(Serialize, Deserialize, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub enum PartImageType {
    Esp,
    Root,
}
impl PartImageType {
    pub fn to_part_type(&self, use_a: bool) -> PartitionType {
        if use_a {
            match self {
                PartImageType::Esp => PartitionType::Esp,
                PartImageType::Root => PartitionType::RootA,
            }
        } else {
            match self {
                PartImageType::Esp => PartitionType::Esp,
                PartImageType::Root => PartitionType::RootB,
            }
        }
    }
}
