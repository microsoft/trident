use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};

use netplan_types::NetworkConfig;

use strum_macros::{Display, EnumString};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

#[cfg(feature = "schemars")]
use crate::schema_helpers::{
    block_device_id_list_schema, block_device_id_schema, make_placeholder_netplan_schema,
};

use crate::is_default;
/// Definition of Trident's full configuration.
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct TridentConfiguration {
    /// Optional URL to reach out to when networking is up, so Trident
    /// can report its status. This is useful for debugging and monitoring purposes,
    /// say by an orchestrator. Note that separately the updates to the Host Status
    /// can be monitored, once gRPC support is implemented. TODO: document the
    /// interface, for reference in the meantime.
    pub phonehome: Option<String>,

    /// Optional URL to stream logs to. TODO: document the interface.
    pub logstream: Option<String>,

    /// If present, indicates the path to an existing datastore Trident
    /// should load its state from. This field should not be included when Trident is
    /// running from the provisioning OS.
    pub datastore: Option<DatastoreConfiguration>,

    /// Optional netplan network configuration for the bootstrap OS. If
    // not specified, the network configuration from Host Configuration
    // will be used otherwise.
    pub network_override: Option<NetworkConfig>,

    /// A combination of flags representing allowed operations. This is a
    /// list of operations that Trident is allowed to perform on the host.
    ///
    /// You can pass multiple flags, separated by `|`. Example: `Update | Transition`.
    /// You can pass `''` to disable all operations, which would result in getting
    /// refreshed Host Status, but no operations performed on the host.
    #[serde(default)]
    pub allowed_operations: Operations,
}

/// Configuration for the datastore.
#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
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
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum HostConfigurationSource {
    /// Path to the host configuration file. This is a YAML file that describes the
    /// host configuration in the Host Configuration format.
    #[serde(rename = "host-configuration-file")]
    File(PathBuf),

    /// Describes the host configuration. This is the configuration that Trident
    /// will apply to the host (same payload as `host-configuration-file`, but
    /// directly embedded in the Trident configuration)
    #[serde(rename = "host-configuration")]
    Embedded(Box<HostConfiguration>),

    /// Path to the kickstart file. This is a kickstart file that describes the host
    /// configuration in the kickstart format. WIP, early preview only. TODO:
    /// document what is supported.
    #[serde(rename = "kickstart-file")]
    Kickstart(PathBuf),

    /// Describes the host configuration in the kickstart format. This is the
    /// configuration that Trident will apply to the host (same payload as
    /// `kickstart-file`, but directly embedded in the Trident configuration). WIP,
    /// early preview only.
    #[serde(rename = "kickstart")]
    KickstartEmbedded(String),

    /// Start a gRPC server for remote configuration. Not yet implemented.
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
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct HostConfiguration {
    /// The Management configuration controls the installation of the Trident agent onto
    /// the runtime OS.
    #[serde(default)]
    pub management: Management,

    /// Describes the storage configuration of the host.
    pub storage: Storage,

    /// Filesystem imaging configuration of the host.
    pub imaging: Imaging,

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
        schemars(schema_with = "make_placeholder_netplan_schema")
    )]
    pub network_provision: Option<NetworkConfig>,

    /// Netplan network configuration for the runtime OS.
    ///
    /// See [Netplan YAML Configuration](https://netplan.readthedocs.io/en/stable/netplan-yaml/) for more information.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "make_placeholder_netplan_schema")
    )]
    pub network: Option<NetworkConfig>,

    /// Scripts to be run after the installation is complete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub post_install_scripts: Vec<Script>,

    /// OS Configuration
    #[serde(default, skip_serializing_if = "is_default")]
    pub osconfig: OsConfig,
}

bitflags::bitflags! {
    #[derive(Serialize, Deserialize, Debug)]
    #[serde(rename_all = "kebab-case", deny_unknown_fields)]
    pub struct Operations: u32 {
        /// Trident will update the host based on the host configuration,
        /// but it will not transition the host to the new configuration. This is useful
        /// if you want to drive additional operations on the host outside of Trident.
        const Update = 0b1;
        /// Trident will transition the host to the new configuration,
        /// which can include rebooting the host. This will only happen if `Update` is
        /// also specified.
        const Transition = 0b10;
    }
}
impl Default for Operations {
    fn default() -> Self {
        Operations::all()
    }
}

