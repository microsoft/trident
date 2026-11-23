use std::{
    collections::{BTreeMap, HashMap},
    fmt::{self, Display, Formatter},
    path::PathBuf,
};

use anyhow::{bail, Context};
use log::{debug, info};
use semver::Version;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;
use strum_macros::{AsRefStr, EnumIter, EnumString};
use trident_proto::v1::ServicingKind;
use uuid::Uuid;

use crate::{
    config::HostConfiguration, constants::IMAGE_CHECKSUM_IGNORED, is_default, BlockDeviceId,
};

/// HostStatus is the status of a host. Reflects the current state of the host and any encountered
/// errors.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
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

    /// Version of Trident that last updated this HostStatus with a recognized servicing state.
    #[serde(default, skip_serializing_if = "TridentVersion::is_none")]
    pub trident_version: TridentVersion,
}

/// Servicing type is the type of servicing that the Trident agent is executing on the host.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServicingType {
    /// No servicing is currently in progress.
    #[default]
    NoActiveServicing = 0,
    /// Update that can be applied without necessitating a reboot.
    RuntimeUpdate = 1,
    /// Update that requires switching to a different root partition and rebooting.
    AbUpdate = 2,
    /// Clean install of the target OS image when the host is booted from the provisioning OS.
    CleanInstall = 3,
    /// Manual Rollback of the target OS image into the inactive volume to the previously deployed state.
    ManualRollbackAb = 4,
    /// Manual Rollback of the target OS image within the active volume to the previously deployed state.
    ManualRollbackRuntime = 5,
}

impl From<ServicingType> for trident_proto::v1::ServicingKind {
    fn from(st: ServicingType) -> Self {
        match st {
            ServicingType::NoActiveServicing => ServicingKind::NoneRequired,
            ServicingType::RuntimeUpdate => ServicingKind::RuntimeUpdate,
            ServicingType::AbUpdate => ServicingKind::AbUpdate,
            ServicingType::CleanInstall => ServicingKind::CleanInstall,
            ServicingType::ManualRollbackAb => ServicingKind::ManualRollbackAb,
            ServicingType::ManualRollbackRuntime => ServicingKind::ManualRollbackRuntime,
        }
    }
}

/// Servicing state describes the progress of the servicing that the Trident agent is executing on
/// the host. The host will transition through a different sequence of servicing states while
/// servicing the host.
#[derive(Clone, Debug, Default, Eq, PartialEq, EnumIter, EnumString, AsRefStr)]
#[strum(serialize_all = "kebab-case")]
pub enum ServicingState {
    /// The host is running from the provisioning OS and has not yet been provisioned by Trident.
    #[default]
    NotProvisioned,
    /// Clean install has been staged, i.e., the initial target OS images have been deployed onto
    /// block devices.
    CleanInstallStaged,
    /// A/B update has been staged. The new target OS images have been deployed onto block devices.
    AbUpdateStaged,
    /// Manual rollback for an AbUpdate has been staged.
    ManualRollbackAbStaged,
    /// Manual rollback for a RuntimeUpdate has been staged.
    ManualRollbackRuntimeStaged,
    /// Runtime update has been staged.
    RuntimeUpdateStaged,
    /// Clean install has been finalized, i.e., UEFI variables have been set, so that firmware boots
    /// from the target OS image after reboot.
    CleanInstallFinalized,
    /// A/B update has been finalized. For the next boot, the firmware will boot from the updated
    /// target OS image.
    AbUpdateFinalized,
    /// Manual rollback for an AbUpdate has been finalized.
    ManualRollbackAbFinalized,
    /// Servicing has been completed, and the host successfully booted from the updated target OS
    /// image. Trident is ready to begin a new servicing.
    Provisioned,
    /// A/B update has been completed, the host booted into the target OS but the health checks failed.
    AbUpdateHealthCheckFailed,
    /// Commit failed and rollback to the retained OS is required.
    CommitFailed,
    /// An unfamiliar state name preserved for inspection, not servicing or recovery.
    #[strum(default)]
    Unknown(String),
}

impl Serialize for ServicingState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let name = match self {
            Self::Unknown(name) => name.as_str(),
            known => known.as_ref(),
        };
        serializer.serialize_str(name)
    }
}

/// Reads known names or preserves well-formed unfamiliar names without normalizing them.
/// Writer-version validation requires the enclosing HostStatus and runs in decode_host_status.
/// Non-string YAML values are rejected rather than coerced into state names.
impl<'de> Deserialize<'de> for ServicingState {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct StateVisitor;

