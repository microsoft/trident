#![allow(unused)]

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    str::FromStr,
};

use log::{debug, info, trace};

use maplit::hashmap;
use osutils::lsblk;
use trident_api::{
    config::{
        AbUpdate, AbVolumePair, Disk, FileSystem, FileSystemSource, FileSystemType,
        HostConfiguration, MountOptions, MountPoint, Partition, PartitionSize, PartitionTableType,
        PartitionType, VerityCorruptionOption, VerityDevice,
    },
    constants::internal_params::ENABLE_UKI_SUPPORT,
    error::{
        ExecutionEnvironmentMisconfigurationError, InitializationError, InvalidInputError,
        ReportError, TridentError, TridentResultExt,
    },
    primitives::bytes::ByteCount,
    status::{AbVolumeSelection, HostStatus, ServicingState},
    BlockDeviceId,
};

use crate::datastore::DataStore;

#[derive(Debug, serde::Deserialize)]
struct PrismPartition {
    id: BlockDeviceId,
    #[allow(unused)]
    start: String,
    #[allow(unused)]
    size: String,

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
    disks: Vec<PrismDisk>,
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

/// Given a path to a Host Status file, initializes the datastore with the Host Status.
/// This command can be executed offline in a chroot environment as part of MIC image customization.
pub fn execute(hs_path: Option<&Path>) -> Result<(), TridentError> {
    let host_status: HostStatus = if let Some(hs_path) = hs_path {
        info!("Reading Host Status from {:?}", hs_path);
        let host_status_yaml = fs::read_to_string(hs_path)
            .structured(InitializationError::LoadHostStatus)
            .message(format!("Failed to read Host Status from {:?}", hs_path))?;
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

        let Some(prism_storage) = history
            .iter()
            .rev()
            .map(|entry| entry.config.storage.as_ref())
            .find(|storage| storage.is_some())
            .flatten()
        else {
            return Err(TridentError::new(InvalidInputError::ParsePrismHistory))
                .message("Prism history doesn't contain any storage information");
        };

        let prism_partitions = &prism_storage
            .disks
            .first()
            .structured(InvalidInputError::ParsePrismHistory)
            .message("Prism history doesn't contain any disks")?
            .partitions;
        let mut host_config = HostConfiguration::default();

        let mut partitions = Vec::new();
        for partition in prism_partitions {
            partitions.push(Partition {
                id: partition.id.clone(),
                partition_type: if partition.ty.as_deref() == Some("esp") {
                    PartitionType::Esp
                } else {
                    PartitionType::LinuxGeneric
                },
                size: PartitionSize::from_str(&partition.size)
                    .structured(InvalidInputError::ParsePrismHistory)
                    .message(format!(
                        "Failed to parse partition size '{}'",
                        partition.size
                    ))?,
            });
        }

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

        // Run lsblk, and then search the output devices to find the one containing a child mounted
        // at '/'. Since we are running inside Prism, this will be a loop device such as /dev/loop29.
        let lsblk_output = lsblk::list()
            .structured(ExecutionEnvironmentMisconfigurationError::PrismChrootEnvironment)
            .message("Failed to run lsblk")?;

        let lsblk_device = lsblk_output
            .iter()
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

        // TODO: should we be sorting by partn?

        for (i, part) in lsblk_device.children.iter().enumerate() {
            if part.part_uuid.is_none() {
                return Err(TridentError::new(
                    ExecutionEnvironmentMisconfigurationError::PrismChrootEnvironment,
                ))
                .message(format!("No part UUID found for partition {}", i + 1));
            }
        }

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

        HostStatus {
            spec: host_config,
            disk_uuids: hashmap!["disk0".to_string() => disk_uuid],
            partition_paths,
            servicing_state: ServicingState::Provisioned,
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            install_index: 0,
            is_management_os: false,
            ..Default::default()
        }
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
        "Failed to persist Host Status to datastore at {:?}",
        datastore_path
    ))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRISM_HISTORY: &str = r#"
[
 {
  "timestamp": "2025-04-17T21:49:51Z",
  "toolVersion": "0.13.0",
  "imageUuid": "ad883874-8d4c-b201-d58b-c90dd1de4fa4",
  "config": {
   "input": {
    "image": {}
   },
   "storage": {
    "bootType": "efi",
    "disks": [
     {
      "partitionTableType": "gpt",
      "maxSize": "13250M",
      "partitions": [
       {
        "id": "esp",
        "start": "1M",
        "size": "512M",
        "type": "esp"
       },
       {
        "id": "trident",
        "start": "513M",
        "size": "512M",
        "type": "linux-generic"
       },
       {
        "id": "boot-a",
        "start": "1025M",
        "size": "256M",
        "type": "xbootldr"
       },
       {
        "id": "boot-b",
        "start": "1281M",
        "size": "256M",
        "type": "xbootldr"
       },
       {
        "id": "root-a",
        "start": "1537M",
        "size": "4G",
        "type": "root"
       },
       {
        "id": "root-b",
        "start": "5633M",
        "size": "4G",
        "type": "root"
       },
       {
        "id": "root-hash-a",
        "start": "9729M",
        "size": "128M",
        "type": "root-verity"
       },
       {
        "id": "root-hash-b",
        "start": "9857M",
        "size": "128M",
        "type": "root-verity"
       },
       {
        "id": "var-a",
        "start": "9985M",
        "size": "1G",
        "type": "linux-generic"
       },
       {
        "id": "var-b",
        "start": "11009M",
        "size": "1G",
        "type": "linux-generic"
       },
       {
        "id": "trident-overlay-a",
        "start": "12033M",
        "size": "32M",
        "type": "linux-generic"
       },
       {
        "id": "trident-overlay-b",
        "start": "12065M",
        "size": "32M",
        "type": "linux-generic"
       },
       {
        "id": "home",
        "start": "12097M",
        "size": "1G",
        "type": "linux-generic"
       },
       {
        "id": "srv",
        "start": "13121M",
        "size": "128M",
        "type": "linux-generic"
       }
      ]
     }
    ],
    "filesystems": [
     {
      "deviceId": "esp",
      "type": "fat32",
      "mountPoint": {
       "options": "umask=0077",
       "path": "/boot/efi"
      }
     },
     {
      "deviceId": "trident",
      "type": "ext4",
      "mountPoint": {
       "path": "/var/lib/trident"
      }
     },
     {
      "deviceId": "boot-a",
      "type": "ext4",
      "mountPoint": {
       "path": "/boot"
      }
     },
     {
      "deviceId": "rootverity",
      "type": "ext4",
      "mountPoint": {
       "options": "defaults,ro",
       "path": "/"
      }
     },
     {
      "deviceId": "var-a",
      "type": "ext4",
      "mountPoint": {
       "path": "/var"
      }
     },
     {
      "deviceId": "trident-overlay-a",
      "type": "ext4",
      "mountPoint": {
       "path": "/var/lib/trident-overlay"
      }
     },
     {
      "deviceId": "home",
      "type": "ext4",
      "mountPoint": {
       "path": "/home"
      }
     },
     {
      "deviceId": "srv",
      "type": "ext4",
      "mountPoint": {
       "path": "/srv"
      }
     }
    ],
    "verity": [
     {
      "id": "rootverity",
      "name": "root",
      "dataDeviceId": "root-a",
      "dataDeviceMountIdType": "uuid",
      "hashDeviceId": "root-hash-a",
      "hashDeviceMountIdType": "uuid"
     }
    ]
   },
   "os": {
    "hostname": "trident-vm-verity-testimg",
    "packages": {
     "install": [
      "device-mapper",
      "dnf",
      "efibootmgr",
      "grub2-efi-binary",
      "iproute",
      "iptables",
      "jq",
      "kexec-tools",
      "lvm2",
      "openssh-server",
      "systemd-udev",
      "trident",
      "veritysetup",
      "vim"
     ]
    },
    "selinux": {
     "mode": "disabled"
    },
    "kernelCommandLine": {
     "extraCommandLine": [
      "console=tty0",
      "console=tty1",
      "console=ttyS0",
      "rd.debug",
      "loglevel=6",
      "log_buf_len=1M",
      "systemd.journald.forward_to_console=1"
     ]
    },
    "additionalFiles": [
     {
      "destination": "/etc/systemd/system/etc-mount.service",
      "source": "files/etc-mount-mic.service",
      "sha256hash": "65432875fecae58c25597744e50bd9403a6e37083f799c960e23b17ac44e6e58"
     },
     {
      "destination": "/usr/local/bin/etc-mount.sh",
      "source": "files/etc-mount.sh",
      "sha256hash": "f89e3dd2d11f753591d92010fdf8ae0c7060cdd56e2322cf1b32191fd04e7bc2"
     },
     {
      "destination": "/usr/lib/systemd/system/sshd-keygen.service",
      "source": "files/sshd-keygen.service",
      "sha256hash": "edf0b999ec21a7acfe0e395883b258af504ac96d098ef9c3c89ad5075dbe7dbc"
     },
     {
      "destination": "/etc/systemd/network/99-dhcp-eth0.network",
      "source": "files/99-dhcp-eth0.network",
      "sha256hash": "de3713d0e3a2d0f34ee9ef61a64f1ee43a230ca138cde0e95bdeded58c519881"
     },
     {
      "destination": "/etc/sudoers.d/wheel",
      "source": "files/sudoers-wheel",
      "sha256hash": "4ff327102cb80dddb24477c0c2faefd47a5cd99d28aa8b4a3fcc45b1b1adab26"
     }
    ],
    "users": [
     {
      "name": "testuser",
      "sshPublicKeyPaths": [
       "/mnt/vss/_work/1/b/2/_work/1/s/test-images/platform-integration-images/trident-vm-testimage/base/files/id_rsa.pub"
      ],
      "secondaryGroups": [
       "wheel"
      ]
     }
    ],
    "services": {
     "enable": [
      "etc-mount",
      "kdump"
     ]
    },
    "bootloader": {
     "resetType": "hard-reset"
    }
   },
   "scripts": {
    "postCustomization": [
     {
      "path": "scripts/post-install.sh",
      "sha256hash": "0e01438e4cb5c48fd2e15bee11a5e675fff564d42f63627ab31d6457caae7a00"
     },
     {
      "path": "scripts/create-rw-overlay-dirs.sh",
      "sha256hash": "6f47ae8eae1521b9c65cdc5a94039fa89150959c06abe98e02234b649156f1db"
     },
     {
      "path": "scripts/update-host-status.sh",
      "sha256hash": "17d5364febaaf3c5a1597ff26d957bae06356e3d711ac07d1f3678cd6a1a3278"
     },
     {
      "path": "scripts/prepare-update-config-verity.sh",
      "sha256hash": "c24669365210ae3727df803867c9d6d264b01c6c304ff5b7893c0000c1d6c84d"
     },
     {
      "path": "scripts/ssh-move-host-keys.sh",
      "sha256hash": "5338dfbe6994c78653b383eb701a7bcc9893f2cbc6beefd64cd63bfa732abec7"
     },
     {
      "path": "scripts/create-ssh-dirs.sh",
      "sha256hash": "1cae9817f61e1d258fe56c7a59e006f29cffc5defcc401e90a2f97db7afd94b5"
     },
     {
      "path": "scripts/duid-type-to-link-layer.sh",
      "sha256hash": "f82839dea5fa59b685da0e2c76cc7c9bfa692094ae5f22f0822ed7c8794841d3"
     },
     {
      "path": "scripts/patch-dracut.sh",
      "sha256hash": "f2142c67bde558934e6ee605e4a75d17a968e5ae80214fc5eee2c74b8119c883"
     }
    ]
   },
   "output": {
    "image": {}
   }
  }
 }
]
"#;

    #[test]
    fn test_prism_history() {
        let history: Vec<PrismHistoryEntry> =
            serde_json::from_str(PRISM_HISTORY).expect("Failed to parse Prism history");
        assert_eq!(history.len(), 1);
        let entry = &history[0];
        assert_eq!(entry.config.storage.as_ref().unwrap().disks.len(), 1);
        let disk = &entry.config.storage.as_ref().unwrap().disks[0];
        assert_eq!(disk.partitions.len(), 14);
        assert_eq!(disk.partitions[0].id, "esp");
        assert_eq!(disk.partitions[1].id, "trident");
    }
}
