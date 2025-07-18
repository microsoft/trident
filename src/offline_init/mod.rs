#![allow(unused)]

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{bail, Error};
use log::{debug, info, trace};

use maplit::hashmap;
use osutils::lsblk;
use trident_api::{
    config::{
        AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, HostConfiguration,
        MountOptions, MountPoint, Partition, PartitionSize, PartitionTableType, PartitionType,
        VerityCorruptionOption, VerityDevice,
    },
    constants::internal_params::ENABLE_UKI_SUPPORT,
    error::{
        ExecutionEnvironmentMisconfigurationError, InitializationError, InvalidInputError,
        ReportError, TridentError, TridentResultExt,
    },
    status::{AbVolumeSelection, HostStatus, ServicingState},
    BlockDeviceId,
};
use uuid::Uuid;

use crate::datastore::DataStore;

#[derive(Clone, Debug, serde::Deserialize)]
struct PrismPartition {
    id: BlockDeviceId,
    #[allow(unused)]
    start: String,
    #[allow(unused)]
    size: Option<String>,

    #[serde(rename = "type")]
    ty: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct PrismDisk {
    partitions: Vec<PrismPartition>,
}

#[derive(Debug, serde::Deserialize)]
struct PrismMountPoint {
    path: String,
    #[serde(default)]
    options: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismFileSystem {
    device_id: BlockDeviceId,
    #[serde(rename = "type")]
    ty: String,
    mount_point: Option<PrismMountPoint>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismVerity {
    id: BlockDeviceId,
    name: String,
    data_device_id: BlockDeviceId,
    hash_device_id: BlockDeviceId,
    corruption_option: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismStorage {
    #[serde(default)]
    disks: Vec<PrismDisk>,

    #[serde(default)]
    filesystems: Vec<PrismFileSystem>,

    #[serde(default)]
    verity: Vec<PrismVerity>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismOs {
    #[serde(default)]
    uki: serde_json::Value,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrismHistoryConfig {
    storage: Option<PrismStorage>,
    os: Option<PrismOs>,
    preview_features: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
struct PrismHistoryEntry {
    config: PrismHistoryConfig,
}

#[derive(Clone, Debug)]
struct LazyPartitionInfo {
    name: String,
    part_uuid: String,
    a_partition: PrismPartition,
}

fn generate_host_status(
    history: &[PrismHistoryEntry],
    mut lsblk_output: Vec<lsblk::BlockDevice>,
    lazy_partitions: &[String],
) -> Result<HostStatus, TridentError> {
    let Some(prism_storage) = history
        .iter()
        .rev()
        .map(|entry| entry.config.storage.as_ref())
        .find(|storage| storage.is_some_and(|s| !s.disks.is_empty()))
        .flatten()
    else {
        return Err(TridentError::new(InvalidInputError::ParsePrismHistory))
            .message("Prism history doesn't contain any storage information");
    };

    // Get the partitions declared in the Prism history, this will not include any
    // lazy partitions.
    let prism_partitions = &prism_storage
        .disks
        .first()
        .structured(InvalidInputError::ParsePrismHistory)
        .message("Prism history doesn't contain any disks")?
        .partitions;

    // Validate lazy partitions and create map
    let lazy_partitions = parse_lazy_partitions(lazy_partitions, prism_partitions)
        .structured(InvalidInputError::InvalidLazyPartition)?;

    let mut host_config = HostConfiguration::default();

    let partitions = prism_partitions
        .iter()
        .map(|partition| {
            create_partition(
                partition.id.clone(),
                partition.ty.clone(),
                partition.size.clone(),
            )
        })
        .chain(
            lazy_partitions
                .iter()
                .map(|(lazy_partition_b, lazy_partition_info)| {
                    create_partition(
                        lazy_partition_b.to_string(),
                        lazy_partition_info.a_partition.ty.clone(),
                        lazy_partition_info.a_partition.size.clone(),
                    )
                }),
        )
        .collect::<Result<Vec<_>, _>>()?;

    host_config.storage.disks.push(Disk {
        id: "disk0".to_string(),
        device: "/dev/sda".into(),
        partition_table_type: PartitionTableType::Gpt,
        partitions,
        adopted_partitions: Vec::new(),
    });

    let mut ab_volumes = Vec::new();
    if !prism_storage.verity.is_empty() {
        if prism_storage.verity.len() != 1 {
            return Err(TridentError::new(InvalidInputError::ParsePrismHistory))
                .message("Prism history contains more than one verity device");
        }
        let prism_verity = &prism_storage.verity[0];

        let (data_device_id, hash_device_id) = match (
            prism_verity.data_device_id.strip_suffix("-a"),
            prism_verity.hash_device_id.strip_suffix("-a"),
        ) {
            (Some(data_id), Some(hash_id)) => {
                ab_volumes.push(AbVolumePair {
                    id: data_id.to_string(),
                    volume_a_id: format!("{data_id}-a"),
                    volume_b_id: format!("{data_id}-b"),
                });
                ab_volumes.push(AbVolumePair {
                    id: hash_id.to_string(),
                    volume_a_id: format!("{hash_id}-a"),
                    volume_b_id: format!("{hash_id}-b"),
                });
                (data_id.to_string(), hash_id.to_string())
            }
            (None, None) => (
                prism_verity.data_device_id.clone(),
                prism_verity.hash_device_id.clone(),
            ),
            _ => {
                return Err(TridentError::new(InvalidInputError::ParsePrismHistory))
                    .message("Verity device must use A/B for both data and hash, or neither");
            }
        };

        host_config.storage.verity = vec![VerityDevice {
            id: prism_verity.id.clone(),
            name: prism_verity.name.clone(),
            data_device_id,
            hash_device_id,
            corruption_option: match prism_verity.corruption_option.as_deref() {
                None => VerityCorruptionOption::default(),
                Some("io-error") => VerityCorruptionOption::IoError,
                Some("ignore") => VerityCorruptionOption::Ignore,
                Some("panic") => VerityCorruptionOption::Panic,
                Some("restart") => VerityCorruptionOption::Restart,
                Some(v) => {
                    return Err(TridentError::new(InvalidInputError::ParsePrismHistory))
                        .message(format!("Unknown corruption option: {v}",))
                }
            },
        }];
    }

    // Search the output devices to find the one containing a child mounted at '/'. Since we are
    // running inside Prism, this will be a loop device such as /dev/loop29.
    let lsblk_device = lsblk_output
        .iter_mut()
        .find(|d| {
            d.children
                .iter()
                .filter_map(|p| p.mountpoint.as_ref())
                .any(|m| m == Path::new("/"))
        })
        .structured(ExecutionEnvironmentMisconfigurationError::PrismChrootEnvironment)
        .message("Failed to find root device in lsblk output")?;

    let disk_uuid = lsblk_device
        .ptuuid
        .clone()
        .and_then(|ptuuid| ptuuid.as_uuid())
        .structured(ExecutionEnvironmentMisconfigurationError::PrismChrootEnvironment)
        .message("No UUID found for root device")?;

    lsblk_device.children.sort_by_key(|p| p.partn);

    for (i, part) in lsblk_device.children.iter().enumerate() {
        if part.part_uuid.is_none() {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::PrismChrootEnvironment,
            ))
            .message(format!("No part UUID found for partition {}", i + 1));
        }
    }

    // Get partition paths created from combining Prism history and lsblk output.
    let partition_paths = lsblk_device
        .children
        .iter()
        .zip(prism_partitions.iter())
        .map(|(s, p)| {
            (
                p.id.clone(),
                PathBuf::from(format!(
                    "/dev/disk/by-partuuid/{}",
                    s.part_uuid.as_ref().unwrap_or(&"TODO".into())
                )),
            )
        })
        .chain(
            // Add lazy partitions to the partition paths, if they were provided.
            lazy_partitions
                .iter()
                .map(|(lazy_partition_b, lazy_partition_info)| {
                    (
                        lazy_partition_b.to_string(),
                        PathBuf::from(format!(
                            "/dev/disk/by-partuuid/{}",
                            lazy_partition_info.part_uuid
                        )),
                    )
                }),
        )
        .collect();

    for filesystem in &prism_storage.filesystems {
        let Some(mount_point) = &filesystem.mount_point else {
            continue;
        };

        let device_id = match filesystem.device_id.strip_suffix("-a") {
            Some(device_id) => {
                ab_volumes.push(AbVolumePair {
                    id: device_id.to_string(),
                    volume_a_id: format!("{device_id}-a"),
                    volume_b_id: format!("{device_id}-b"),
                });
                device_id.to_string()
            }
            None => filesystem.device_id.clone(),
        };

        host_config.storage.filesystems.push(FileSystem {
            device_id: Some(device_id),
            source: FileSystemSource::Image,
            mount_point: Some(MountPoint {
                path: PathBuf::from(&mount_point.path),
                options: match &*mount_point.options {
                    "" => MountOptions::defaults(),
                    options => MountOptions::new(options),
                },
            }),
        })
    }

    if !ab_volumes.is_empty() {
        host_config.storage.ab_update = Some(AbUpdate {
            volume_pairs: ab_volumes,
        });
    }

    let preview_features: HashSet<_> = history
        .iter()
        .filter_map(|h| h.config.preview_features.as_ref())
        .flat_map(|f| f.iter().cloned())
        .collect();

    if history
        .iter()
        .filter_map(|h| h.config.os.as_ref())
        .any(|os| !os.uki.is_null())
        || preview_features.contains("uki")
    {
        host_config
            .internal_params
            .set_flag(ENABLE_UKI_SUPPORT.into());
    }

    Ok(HostStatus {
        spec: host_config,
        disk_uuids: hashmap!["disk0".to_string() => disk_uuid],
        partition_paths,
        servicing_state: ServicingState::Provisioned,
        ab_active_volume: Some(AbVolumeSelection::VolumeA),
        install_index: 0,
        is_management_os: false,
        ..Default::default()
    })
}

fn create_partition(
    id: String,
    ty: Option<String>,
    size: Option<String>,
) -> Result<Partition, TridentError> {
    Ok(Partition {
        id: id.clone(),
        partition_type: if ty.as_deref() == Some("esp") {
            PartitionType::Esp
        } else {
            PartitionType::LinuxGeneric
        },
        size: match &size {
            Some(s) => PartitionSize::from_str(s)
                .structured(InvalidInputError::ParsePrismHistory)
                .message(format!("Failed to parse partition size '{s}'"))?,
            None => PartitionSize::Grow,
        },
    })
}

fn parse_lazy_partitions(
    lazy_partitions: &[String],
    prism_history_partitions: &[PrismPartition],
) -> Result<HashMap<String, LazyPartitionInfo>, Error> {
    let mut lazy_partitions_map = HashMap::new();
    for partition in lazy_partitions {
        // Ensure that provided input is in the form xxx:yyy
        match partition.split_once(':') {
            Some((name, uuid)) => {
                if name.is_empty() || uuid.is_empty() {
                    bail!("Lazy partitions must be provided as <b-partition-name>:<b-partition-partuuid> pairs");
                }
                // Ensure that the second part is a valid UUID
                if let Err(err) = Uuid::parse_str(uuid) {
                    bail!("Invalid UUID format: {uuid}: {err}");
                }
                // Ensure that the partition name ends with '-b'
                if !name.ends_with("-b") {
                    bail!("Lazy partitions must end with '-b'");
                }
                // Ensure that there is a corresponding '-a' partition
                let corresponding_a_partition = name.replace("-b", "-a");
                match prism_history_partitions
                    .iter()
                    .find(|p| p.id == *corresponding_a_partition)
                {
                    Some(a_partition) => {
                        lazy_partitions_map.insert(
                            name.to_string(),
                            LazyPartitionInfo {
                                name: name.to_string(),
                                part_uuid: uuid.to_string(),
                                a_partition: a_partition.clone(),
                            },
                        );
                    }
                    None => {
                        bail!("No corresponding '-a' partition found for lazy partition '{name}'");
                    }
                };
            }
            None => {
                bail!("Lazy partitions must be provided as colon-separated <b-partition-name>:<b-partition-partuuid> pairs");
            }
        }
    }
    Ok(lazy_partitions_map)
}

/// Given a path to a Host Status file, initializes the datastore with the Host Status.
/// This command can be executed offline in a chroot environment as part of MIC image customization.
pub fn execute(hs_path: Option<&Path>, lazy_partitions: &[String]) -> Result<(), TridentError> {
    let host_status: HostStatus = if let Some(hs_path) = hs_path {
        info!("Reading Host Status from {:?}", hs_path);
        let host_status_yaml = fs::read_to_string(hs_path)
            .structured(InitializationError::LoadHostStatus)
            .message(format!("Failed to read Host Status from {hs_path:?}"))?;
        let mut host_status: HostStatus = serde_yaml::from_str(&host_status_yaml)
            .structured(InitializationError::ParseHostStatus)
            .message("Failed to parse Host Status from YAML")?;
        host_status
            .spec
            .internal_params
            .set_flag("injectedHostStatus".into());
        host_status
    } else {
        let history_file_path = "/usr/share/image-customizer/history.json";
        let history_file = fs::read_to_string(history_file_path)
            .structured(InvalidInputError::ReadInputFile {
                path: history_file_path.to_string(),
            })
            .message("Failed to read Prism history file")?;

        trace!("Prism history contents:\n{history_file}");

        // TODO: Don't hardcode /dev/sda
        let disk_path = Path::new("/dev/sda");
        if !disk_path.exists() {
            return Err(TridentError::new(
                ExecutionEnvironmentMisconfigurationError::PrismChrootEnvironment,
            ))
            .message("Prism chroot environment doesn't contain /dev/sda");
        }

        let history: Vec<PrismHistoryEntry> =
            serde_json::from_str(&history_file).structured(InvalidInputError::ParsePrismHistory)?;

        let lsblk_output = lsblk::list()
            .structured(ExecutionEnvironmentMisconfigurationError::PrismChrootEnvironment)
            .message("Failed to run lsblk")?;

        generate_host_status(&history, lsblk_output, lazy_partitions)?
    };

    debug!(
        "host_status:\n{}",
        serde_yaml::to_string(&host_status).unwrap_or("Failed to serialize Host Status".into())
    );

    host_status
        .spec
        .validate()
        .map_err(Into::into)
        .message("The provided Host Status has an invalid Host Configuration")?;

    let datastore_path = host_status.spec.trident.datastore_path.clone();

    let mut datastore =
        DataStore::open_or_create(&datastore_path).message("Failed to open temporary datastore")?;
    datastore
        .with_host_status(|hs| *hs = host_status)
        .message("Failed to set new Host Status")?;

    datastore.persist(&datastore_path).message(format!(
        "Failed to persist Host Status to datastore at {datastore_path:?}"
    ))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use osutils::lsblk::LsBlkOutput;

    use super::*;

    // lsblk.json was adding a postCustomization step to the
    // usr-verity configuration in test-images
    // (test-images/platform-integration-images/trident-vm-testimage/base/baseimg-usr-verity.yaml)
    // like this:
    //      scripts:
    //          postCustomization:
    //             - path: lsblk.sh
    // the script does this:
    //     `lsblk --json --output-all --bytes`
    // and running:
    //     `make trident-vm-usr-verity-testimage`
    // the output from this postCusomization step was then
    // captured and copied into lsblk.json.
    const LSBLK: &str = include_str!("lsblk.json");
    // prism_history.json was created from the usr-verity
    // configuration in test-images
    // (test-images/platform-integration-images/trident-vm-testimage/base/baseimg-usr-verity.yaml)
    // by running:
    //     `make trident-vm-usr-verity-testimage`
    // and then untar'ing and unzstd'ing the resulting
    // COSI file, and then extracting the history.json
    // from the /usr partition in share/image-customizer/history.json.
    const PRISM_HISTORY: &str = include_str!("prism_history.json");
    // lazy_prism_history.json was created by deleting the '-b'
    // partitions from prism_history.json
    const LAZY_PRISM_HISTORY: &str = include_str!("lazy_prism_history.json");
    // lazy_lsblk.json was created by finding the 'b' parition
    // partuuids in prism_history.json and removing them from the
    // lsblk.json file.
    const LAZY_LSBLK: &str = include_str!("lazy_lsblk.json");

    #[test]
    fn test_parse_prism_history() {
        let history: Vec<PrismHistoryEntry> =
            serde_json::from_str(PRISM_HISTORY).expect("Failed to parse Prism history");
        assert_eq!(history.len(), 1);
        let entry = &history[0];
        assert_eq!(entry.config.storage.as_ref().unwrap().disks.len(), 1);
        let disk = &entry.config.storage.as_ref().unwrap().disks[0];
        assert_eq!(disk.partitions.len(), 12);
        assert_eq!(disk.partitions[0].id, "esp");
        assert_eq!(disk.partitions[1].id, "boot-a");
        assert_eq!(disk.partitions[2].id, "boot-b");

        let _history2: Vec<PrismHistoryEntry> =
            serde_json::from_str(include_str!("aksee_prism_history.json")).unwrap();
    }

    #[test]
    fn test_generate_host_status() {
        let history: Vec<PrismHistoryEntry> =
            serde_json::from_str(PRISM_HISTORY).expect("Failed to parse Prism history");
        let lsblk_output: LsBlkOutput =
            serde_json::from_str(LSBLK).expect("Failed to parse lsblk output");

        let host_status = generate_host_status(&history, lsblk_output.blockdevices, &[]).unwrap();
        print!(
            "host_status:\n{}",
            serde_yaml::to_string(&host_status).unwrap_or("Failed to serialize Host Status".into())
        );

        assert_eq!(host_status.spec.storage.disks.len(), 1);
        assert_eq!(host_status.spec.storage.filesystems.len(), 7);
        assert_eq!(host_status.spec.storage.verity.len(), 1);

        assert!(host_status.partition_paths.contains_key("boot-a"));
        assert!(host_status.partition_paths.contains_key("boot-b"));
        assert!(host_status.partition_paths.contains_key("esp"));
        assert!(host_status.partition_paths.contains_key("home"));
        assert!(host_status.partition_paths.contains_key("root-a"));
        assert!(host_status.partition_paths.contains_key("root-b"));
        assert!(host_status.partition_paths.contains_key("srv"));
        assert!(host_status.partition_paths.contains_key("trident"));
        assert!(host_status.partition_paths.contains_key("usr-a"));
        assert!(host_status.partition_paths.contains_key("usr-b"));
        assert!(host_status.partition_paths.contains_key("usr-hash-a"));
        assert!(host_status.partition_paths.contains_key("usr-hash-b"));
        assert_eq!(host_status.partition_paths.len(), 12);
    }

    #[test]
    fn test_generate_host_status_with_lazy_partitions() {
        let history: Vec<PrismHistoryEntry> =
            serde_json::from_str(LAZY_PRISM_HISTORY).expect("Failed to parse Prism history");
        let lsblk_output: LsBlkOutput =
            serde_json::from_str(LAZY_LSBLK).expect("Failed to parse lsblk output");

        // Validate that the '-b' partitions are not present in the history
        let host_status_without_lazy_command_line_overrides =
            generate_host_status(&history, lsblk_output.clone().blockdevices, &[]).unwrap();
        print!(
            "host_status_without_lazy_command_line_overrides:\n{}",
            serde_yaml::to_string(&host_status_without_lazy_command_line_overrides)
                .unwrap_or("Failed to serialize Host Status".into())
        );
        assert_eq!(
            host_status_without_lazy_command_line_overrides
                .partition_paths
                .len(),
            8
        );

        // Validate that the '-b' partitions provided by the command line are
        // applied by generate_host_status.
        let boot_b_uuid = "6d792d45-30bc-4764-a3f0-e1c1c8eadbad";
        let root_b_uuid = "05cdb533-2132-4cf9-8802-cb69a71f0c2a";
        let usr_b_uuid = "95f66080-cb1c-400c-8841-06f7fd8380a1";
        let usr_hash_b_uuid = "0b76cf98-d6f7-478c-a93c-d6f912c9b0bd";
        let lazy_partitions = vec![
            format!("boot-b:{boot_b_uuid}"),
            format!("root-b:{root_b_uuid}"),
            format!("usr-b:{usr_b_uuid}"),
            format!("usr-hash-b:{usr_hash_b_uuid}"),
        ];
        let host_status = generate_host_status(
            &history,
            lsblk_output.clone().blockdevices,
            &lazy_partitions,
        )
        .unwrap();
        print!(
            "host_status:\n{}",
            serde_yaml::to_string(&host_status).unwrap_or("Failed to serialize Host Status".into())
        );

        assert_eq!(host_status.spec.storage.disks.len(), 1);
        assert_eq!(host_status.spec.storage.filesystems.len(), 7);
        assert_eq!(host_status.spec.storage.verity.len(), 1);

        // Check that all partitions are present in PartitionPaths
        assert!(host_status.partition_paths.contains_key("boot-a"));
        assert!(host_status.partition_paths.contains_key("boot-b"));
        assert!(host_status.partition_paths.contains_key("esp"));
        assert!(host_status.partition_paths.contains_key("home"));
        assert!(host_status.partition_paths.contains_key("root-a"));
        assert!(host_status.partition_paths.contains_key("root-b"));
        assert!(host_status.partition_paths.contains_key("srv"));
        assert!(host_status.partition_paths.contains_key("trident"));
        assert!(host_status.partition_paths.contains_key("usr-a"));
        assert!(host_status.partition_paths.contains_key("usr-b"));
        assert!(host_status.partition_paths.contains_key("usr-hash-a"));
        assert!(host_status.partition_paths.contains_key("usr-hash-b"));
        // Check that all partition uuids are as expected for lazy partition overridees
        assert_eq!(
            host_status.partition_paths.get("boot-b").unwrap(),
            &PathBuf::from(format!("/dev/disk/by-partuuid/{boot_b_uuid}"))
        );
        assert_eq!(
            host_status.partition_paths.get("root-b").unwrap(),
            &PathBuf::from(format!("/dev/disk/by-partuuid/{root_b_uuid}"))
        );
        assert_eq!(
            host_status.partition_paths.get("usr-b").unwrap(),
            &PathBuf::from(format!("/dev/disk/by-partuuid/{usr_b_uuid}"))
        );
        assert_eq!(
            host_status.partition_paths.get("usr-hash-b").unwrap(),
            &PathBuf::from(format!("/dev/disk/by-partuuid/{usr_hash_b_uuid}"))
        );
        assert_eq!(host_status.partition_paths.len(), 12);
    }

    #[test]
    fn test_parse_lazy_partitions_with_bad_lazy_partitions() {
        let history: Vec<PrismHistoryEntry> =
            serde_json::from_str(LAZY_PRISM_HISTORY).expect("Failed to parse Prism history");

        let prism_partitions = &history
            .iter()
            .rev()
            .map(|entry| entry.config.storage.as_ref())
            .find(|storage| storage.is_some_and(|s| !s.disks.is_empty()))
            .flatten()
            .unwrap()
            .disks
            .first()
            .unwrap()
            .partitions;

        // Not colon-separated
        assert_eq!(
            parse_lazy_partitions(&["no-colon-in-string".to_string()], prism_partitions)
                .unwrap_err()
                .to_string(),
            "Lazy partitions must be provided as colon-separated <b-partition-name>:<b-partition-partuuid> pairs"
        );

        // no partition name
        assert_eq!(
            parse_lazy_partitions(
                &[":6d792d45-30bc-4764-a3f0-e1c1c8eadbad".to_string()],
                prism_partitions,
            )
            .unwrap_err()
            .to_string(),
            "Lazy partitions must be provided as <b-partition-name>:<b-partition-partuuid> pairs"
        );

        // no partition uuid
        assert_eq!(
            parse_lazy_partitions(&["foo-b:".to_string()], prism_partitions)
                .unwrap_err()
                .to_string(),
            "Lazy partitions must be provided as <b-partition-name>:<b-partition-partuuid> pairs"
        );

        // invalid partition uuid
        assert_eq!(
            parse_lazy_partitions(&["foo-b:asd".to_string()], prism_partitions)
                .unwrap_err()
                .to_string(),
            "Invalid UUID format: asd: invalid character: expected an optional prefix of `urn:uuid:` followed by [0-9a-fA-F-], found `s` at 2"
        );

        // partition doesn't end in '-b'
        assert_eq!(
            parse_lazy_partitions(
                &["no_dash_b:6d792d45-30bc-4764-a3f0-e1c1c8eadbad".to_string()],
                prism_partitions,
            )
            .unwrap_err()
            .to_string(),
            "Lazy partitions must end with '-b'"
        );

        // no corresponding '-a' partition
        assert_eq!(
            parse_lazy_partitions(
                &["foo-b:6d792d45-30bc-4764-a3f0-e1c1c8eadbad".to_string()],
                prism_partitions,
            )
            .unwrap_err()
            .to_string(),
            "No corresponding '-a' partition found for lazy partition 'foo-b'"
        );
    }
}