/// The Management configuration controls the installation of the Trident agent onto
/// the runtime OS.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
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

/// Storage configuration for a host.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Storage {
    /// Per disk configuration.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub disks: Vec<Disk>,

    /// RAID configuration.
    #[serde(default, skip_serializing_if = "is_default")]
    pub raid: RaidConfig,

    /// Mount point configuration.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mount_points: Vec<MountPoint>,
}

/// Identifier for a block device.
pub type BlockDeviceId = String;

/// Per disk configuration.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Disk {
    /// A unique identifier for the disk. This is a user defined string that
    /// allows to link the disk to what is consuming it and also to results in the
    /// Host Status.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The device path of the disk. Points to the disk device in the host. It is
    /// recommended to use stable paths, such as the ones under `/dev/disk/by-path/`
    /// or [WWNs](https://en.wikipedia.org/wiki/World_Wide_Name).
    pub device: PathBuf,

    /// The partition table type of the disk.
    pub partition_table_type: PartitionTableType,

    /// A list of partitions that will be created on the disk.
    pub partitions: Vec<Partition>,
}

/// Partition table type. Currently only GPT is supported.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum PartitionTableType {
    /// # GPT
    ///
    /// Disk should be formatted with a GUID Partition Table (GPT).
    #[default]
    Gpt,
}

/// Per partition configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Partition {
    /// A unique identifier for the partition.
    ///
    /// This is a user defined string that
    /// allows to link the partition to the mount points and also to results in the
    /// Host Status.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The type of the partition.
    ///
    /// As defined by the [Discoverable Partitions Specification](https://uapi-group.org/specifications/specs/discoverable_partitions_specification/).
    #[serde(rename = "type")]
    pub partition_type: PartitionType,

    /// Size of the partition.
    ///
    /// Format: String `<number>[<unit>]`
    ///
    /// Accepted values:
    ///
    /// - `grow`: Use all available space.
    ///
    /// - A number with optional unit suffixes: K, M, G, T (to the base of 1024),
    ///   bytes by default when no unit is specified.
    ///
    /// Examples:
    ///
    /// - `1G`
    ///
    /// - `200M`
    ///
    /// - `grow`
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub size: PartitionSize,
}

/// Partition size enum.
/// Serialize and Deserialize traits are implemented manually in the crate::serde module.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum PartitionSize {
    /// # Grow
    ///
    /// Grow a partition to use all available space.
    ///
    /// String equivalent is defined in constants::PARTITION_SIZE_GROW
    Grow,

    /// # Fixed
    ///
    /// Fixed size in bytes.
    Fixed(u64),
    // Not implemented yet but left as a reference for the future.
    // Min(u64),
    // Max(u64),
    // MinMax(u64, u64),
}

/// Partition types as defined by The Discoverable Partitions Specification (https://uapi-group.org/specifications/specs/discoverable_partitions_specification/).
#[derive(Serialize, Deserialize, Copy, Clone, Debug, Hash, Eq, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum PartitionType {
    /// # EFI System Partition
    ///
    /// `C12A7328-F81F-11D2-BA4B-00A0C93EC93B`
    Esp,

    /// # Root partition
    ///
    /// x64: `4F68BCE3-E8CD-4DB1-96E7-FBCAF984B709`
    Root,

    /// # Swap partition
    ///
    /// `0657fd6d-a4ab-43c4-84e5-0933c84b4f4f`
    Swap,

    /// # Root partition with dm-verity enabled
    ///
    /// x64: `2c7357ed-ebd2-46d9-aec1-23d437ec2bf5`
    RootVerity,

    /// # Home partition
    ///
    /// `933ac7e1-2eb4-4f13-b844-0e14e2aef915`
    Home,

    /// # Var partition
    ///
    /// `4d21b016-b534-45c2-a9fb-5c16e091fd2d`
    Var,

    /// # Usr partition
    ///
    /// x64: `8484680c-9521-48c6-9c11-b0720656f69e`
    Usr,

    /// # Tmp partition
    ///
    /// `7ec6f557-3bc5-4aca-b293-16ef5df639d1`
    Tmp,

    /// # Generic Linux partition
    ///
    /// `0fc63daf-8483-4772-8e79-3d69d8477de4`
    LinuxGeneric,
}

