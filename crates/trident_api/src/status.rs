use std::{
    collections::{BTreeMap, HashMap},
    fmt::{self, Display, Formatter},
    path::PathBuf,
};

use anyhow::{bail, Context};
use log::{debug, info};
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use strum_macros::EnumIter;
use uuid::Uuid;

use crate::{config::HostConfiguration, is_default, BlockDeviceId};

/// HostStatus is the status of a host. Reflects the current state of the host and any encountered
/// errors.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostStatus {
    pub spec: HostConfiguration,

    /// If the host is currently in AbUpdateStaged or AbUpdateFinalized state, this holds the
    /// previous Host Configuration, from before the A/B update servicing has started.
    #[serde(default, skip_serializing_if = "is_default")]
    pub spec_old: HostConfiguration,

    /// Current state of the servicing that Trident is executing on the host.
    pub servicing_state: ServicingState,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<serde_yaml::Value>,

    /// The device paths of each partition.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub partition_paths: BTreeMap<BlockDeviceId, PathBuf>,

    /// A/B update status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ab_active_volume: Option<AbVolumeSelection>,

    /// The UUID for each disk.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub disk_uuids: HashMap<BlockDeviceId, Uuid>,

    /// Index of the current Azure Linux install. Used to distinguish between
    /// different installs of Azure Linux on the same host.
    ///
    /// An AzL "install" is the result of a deployment of Azure Linux (e.g. with
    /// Trident), and encompasses the entire deployment, including both A/B
    /// volumes (when present).
    ///
    /// Indexes are assigned sequentially, starting from 0. On a clean install,
    /// Trident will determine the next available index and use it for the new
    /// install.
    pub install_index: usize,

    /// Whether this HostStatus is stored on the management OS.
    #[serde(default, skip_serializing_if = "is_default")]
    pub is_management_os: bool,
}

/// Servicing type is the type of servicing that the Trident agent is executing on the host.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ServicingType {
    /// Update that can be applied without pausing the workload.
    HotPatch = 0,
    /// Update that requires pausing the workload.
    NormalUpdate = 1,
    /// Update that requires rebooting the host.
    UpdateAndReboot = 2,
    /// Update that requires switching to a different root partition and rebooting.
    AbUpdate = 3,
    /// Clean install of the target OS image when the host is booted from the provisioning OS.
    CleanInstall = 4,
    /// No servicing is currently in progress.
    #[default]
    NoActiveServicing = 5,
}

/// Servicing state describes the progress of the servicing that the Trident agent is executing on
/// the host. The host will transition through a different sequence of servicing states while
/// servicing the host.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum ServicingState {
    /// The host is running from the provisioning OS and has not yet been provisioned by Trident.
    #[default]
    NotProvisioned,
    /// Clean install has been staged, i.e., the initial target OS images have been deployed onto
    /// block devices.
    CleanInstallStaged,
    /// A/B update has been staged. The new target OS images have been deployed onto block devices.
    AbUpdateStaged,
    /// Clean install has been finalized, i.e., UEFI variables have been set, so that firmware boots
    /// from the target OS image after reboot.
    CleanInstallFinalized,
    /// A/B update has been finalized. For the next boot, the firmware will boot from the updated
    /// target OS image.
    AbUpdateFinalized,
    /// Servicing has been completed, and the host successfully booted from the updated target OS
    /// image. Trident is ready to begin a new servicing.
    Provisioned,
    /// A/B update has been completed, the host booted into the target OS but the Health Checks failed.
    AbUpdateHealthCheckFailed,
}

/// A/B volume selection. Determines which set of volumes are currently
/// active/used by the OS.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, EnumIter)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum AbVolumeSelection {
    VolumeA,
    VolumeB,
}

impl Display for AbVolumeSelection {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            AbVolumeSelection::VolumeA => write!(f, "Volume A"),
            AbVolumeSelection::VolumeB => write!(f, "Volume B"),
        }
    }
}

