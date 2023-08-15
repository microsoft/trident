use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use std::{collections::BTreeMap, path::PathBuf};

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
    #[serde(flatten)]
    pub host_config: HostConfigSource,
    //pub host_config: HostConfig,
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

#[derive(Serialize, Deserialize, Debug, Default)]
//#[serde(rename_all = "kebab-case")]
//#[serde(tag = "host-config-source")]
pub enum HostConfigSource {
    /// Use the host config file.
    #[serde(rename = "host-config-file")]
    File(PathBuf),

    /// Use the host config embedded in the config file.
    #[serde(rename = "host-config")]
    Embedded(HostConfig),

    #[default]
    NoHostConfig,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub struct HostConfig {
    pub storage: Storage,

    pub imaging: Imaging,

    /// Netplan configuration for the provisioning OS _ONLY_.
    pub network_provision: Option<Value>,

    /// Netplan configuration for the runtime OS.
    pub network: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Storage {
    pub disks: Vec<Disk>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
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
pub enum PartitionTableType {
    #[default]
    Gpt,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct Partition {
    #[serde(rename = "type")]
    pub partition_type: PartitionType,
    pub size: String,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
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
pub struct Imaging {
    pub images: Vec<Image>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct Image {
    pub url: String,
    pub sha256: String,
    pub kind: ImageKind,
    pub parts: BTreeMap<PartImageType, String>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum ImageKind {
    RawZstd,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
pub struct PartImage {
    #[serde(rename = "type")]
    pub ty: PartImageType,
    pub uuid: String,
}

#[derive(Serialize, Deserialize, Debug, Hash, Eq, PartialEq, Ord, PartialOrd)]
#[serde(rename_all = "kebab-case")]
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