impl PartitionType {
    /// Helper function that returns PartititionType as a string. Return values
    /// are based on GPT partition type identifiers, as defined in the Type
    /// section of systemd repart.d manual:
    /// https://www.man7.org/linux/man-pages/man5/repart.d.5.html.
    pub fn to_sdrepart_part_type(&self) -> &str {
        match self {
            PartitionType::Esp => "esp",
            PartitionType::Root => "root",
            PartitionType::Swap => "swap",
            PartitionType::RootVerity => "root-verity",
            PartitionType::Home => "home",
            PartitionType::Var => "var",
            PartitionType::Usr => "usr",
            PartitionType::Tmp => "tmp",
            PartitionType::LinuxGeneric => "linux-generic",
        }
    }
}

/// RAID configuration for a host.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct RaidConfig {
    /// Individual software raid configurations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub software: Vec<SoftwareRaidArray>,
}

// Software RAID configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct SoftwareRaidArray {
    /// A unique identifier for the RAID array.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// Name of the RAID array. This will be used for creation
    pub name: String,

    /// RAID level. Such as RAID0, RAID1, RAID5, RAID6, RAID10.
    pub level: RaidLevel,

    /// Devices that will be used for the RAID array.
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "block_device_id_list_schema")
    )]
    pub devices: Vec<BlockDeviceId>,

    /// Superblock version. Such as 0.9, 1.0
    pub metadata_version: String,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Hash, Eq, PartialEq, Display, EnumString)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum RaidLevel {
    /// # Striping
    #[strum(serialize = "0")]
    Raid0,

    /// # Mirroring
    #[strum(serialize = "1")]
    Raid1,

    /// # Striping with parity
    #[strum(serialize = "5")]
    Raid5,

    /// # Striping with double parity
    #[strum(serialize = "6")]
    Raid6,

    /// # Stripe of mirrors
    #[strum(serialize = "10")]
    Raid10,
}

/// Mount point configuration. Carries information necessary to populate
/// /etc/fstab configuration to mount a filesystem on a block device.
///
/// The resulting `/etc/fstab` is produced as follows:
///
/// - For each mount point, a line is added to the `/etc/fstab` file, if the `path`
///   does not already exist in the `/etc/fstab` supplied in the runtime OS image.
///   If the `path` already exists in the `/etc/fstab` supplied in the runtime OS,
///   it will be updated to match the configuration provided in the Host
///   Configuration mount points.
/// - If a mount point is not present in the Host Configuration, but present in the
///   `/etc/fstab`, the line will be preserved as is in the `/etc/fstab`.
///
/// Note that you do not need to specify the mounts points, if your runtime OS
/// `/etc/fstab` carries the correct configuration already. In this case, Trident
/// will not modify the `/etc/fstab` file nor will it format the partitions.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct MountPoint {
    /// The path of the mount point.
    ///
    /// This is the path where the volume will be mounted in the runtime OS.
    /// For `swap` partitions, the path should be `none`.
    pub path: PathBuf,

    /// The filesystem to be used for this mount point.
    ///
    /// This value will be used to format the partition.
    pub filesystem: String,

    /// A list of options to be used for this mount point.
    ///
    /// These will be passed as is to the `/etc/fstab` file.
    pub options: Vec<String>,

    /// The ID of the partition that will be mounted at this mount point.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub target_id: BlockDeviceId,
}

/// Imaging configuration for a host.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Imaging {
    /// Per image configuration
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,

    /// A/B update configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ab_update: Option<AbUpdate>,
}

/// Per image configuration.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Image {
    /// The URL of the image.
    ///
    /// Supported schemes are: `file`, `http`, `https`.
    pub url: String,

    /// The SHA256 checksum of the image.
    ///
    /// This is used to verify the integrity of the image.
    /// The checksum is a 64 character hexadecimal string.
    pub sha256: String,

    /// The format of the image.
    pub format: ImageFormat,

    /// The ID of the partition that will be used to store the image.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub target_id: BlockDeviceId,
}

/// Image format.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum ImageFormat {
    /// # Raw Zstd Compressed
    ///
    /// Raw filesystem image with zstd compression.
    RawZstd,

    /// Raw filesystem image with lzma compression, required by
    /// systemd-sysupdate.
    RawLzma,
}

