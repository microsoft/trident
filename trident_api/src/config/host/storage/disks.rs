use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{
    config::{AdoptedPartition, Partition},
    BlockDeviceId,
};

#[cfg(feature = "schemars")]
use crate::schema_helpers::block_device_id_schema;

/// Per disk configuration.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Disk {
    /// A unique identifier for the disk. This is a user defined string that
    /// allows to link the disk to what is consuming it and also to results in the
    /// Host Status. The identifier needs to be unique across all types of
    /// devices, not just disks.
    ///
    /// TBD: At the moment, the partition table is created from scratch. In the
    /// future, it will be possible to consume an existing partition table.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// The device path of the disk. Points to the disk device in the host. It is
    /// recommended to use stable paths, such as the ones under `/dev/disk/by-path/`
    /// or [WWNs](https://en.wikipedia.org/wiki/World_Wide_Name).
    pub device: PathBuf,

    /// The partition table type of the disk. Supported values are: `gpt`.
    pub partition_table_type: PartitionTableType,

    /// A list of partitions that will be created on the disk.
    pub partitions: Vec<Partition>,

    /// A list of pre-existing partitions that will be adopted from the disk.
    ///
    /// Several options are available to match a partition to adopt. If more
    /// than one option is specified, ALL the provided criteria will be used to
    /// match the partition.
    #[serde(default)]
    pub adopted_partitions: Vec<AdoptedPartition>,
}

/// Partition table type. Currently only GPT is supported.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum PartitionTableType {
    /// # GPT
    ///
    /// Disk should be formatted with a GUID Partition Table (GPT).
    #[default]
    Gpt,
}
