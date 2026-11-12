use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{
    config::{AdoptedPartition, Partition},
    BlockDeviceId,
};

#[cfg(feature = "schemars")]
use crate::schema::block_device_id_schema;

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
    /// Specify exactly one of `device` or `selector`.
    #[serde(default, skip_serializing_if = "crate::is_default")]
    pub device: PathBuf,

    /// Select a disk during installation instead of specifying a device path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<DiskSelector>,

    /// The partition table type of the disk. Supported values are: `gpt`.
    pub partition_table_type: PartitionTableType,

    /// A list of partitions that will be created on the disk.
    pub partitions: Vec<Partition>,

    /// A list of pre-existing partitions that will be adopted from the disk.
    ///
    /// Adopting partitions requires an explicit `device` and cannot be combined
    /// with a disk selector.
    ///
    /// Several options are available to match a partition to adopt. If more
    /// than one option is specified, ALL the provided criteria will be used to
    /// match the partition.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adopted_partitions: Vec<AdoptedPartition>,
}

/// Installation-time disk selection. Existing data does not prevent selection.
///
/// Newly formatted filesystems require fixed partition sizes.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct DiskSelector {
    /// Strategy used to choose an eligible disk large enough for the layout.
    pub strategy: DiskSelectionStrategy,
}

/// Supported disk selection strategies.
#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum DiskSelectionStrategy {
    /// Select the smallest eligible disk that fits the layout.
    #[default]
    SmallestFit,
    /// Apply device-family priority before comparing disk capacity. Check
    /// eligible disks by Linux device-name family, in this order:
    ///
    /// 1. NVMe (`nvme*`)
    ///
    /// 2. SCSI/SATA-style disks (`sd*`)
    ///
    /// 3. Virtio disks (`vd*`)
    ///
    /// 4. Non-removable eMMC user-area disks (`mmcblk*`)
    ///
    /// 5. Legacy IDE-style disks (`hd*`)
    ///
    /// Choose the first family that contains a disk large enough for the
    /// configured storage layout, then select the smallest fitting disk within
    /// that family. Move to the next family only if no eligible disk in the
    /// current family fits.
    ///
    /// Capacity is not compared across families: a fitting NVMe disk is chosen
    /// even if a smaller fitting `sd` disk is available.
    TieredSmallestFit,
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

#[cfg(test)]
mod tests {
    use super::*;

    use indoc::indoc;

    use crate::config::{HostConfigurationStaticValidationError, Storage};

    #[test]
    fn selector_requires_an_explicit_supported_strategy() {
        let disk: Disk = serde_yaml::from_str(indoc! {"
            id: os
            selector:
              strategy: tiered-smallest-fit
            partitionTableType: gpt
            partitions: []
        "})
        .unwrap();
        assert!(disk.device.as_os_str().is_empty());
        assert_eq!(
            disk.selector.as_ref().unwrap().strategy,
            DiskSelectionStrategy::TieredSmallestFit
        );
        let value = serde_yaml::to_value(&disk).unwrap();
        assert!(value.get("device").is_none());
        assert_eq!(value["selector"]["strategy"], "tiered-smallest-fit");
        assert!(serde_yaml::from_str::<DiskSelector>("{}").is_err());
        assert!(serde_yaml::from_str::<DiskSelector>("strategy: largest").is_err());
    }

    #[test]
    fn resolved_disk_preserves_the_existing_serialized_shape() {
        let disk: Disk = serde_yaml::from_str(indoc! {"
            id: os
            device: /dev/disk/by-id/example
            partitionTableType: gpt
            partitions: []
        "})
        .unwrap();
        assert!(disk.selector.is_none());
        let value = serde_yaml::to_value(&disk).unwrap();
        assert_eq!(value["device"], "/dev/disk/by-id/example");
        assert!(value.get("selector").is_none());
    }

    #[test]
    fn disk_requires_exactly_one_target_and_rejects_selector_adoption() {
        let mut disk = Disk {
            id: "os".into(),
            ..Default::default()
        };
        for (device, selector) in [
            (PathBuf::new(), None),
            ("/dev/sda".into(), Some(DiskSelector::default())),
        ] {
            disk.device = device;
            disk.selector = selector;
            let storage = Storage {
                disks: vec![disk.clone()],
                ..Default::default()
            };
            assert!(matches!(
                storage.validate(false),
                Err(HostConfigurationStaticValidationError::InvalidDiskTarget { .. })
            ));
        }

        disk.device = PathBuf::new();
        disk.adopted_partitions.push(AdoptedPartition {
            id: "existing".into(),
            match_label: Some("existing".into()),
            match_uuid: None,
        });
        let storage = Storage {
            disks: vec![disk],
            ..Default::default()
        };
        assert!(matches!(
            storage.validate(false),
            Err(HostConfigurationStaticValidationError::SelectorWithAdoptedPartitions { .. })
        ));
    }

    #[test]
    fn multiple_selectors_do_not_conflict_as_empty_device_paths() {
        let storage = Storage {
            disks: ["first", "second"]
                .into_iter()
                .map(|id| Disk {
                    id: id.into(),
                    selector: Some(DiskSelector::default()),
                    partitions: vec![Partition::new(format!("{id}-partition"), 4096_u64)],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        storage.build_graph().unwrap();
    }
}