/// A/B update configuration. Carries information about the A/B update volume
/// pairs that are used to perform A/B updates.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct AbUpdate {
    /// A list of volume pairs that will be used for A/B Update.
    ///
    /// You can target the A/B Update volume pair from the `images` and
    /// `mount-points` and Trident will pick the right volume to use based on
    /// the A/B Update state of the host.
    pub volume_pairs: Vec<AbVolumePair>,
}

/// Per A/B update volume pair configuration. Points to the underlying block
/// devices used for the A/B update.
///
/// **Under development, initial logic for illustration purposes only.**
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct AbVolumePair {
    /// A unique identifier for the volume pair.
    ///
    /// This is a user defined string that allows to link the volume pair
    /// to the results in the Host Status and to the mount points.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The ID of the partition that will be used as the A volume.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub volume_a_id: BlockDeviceId,

    /// The ID of the partition that will be used as the B volume.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub volume_b_id: BlockDeviceId,
}

/// A script to be run by Trident at a specific stage.
#[derive(Serialize, Deserialize, Debug, Default, Clone)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Script {
    /// Binary to run the script with.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interpreter: Option<PathBuf>,

    /// The contents of the script.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub content: String,

    /// Path of a file to write the script's output to.
    ///
    /// This includes both stdout and stderr. The path and file
    /// will be created if they don't exist. If the file already
    /// exists, it will be truncated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_file_path: Option<PathBuf>,
}

/// Configuration for the host OS.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct OsConfig {
    /// # Users
    ///
    /// Map of users to configure on the host. The key is the username.
    #[serde(default)]
    pub users: HashMap<String, User>,
}

/// Configuration for a specific user.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct User {
    /// Password configuration.
    #[serde(default, skip_serializing_if = "is_default")]
    pub password: Password,

    /// List of groups to add the user to. **(IN DEVELOPMENT)**
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,

    /// List of SSH keys to add to the user's authorized keys. **(IN DEVELOPMENT)**
    #[serde(default)]
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ssh_keys: Vec<String>,

    /// SSH configuration for the user. **(IN DEVELOPMENT)**
    #[serde(default)]
    #[serde(skip_serializing_if = "is_default")]
    pub ssh_mode: SshMode,
}

/// Password configuration for a user.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(
    rename_all = "kebab-case",
    deny_unknown_fields,
    tag = "mode",
    content = "value"
)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum Password {
    /// # [DEFAULT] Locked Password
    ///
    /// Lock the user's password. (equivalent to `passwd -l`)
    #[default]
    Locked,

    /// # Plaintext Password
    ///
    /// Set the user's password to a plaintext value.
    DangerousPlainText(String),

    /// # Hashed Password
    ///
    /// Set the user's password to a hashed value.
    DangerousHashed(String),
}

#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum SshMode {
    /// # [DEFAULT] Blocked
    ///
    /// Disable SSH for this entity.
    #[default]
    Block,

    /// # Key Only
    ///
    /// Enable SSH for this entity with KEY only.
    KeyOnly,

    /// # Key and Password
    ///
    /// Enable SSH for this entity with KEY and PASSWORD.
    DangerousAllowPassword,
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Test that validates that to_sdrepart_part_type() returns the correct string for each
    /// PartitionType.
    #[test]
    fn test_to_sdrepart_part_type() {
        assert_eq!(PartitionType::Esp.to_sdrepart_part_type(), "esp");
        assert_eq!(PartitionType::Home.to_sdrepart_part_type(), "home");
        assert_eq!(
            PartitionType::LinuxGeneric.to_sdrepart_part_type(),
            "linux-generic"
        );
        assert_eq!(PartitionType::Root.to_sdrepart_part_type(), "root");
        assert_eq!(
            PartitionType::RootVerity.to_sdrepart_part_type(),
            "root-verity"
        );
        assert_eq!(PartitionType::Swap.to_sdrepart_part_type(), "swap");
        assert_eq!(PartitionType::Tmp.to_sdrepart_part_type(), "tmp");
        assert_eq!(PartitionType::Usr.to_sdrepart_part_type(), "usr");
        assert_eq!(PartitionType::Var.to_sdrepart_part_type(), "var");
    }
}
