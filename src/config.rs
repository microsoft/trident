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
    pub disk: Disk,

    /// Netplan configuration for the provisioning OS _ONLY_.
    pub network_provision: Option<Value>,

    /// Netplan configuration for the runtime OS.
    pub network: Option<Value>,
}

#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
pub struct Disk {
    /// The path to the disk. For instance, "/dev/sda" or
    /// "/dev/disk/by-path/pci-0000:00:1f.2-ata-1".
    pub device: PathBuf,

    /// The partition to use as the root partition. For instance, "/dev/sda1" or
    /// "/dev/disk/by-path/pci-0000:00:1f.2-ata-1-part1". Trident itself doesn't create this
    /// partition, it is expected to already exist within the disk image.
    ///
    /// Specifically, trident copies the full raw image onto the disk without trying to parse or
    /// understand it. That means that the partition field here is really metadata about the disk
    /// image, rather than a directive to trident on how to partition the disk.
    pub partition: PathBuf,

    /// The URL to download the disk image from. Currently must be a ZStandard compressed raw disk
    /// image.
    pub image_url: String,

    /// The SHA256 of the disk image.
    pub image_sha256: String,
}