fn fix_host_config(yaml: &mut Value) -> Result<(), anyhow::Error> {
    let Value::Mapping(ref mut m) = yaml else {
        bail!("Host config is not a mapping")
    };

    if let Some(Value::Mapping(mut e)) = m.remove("osImage") {
        debug!("Converting 'osImage' host status section to 'image'");
        e.remove("type");
        e.insert("sha384".into(), Value::String("ignored".into()));
        m.insert("image".into(), e.into());
    }

    if let Some(Value::Mapping(ref mut os)) = m.get_mut("os") {
        if let Some(n) = os.remove("network") {
            debug!("Converting 'os.network' host status section to 'os.netplan'");
            os.insert("netplan".into(), n);
        }
    }

    if let Some(Value::Mapping(ref mut t)) = m.get_mut("trident") {
        debug!("Removing deprecated 'trident.selfUpgrade' field from host status");
        t.remove("selfUpgrade");
    }

    if let Some(Value::Mapping(ref mut storage)) = m.get_mut("storage") {
        if let Some(Value::Sequence(ref mut fs_list)) = storage.get_mut("filesystems") {
            for fs in fs_list.iter_mut() {
                if let Value::Mapping(ref mut fs_map) = fs {
                    if let Some(Value::Mapping(s)) = fs_map.remove("source") {
                        if s.get("type") == Some(&Value::String("create".into()))
                            || s.get("type") == Some(&Value::String("new".into()))
                        {
                            bail!(
                                "Cannot convert old host status with 'create' or 'new' filesystem source"
                            );
                        }
                    }

                    fs_map.remove("type");
                }
            }
        }

        let mut extra_verity = Vec::new();
        let mut extra_filesystems = Vec::new();

        if let Some(Value::Sequence(ref mut fs_list)) = storage.remove("verityFilesystems") {
            for fs in fs_list.iter_mut() {
                match fs {
                    Value::Mapping(ref mut fs_map) => {
                        let data_device_id = fs_map.remove("dataDeviceId").context("Cannot convert old host status with verity filesystem missing dataDeviceId")?;
                        let hash_device_id = fs_map.remove("hashDeviceId").context("Cannot convert old host status with verity filesystem missing hashDeviceId")?;
                        let mount_point = fs_map.remove("mountPoint").context("Cannot convert old host status with verity filesystem missing mountPoint")?;
                        let name = fs_map.remove("name").context(
                            "Cannot convert old host status with verity filesystem missing name",
                        )?;

                        let id = format!("verity{}", extra_verity.len());

                        extra_verity.push(Value::Mapping(
                            vec![
                                ("name".into(), name),
                                ("id".into(), Value::String(id.clone())),
                                ("dataDeviceId".into(), data_device_id),
                                ("hashDeviceId".into(), hash_device_id),
                            ]
                            .into_iter()
                            .collect(),
                        ));

                        extra_filesystems.push(Value::Mapping(
                            vec![
                                ("deviceId".into(), Value::String(id)),
                                ("mountPoint".into(), mount_point),
                            ]
                            .into_iter()
                            .collect(),
                        ));
                    }
                    _ => {
                        bail!("Cannot convert old host status with non-mapping verity filesystem");
                    }
                }
            }
        }

        if !extra_verity.is_empty() {
            debug!("Adding converted verity devices to host status");
            storage.insert("verity".into(), Value::Sequence(extra_verity));
        }
        if !extra_filesystems.is_empty() {
            debug!("Adding converted verity filesystems to host status");
            if let Some(Value::Sequence(ref mut fs_list)) = storage.get_mut("filesystems") {
                fs_list.extend(extra_filesystems);
            } else {
                storage.insert("filesystems".into(), Value::Sequence(extra_filesystems));
            }
        }
    }
    Ok(())
}

fn fix_old_host_status(yaml: &mut Value) -> Result<(), anyhow::Error> {
    let Value::Mapping(ref mut m) = yaml else {
        bail!("Host status is not a mapping");
    };

    info!("Attempting to fix old host status format");

    fix_host_config(
        m.get_mut("spec")
            .context("Missing 'spec' field in host status")?,
    )?;

    if let Some(ref mut spec_old @ Value::Mapping(_)) = m.get_mut("specOld") {
        fix_host_config(spec_old).context("Invalid 'specOld' field in host status")?;
    }

    Ok(())
}

pub fn decode_host_status(mut yaml: Value) -> Result<HostStatus, serde_yaml::Error> {
    let decoded = serde_yaml::from_value(yaml.clone());
    if let Ok(host_status) = decoded {
        return Ok(host_status);
    }

    if let Err(e) = fix_old_host_status(&mut yaml) {
        info!("Failed to fixup old Host Status format: {e}");
        return decoded;
    }

    match serde_yaml::from_value(yaml) {
        Ok(hs) => Ok(hs),
        Err(e) => {
            info!("Failed to parse Host Status after fixup: {e}");
            decoded
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_old_format() {
        let old_yaml = include_str!("test/preview12_host_status.yaml");
        let yaml: Value = serde_yaml::from_str(old_yaml).unwrap();

        let hs = decode_host_status(yaml).unwrap();
        hs.spec.validate().unwrap();
    }
}
