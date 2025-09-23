use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumString};

#[cfg(feature = "schemars")]
use schemars::JsonSchema;

use crate::{constants::DEV_MD_PATH, BlockDeviceId};

#[cfg(feature = "schemars")]
use crate::schema_helpers::{block_device_id_list_schema, block_device_id_schema};

/// RAID configuration for a host.
#[derive(Serialize, Deserialize, Debug, Default, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct Raid {
    /// Individual software RAID configurations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub software: Vec<SoftwareRaidArray>,

    /// Timeout in seconds to wait for RAID arrays to sync.
    ///
    /// By default, Trident will NOT wait for RAID arrays to finish syncing before continuing on
    /// with provisioning. This is because RAID arrays are supposed to be usable immediately after
    /// creation. If the user provides a value for this field and the RAID arrays do NOT finish
    /// syncing within the specified timeout, Trident will fail the provisioning process and return
    /// an error. The user will need to increase their timeout value if the RAID arrays are taking
    /// longer to sync than expected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_timeout: Option<u64>,
}

/// Software RAID configuration.
///
/// The RAID array will be created using the `mdadm` package. During a clean install, all the
/// existing RAID arrays that are on disks defined in the host configuration will be unmounted, and
/// stopped.
///
/// The RAID arrays that are defined in the host configuration will be created, and mounted if
/// specified in `mount-points`.
///
/// To learn more about RAID, please refer to the [RAID wiki](https://wiki.archlinux.org/title/RAID).
///
/// To learn more about `mdadm`, please refer to the [mdadm
/// guide](https://raid.wiki.kernel.org/index.php/A_guide_to_mdadm).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct SoftwareRaidArray {
    /// A unique identifier for the RAID array.
    ///
    /// This is a user-defined string that allows to link the RAID array to the mount points and
    /// also to results in the Host Status. The identifier needs to be unique across all types of
    /// devices, not just RAID arrays.
    #[cfg_attr(feature = "schemars", schemars(schema_with = "block_device_id_schema"))]
    pub id: BlockDeviceId,

    /// Name of the RAID array.
    ///
    /// This is used to reference the RAID array on the system. For example, `some-raid` will
    /// result in `/dev/md/some-raid` on the system.
    pub name: String,

    /// RAID level.
    ///
    /// `raid1` is supported and tested.
    ///
    /// Other possible values yet to be tested are: `raid0`, `raid5`, `raid6`, `raid10`.
    pub level: RaidLevel,

    /// Devices that will be used for the RAID array.
    ///
    /// See the reference links for picking the right number of devices. Devices are partition ids
    /// from the `disks` section.
    #[cfg_attr(
        feature = "schemars",
        schemars(schema_with = "block_device_id_list_schema")
    )]
    pub devices: Vec<BlockDeviceId>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, Hash, Eq, PartialEq, Display, EnumString)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
#[strum(serialize_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub enum RaidLevel {
    /// # Striping
    Raid0,

    /// # Mirroring
    Raid1,

    /// # Striping with parity
    Raid5,

    /// # Striping with double parity
    Raid6,

    /// # Stripe of mirrors
    Raid10,
}

impl SoftwareRaidArray {
    pub fn device_path(&self) -> PathBuf {
        Path::new(DEV_MD_PATH).join(&self.name)
    }
}