        impl<'de> serde::de::Visitor<'de> for StateVisitor {
            type Value = ServicingState;

            fn expecting(&self, formatter: &mut Formatter) -> fmt::Result {
                formatter.write_str("a servicing state string")
            }

            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Self::Value, E> {
                // Future names must follow the persisted naming contract too.
                if !matches!(name.as_bytes().first(), Some(b'a'..=b'z'))
                    || name.split('-').any(|part| {
                        part.is_empty()
                            || !part
                                .bytes()
                                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                    })
                {
                    return Err(E::custom(format!(
                        "invalid servicing state {name:?}: expected a non-empty \
                         lowercase kebab-case name beginning with a letter"
                    )));
                }

                // EnumString recognizes known names; its default variant captures unfamiliar names.
                name.parse().map_err(E::custom)
            }
        }

        // Only visit_str is supported: deserializing String directly could coerce YAML numbers.
        deserializer.deserialize_any(StateVisitor)
    }
}

/// A/B volume selection. Determines which set of volumes are currently
/// active/used by the OS.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, EnumIter)]
#[serde(rename_all = "kebab-case")]
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

/// Trident version
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TridentVersion {
    SemVer(Version),
    Other(String),
    #[default]
    None,
}

impl TridentVersion {
    fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }
}

fn fix_host_config(yaml: &mut Value) -> Result<(), anyhow::Error> {
    let Value::Mapping(ref mut m) = yaml else {
        bail!("Host config is not a mapping")
    };

    if let Some(Value::Mapping(mut e)) = m.remove("osImage") {
        debug!("Converting 'osImage' host status section to 'image'");
        e.remove("type");
        e.insert(
            "sha384".into(),
            Value::String(IMAGE_CHECKSUM_IGNORED.into()),
        );
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

/// Decodes current or legacy Host Status, requiring a strictly newer writer for unknown states.
pub fn decode_host_status(
    yaml: Value,
    reader_version: &Version,
) -> Result<HostStatus, serde_yaml::Error> {
    let host_status = decode_host_status_unchecked(yaml)?;

    if let ServicingState::Unknown(name) = &host_status.servicing_state {
        // A newer writer does not prove integrity: well-formed corruption can still pass.
        let newer_writer = matches!(
            &host_status.trident_version,
            TridentVersion::SemVer(writer_version)
                if writer_version.cmp_precedence(reader_version).is_gt()
        );
        if !newer_writer {
            return Err(<serde_yaml::Error as serde::de::Error>::custom(format!(
                "unknown servicing state {name:?} requires a writer version newer \
                 than {reader_version}; stored version: {:?}",
                host_status.trident_version
            )));
        }
    }

    Ok(host_status)
}

fn decode_host_status_unchecked(mut yaml: Value) -> Result<HostStatus, serde_yaml::Error> {
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
    fn servicing_state_preserves_known_and_unknown_names() {
        let cases = [
            ("not-provisioned", ServicingState::NotProvisioned),
            ("clean-install-staged", ServicingState::CleanInstallStaged),
            ("ab-update-staged", ServicingState::AbUpdateStaged),
            (
                "manual-rollback-ab-staged",
                ServicingState::ManualRollbackAbStaged,
            ),
            (
                "manual-rollback-runtime-staged",
                ServicingState::ManualRollbackRuntimeStaged,
            ),
            ("runtime-update-staged", ServicingState::RuntimeUpdateStaged),
            (
                "clean-install-finalized",
                ServicingState::CleanInstallFinalized,
            ),
            ("ab-update-finalized", ServicingState::AbUpdateFinalized),
            (
                "manual-rollback-ab-finalized",
                ServicingState::ManualRollbackAbFinalized,
            ),
            ("provisioned", ServicingState::Provisioned),
            (
                "ab-update-health-check-failed",
                ServicingState::AbUpdateHealthCheckFailed,
            ),
            ("commit-failed", ServicingState::CommitFailed),
            (
                "future-recovery-step",
                ServicingState::Unknown("future-recovery-step".into()),
            ),
            (
                "ab-update-hypervisor-check-failed",
                ServicingState::Unknown("ab-update-hypervisor-check-failed".into()),
            ),
        ];
        for (name, expected) in cases {
            let yaml = serde_yaml::to_string(name).unwrap();
            let decoded: ServicingState = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(decoded, expected);
            assert_eq!(
                serde_yaml::to_value(&decoded).unwrap(),
                Value::String(name.into())
            );
        }
    }

    #[test]
    fn servicing_state_rejects_non_string_values() {
        for yaml in ["42", "true", "null", "[]", "{}"] {
            serde_yaml::from_str::<ServicingState>(yaml).unwrap_err();
        }
    }

    #[test]
    fn servicing_state_rejects_malformed_names() {
        for name in [
            "",
            "Provisioned",
            "future_state",
            "-future",
            "future-",
            "future--state",
            "future state",
            " future",
            "future\nstate",
            "1future",
            "futur\u{e9}",
        ] {
            let yaml = serde_yaml::to_string(name).unwrap();
            assert!(
                serde_yaml::from_str::<ServicingState>(&yaml).is_err(),
                "{name:?}"
            );
        }
        let yaml = serde_yaml::to_string("future-v2-state").unwrap();
        assert!(serde_yaml::from_str::<ServicingState>(&yaml).is_ok());
    }

    #[test]
    fn unknown_servicing_state_requires_a_newer_writer() {
        let reader = Version::parse("1.2.3").unwrap();
        for (writer, accepted) in [
            (TridentVersion::None, false),
            (TridentVersion::Other("development".into()), false),
            (
                TridentVersion::SemVer(Version::parse("1.2.2").unwrap()),
                false,
            ),
            (TridentVersion::SemVer(reader.clone()), false),
            (
                TridentVersion::SemVer(Version::parse("1.2.3+other").unwrap()),
                false,
            ),
            (
                TridentVersion::SemVer(Version::parse("1.2.3-rc.1").unwrap()),
                false,
            ),
            (
                TridentVersion::SemVer(Version::parse("1.2.4").unwrap()),
                true,
            ),
        ] {
            let mut status = HostStatus {
                servicing_state: ServicingState::Unknown("future-recovery-step".into()),
                trident_version: writer.clone(),
                ..Default::default()
            };
            let result = decode_host_status(serde_yaml::to_value(&status).unwrap(), &reader);
            assert_eq!(result.is_ok(), accepted, "{writer:?}");
            if let Ok(decoded) = result {
                assert_eq!(decoded, status);
            }
            status.servicing_state = ServicingState::Provisioned;
            assert_eq!(
                decode_host_status(serde_yaml::to_value(&status).unwrap(), &reader).unwrap(),
                status,
            );
        }
    }

    #[test]
    fn newer_writer_does_not_establish_unknown_state_integrity() {
        let status = HostStatus {
            servicing_state: ServicingState::Unknown("provisione".into()),
            trident_version: TridentVersion::SemVer(Version::new(2, 0, 0)),
            ..Default::default()
        };
        assert_eq!(
            decode_host_status(
                serde_yaml::to_value(&status).unwrap(),
                &Version::new(1, 0, 0),
            )
            .unwrap(),
            status,
        );
    }

    #[test]
    fn load_old_format() {
        let old_yaml = include_str!("test/preview12_host_status.yaml");
        let yaml: Value = serde_yaml::from_str(old_yaml).unwrap();

        let hs = decode_host_status(yaml, &Version::new(1, 0, 0)).unwrap();
        hs.spec.validate().unwrap();
    }

    #[test]
    fn check_trident_version_serde() {
        // Check missing TridentVersion
        let hs = HostStatus {
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&hs).unwrap();
        assert!(!yaml.contains("tridentVersion"));
        let hs_dserialized: HostStatus = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(hs, hs_dserialized);
        assert!(hs_dserialized.trident_version.is_none());

        // Check semver TridentVersion
        let semver_version = TridentVersion::SemVer(Version::parse("1.2.3").unwrap());
        let hs = HostStatus {
            trident_version: semver_version.clone(),
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&hs).unwrap();
        assert!(!yaml.contains("!sem-ver"));
        assert!(yaml.contains("tridentVersion: 1.2.3"));
        let hs_dserialized: HostStatus = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(hs, hs_dserialized);
        assert_eq!(hs_dserialized.trident_version, semver_version);

        // Check other TridentVersion
        let other_version = TridentVersion::Other("foo".to_string());
        let hs = HostStatus {
            trident_version: other_version.clone(),
            ..Default::default()
        };
        let yaml = serde_yaml::to_string(&hs).unwrap();
        assert!(!yaml.contains("!other"));
        assert!(yaml.contains("tridentVersion: foo"));
        let hs_dserialized: HostStatus = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(hs, hs_dserialized);
        assert_eq!(hs_dserialized.trident_version, other_version);
    }
}
