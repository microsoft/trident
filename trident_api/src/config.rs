use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use netplan_types::NetworkConfig;

/// Definition of Trident's full configuration.
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct LocalConfigFile {
    /// Configuration for Trident itself.
    #[serde(flatten, default)]
    pub trident_config: TridentConfiguration,

    /// The host config to use.
    #[serde(flatten, default)]
    pub host_config_source: HostConfigurationSource,
}

/// Configuration that Trident needs which doesn't belong in the host config.
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct TridentConfiguration {
    /// Optional URL to reach out to when networking is up.
    pub phonehome: Option<String>,

    /// Optional URL to stream logs to
    pub logstream: Option<String>,

    /// Configuration of the datastore with its location.
    pub datastore: Option<DatastoreConfiguration>,

    /// Netplan configuration to use instead of what is specified in the host config.
    pub network_override: Option<NetworkConfig>,

    /// Defines the operation to perform.
    #[serde(default)]
    pub allowed_operations: Operations,
}

/// Configuration for the datastore.
#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
#[serde(untagged)]
pub enum DatastoreConfiguration {
    /// Directory in the runtime filesystem where to create the datastore during provisioning.
    #[serde(rename_all = "kebab-case")]
    Create { create_path: PathBuf },

    /// Directory in the runtime filesystem where the datastore is already present.
    #[serde(rename_all = "kebab-case")]
    Load { load_path: PathBuf },
}

/// HostConfigurationSource is the source of the host configuration.
#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub enum HostConfigurationSource {
    /// Use the host config file.
    #[serde(rename = "host-configuration-file")]
    File(PathBuf),

    /// Use the host config embedded in the config file.
    #[serde(rename = "host-configuration")]
    Embedded(Box<HostConfiguration>),

    #[serde(rename = "kickstart-file")]
    Kickstart(PathBuf),

    #[serde(rename = "kickstart")]
    KickstartEmbedded(String),

    #[serde(rename = "grpc")]
    GrpcCommand {
        /// Port for the gRPC server (default is 50051)
        listen_port: Option<u16>,
    },
}
impl Default for HostConfigurationSource {
    fn default() -> Self {
        HostConfigurationSource::GrpcCommand { listen_port: None }
    }
}

/// HostConfiguration is the configuration for a host. Trident agent will use this to configure the host.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct HostConfiguration {
    #[serde(default)]
    pub management: Management,

    pub storage: Storage,

    pub imaging: Imaging,

    /// Netplan configuration for the provisioning OS _ONLY_.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_provision: Option<NetworkConfig>,

    /// Netplan configuration for the runtime OS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkConfig>,

    /// Scripts to be run after the installation is complete.
    /// Should reference the name of a script in the `scripts` section.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_install_scripts: Vec<Script>,
}

bitflags::bitflags! {
    #[derive(Serialize, Deserialize, Debug)]
    #[serde(rename_all = "kebab-case")]
    #[serde(deny_unknown_fields)]
    pub struct Operations: u32 {
        /// Reconcile the host configuration with the current state of the host.
        const Update = 0b1;
        /// Restart the machine (either via kexec or a normal reboot) if needed by an update.
        const Transition = 0b10;
    }
}
impl Default for Operations {
    fn default() -> Self {
        Operations::all()
    }
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Management {
    /// Whether to skip installing the agent on the runtime OS.
    #[serde(default)]
    pub disable: bool,

    /// For debugging, copy the agent from the provisioning OS to the runtime OS.
    #[serde(default)]
    pub self_upgrade: bool,

    /// Path to save the datastore, or `None` if the default path should be used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datastore_path: Option<PathBuf>,

    /// Optional URL to reach out to when networking is up.
    pub phonehome: Option<String>,
}

/// Storage configuration for a host.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Storage {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<Disk>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mount_points: Vec<MountPoint>,
}

/// Identifier for a block device.
pub type BlockDeviceId = String;

/// Per disk configuration. Carries information about the disk block volume device, the partition table, and the partitions.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Disk {
    pub id: BlockDeviceId,

    /// The path to the disk. For instance, "/dev/sda" or
    /// "/dev/disk/by-path/pci-0000:00:1f.2-ata-1".
    pub device: PathBuf,

    /// The partition table type, which currently must be GPT.
    pub partition_table_type: PartitionTableType,

    pub partitions: Vec<Partition>,
}

/// Partition table type. Currently only GPT is supported.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub enum PartitionTableType {
    #[default]
    Gpt,
}

/// Per partition configuration. Carries information about the partition type,
/// and the size in bytes.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Partition {
    pub id: BlockDeviceId,

    #[serde(rename = "type")]
    pub partition_type: PartitionType,
    /// Size in bytes.
    pub size: String,
}

/// Partition types as defined by The Discoverable Partitions Specification (https://uapi-group.org/specifications/specs/discoverable_partitions_specification/).
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub enum PartitionType {
    /// EFI System Partition
    /// C12A7328-F81F-11D2-BA4B-00A0C93EC93B
    Esp,
    /// x64: 4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709
    Root,
    /// 0657fd6d-a4ab-43c4-84e5-0933c84b4f4f
    Swap,
    /// x64: 2c7357ed-ebd2-46d9-aec1-23d437ec2bf5
    RootVerity,
    /// 933ac7e1-2eb4-4f13-b844-0e14e2aef915
    Home,
    /// 4d21b016-b534-45c2-a9fb-5c16e091fd2d
    Var,
}

/// Mount point configuration. Carries information necessary to populate
/// /etc/fstab configuration to mount a filesystem on a block device.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct MountPoint {
    pub path: PathBuf,
    pub filesystem: String,
    pub options: Vec<String>,
    pub target_id: BlockDeviceId,
}

/// Imaging configuration for a host. Carries information about the images to
/// deploy onto host block devices and the A/B update configuration.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Imaging {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub ab_update: Option<AbUpdate>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct ImageConfiguration {
    pub image: Image,
    pub mount_point: MountPoint,
    pub target_id: BlockDeviceId,
}

/// Per image configuration. Carries information about the image URL
/// (http(s)://, file://, or oci:// prefixes are supported), the image hash, the
/// image format, and the target block device to deploy the image onto.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct Image {
    pub url: String,
    pub sha256: String,
    pub format: ImageFormat,
    pub target_id: BlockDeviceId,
}

/// Image format. Currently only RawZstd is supported, which represents a raw
/// filesystem image compressed with zstd.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub enum ImageFormat {
    RawZstd,
}

/// A/B update configuration. Carries information about the A/B update volume
/// pairs that are used to perform A/B updates.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct AbUpdate {
    pub volume_pairs: Vec<AbVolumePair>,
}

/// Per A/B update volume pair configuration. Points to the underlying block
/// devices used for the A/B update.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case")]
#[serde(deny_unknown_fields)]
pub struct AbVolumePair {
    pub id: BlockDeviceId,

    pub volume_a_id: BlockDeviceId,
    pub volume_b_id: BlockDeviceId,
}

#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case")]
pub struct Script {
    /// Binary to run the script with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<PathBuf>,

    /// The script itself.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,

    /// Path of a file to write the script's output to.
    /// THis includes both stdout and stderr.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_file_path: Option<PathBuf>,
}
