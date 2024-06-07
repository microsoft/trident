use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Error};
use osutils::{
    exe::RunAndCheck,
    filesystems::TabFileSystemType,
    tabfile::{TabFile, TabFileEntry},
};
use serde_json::Value;

use trident_api::{
    config::{FileSystemType, InternalMountPoint},
    status::HostStatus,
};

use crate::modules;

pub(super) const DEFAULT_FSTAB_PATH: &str = "/etc/fstab";

pub(crate) fn from_mountpoints(
    host_status: &HostStatus,
    mount_points: &[InternalMountPoint],
) -> Result<TabFile, Error> {
    // Generate a list of entries for the tab file
    let entries = mount_points
        .iter()
        .map(|mp| entry_from_mountpoint(host_status, mp))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(TabFile { entries })
}

fn entry_from_mountpoint(hs: &HostStatus, mp: &InternalMountPoint) -> Result<TabFileEntry, Error> {
    Ok(match mp.filesystem {
        // First, check the types that do not depend on a block device
        FileSystemType::Overlay => TabFileEntry::new_overlay(&mp.path),
        FileSystemType::Tmpfs => TabFileEntry::new_tmpfs(&mp.path),

        // Now, for all the types that *do* require a block device:
        fs_type => {
            // Try to look up the block device
            let device = modules::get_block_device(hs, &mp.target_id, false)
                .context(format!(
                    "Failed to find block device with id {}",
                    mp.target_id
                ))?
                .path;

            // Create the entry according to the file system type
            match fs_type {
                FileSystemType::Swap => TabFileEntry::new_swap(device),
                _ => TabFileEntry::new_path(
                    device,
                    &mp.path,
                    TabFileSystemType::from_api_type(fs_type)
                        .context("Invalid file system type")?,
                ),
            }
        }
    }
    .with_options(mp.options.clone()))
}

/// Based on the given tab file, get the device path for the partition with mount point `path`.
pub(crate) fn get_device_path(tab_file_path: &Path, path: &Path) -> Result<PathBuf, Error> {
    let findmnt_output_json = Command::new("findmnt")
        .arg("--tab-file")
        .arg(tab_file_path)
        .arg("--json")
        .arg("--output")
        .arg("source,target,fstype,vfs-options,fs-options,freq,passno")
        .arg("--mountpoint")
        .arg(path)
        .output_and_check()
        .context(format!("Failed to find {:?} in {:?}", path, tab_file_path))?;
    let map = parse_findmnt_output(findmnt_output_json.as_str())?;
    if map.len() != 1 {
        bail!(
            "Unexpected number of entries in the tab file matching the mount point '{}'",
            path.display()
        );
    }

    let device_path = map.get(path).context(format!(
        "Failed to find entry in the tab file matching the mount point '{}'",
        path.display()
    ))?;

    Ok(device_path.clone())
}

fn parse_findmnt_output(findmnt_output: &str) -> Result<HashMap<PathBuf, PathBuf>, Error> {
    let payload: Value = serde_json::from_str(findmnt_output)
        .context("Failed to deserialize output of tab file reader")?;

    let filesystems = payload["filesystems"].as_array().context(format!(
        "Unexpected formatting of the findmnt utility, missing 'filesystems' in {:?}",
        payload
    ))?;

    // returns the first error or the list of results
    filesystems.iter().map(parse_findmnt_entry).collect()
}

fn parse_findmnt_entry(entry: &Value) -> Result<(PathBuf, PathBuf), Error> {
    let device_path = entry["source"].as_str().context(format!(
        "Unexpected formatting of the findmnt utility, missing 'source' in {:?}",
        entry
    ))?;

    let mount_path = entry["target"].as_str().context(format!(
        "Unexpected formatting of the findmnt utility, missing 'target' in {:?}",
        entry
    ))?;

    Ok((PathBuf::from(mount_path), PathBuf::from(device_path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        collections::HashMap,
        io::Write,
        path::{Path, PathBuf},
        str::FromStr,
    };

    use indoc::indoc;
    use maplit::btreemap;
    use tempfile::NamedTempFile;

    use trident_api::{
        config::{
            Disk, FileSystemType, HostConfiguration, ImageFormat, ImageSha256, InternalImage,
            Partition, PartitionSize, PartitionTableType, PartitionType, Storage,
        },
        constants::{self, SWAP_MOUNT_POINT},
        status::{
            BlockDeviceContents, BlockDeviceInfo, HostStatus, ServicingState, ServicingType,
            Storage as StorageStatus,
        },
    };

    fn get_host_status() -> HostStatus {
        HostStatus {
            servicing_type: Some(ServicingType::CleanInstall),
            servicing_state: ServicingState::Staging,
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "os".to_owned(),
                        device: PathBuf::from("/dev/disk/by-bus/foobar"),
                        partition_table_type: PartitionTableType::Gpt,
                        partitions: vec![
                            Partition {
                                id: "efi".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::from_str("100M").unwrap(),
                            },
                            Partition {
                                id: "root".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "home".to_owned(),
                                partition_type: PartitionType::Home,
                                size: PartitionSize::from_str("10G").unwrap(),
                            },
                            Partition {
                                id: "swap".to_owned(),
                                partition_type: PartitionType::Swap,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: StorageStatus {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "efi".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 0,
                        contents: BlockDeviceContents::Image {
                            sha256: "2cb228bc3bbbc2174585327b255a7196075559ecd0c49bf710dfd5432af8f9ec".to_owned(),
                            length: 738484224,
                            url: "file:///root.raw.zst".to_owned(),
                        },
                    },
                    "home".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "swap".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/swap"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_entry_from_mountpoint_regular() {
        let host_status = get_host_status();

        assert_eq!(
            entry_from_mountpoint(
                &host_status,
                &InternalMountPoint {
                    path: PathBuf::from("/boot/efi"),
                    filesystem: FileSystemType::Vfat,
                    options: vec!["umask=0077".to_owned()],
                    target_id: "efi".to_owned(),
                },
            )
            .unwrap(),
            TabFileEntry::new_path(
                PathBuf::from("/dev/disk/by-partlabel/osp1"),
                PathBuf::from("/boot/efi"),
                TabFileSystemType::Vfat
            )
            .with_options(vec!["umask=0077".to_owned()])
        );
    }

    #[test]
    fn test_entry_from_mountpoint_swap() {
        let host_status = get_host_status();

        assert_eq!(
            entry_from_mountpoint(
                &host_status,
                &InternalMountPoint {
                    path: PathBuf::from(SWAP_MOUNT_POINT),
                    filesystem: FileSystemType::Swap,
                    options: vec!["sw".to_owned()],
                    target_id: "swap".to_owned(),
                },
            )
            .unwrap(),
            TabFileEntry::new_swap(PathBuf::from("/dev/disk/by-partlabel/swap"))
                .with_options(vec!["sw".into()])
        );
    }

    #[test]
    fn test_entry_from_mountpoint_tmpfs() {
        let host_status = get_host_status();

        assert_eq!(
            entry_from_mountpoint(
                &host_status,
                &InternalMountPoint {
                    path: PathBuf::from("/tmp"),
                    filesystem: FileSystemType::Tmpfs,
                    options: vec![],
                    target_id: "".to_owned(),
                },
            )
            .unwrap(),
            TabFileEntry::new_tmpfs(PathBuf::from("/tmp"))
        );
    }

    #[test]
    fn test_entry_from_mountpoint_overlay() {
        let host_status = get_host_status();

        assert_eq!(
            entry_from_mountpoint(
                &host_status,
                &InternalMountPoint {
                    path: PathBuf::from("/etc"),
                    filesystem: FileSystemType::Overlay,
                    options: vec![
                        "lowerdir=/etc".into(),
                        "upperdir=/var/lib/trident-overlay/etc/upper".into(),
                        "workdir=/var/lib/trident-overlay/etc/work".into(),
                        "ro".into()
                    ],
                    target_id: "".to_owned(),
                },
            )
            .unwrap(),
            TabFileEntry::new_overlay(PathBuf::from("/etc")).with_options(vec![
                "lowerdir=/etc".into(),
                "upperdir=/var/lib/trident-overlay/etc/upper".into(),
                "workdir=/var/lib/trident-overlay/etc/work".into(),
                "ro".into()
            ])
        );
    }

    #[test]
    fn test_from_mount_points() {
        let expected_fstab = indoc! {r#"
            /dev/disk/by-partlabel/osp1 /boot/efi vfat umask=0077 0 2
            /dev/disk/by-partlabel/osp2 / ext4 errors=remount-ro 0 1
            /dev/disk/by-partlabel/osp3 /home ext4 defaults,x-systemd.makefs 0 2
            /dev/disk/by-partlabel/swap none swap sw 0 0
        "#};

        let host_config = HostConfiguration {
            storage: Storage {
                internal_images: vec![
                    InternalImage {
                        url: "file:///path/to/efi-image".to_owned(),
                        sha256: ImageSha256::Checksum(
                            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
                                .to_owned(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "efi".into(),
                    },
                    InternalImage {
                        url: "file:///path/to/root-image".to_owned(),
                        sha256: ImageSha256::Checksum(
                            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"
                                .to_owned(),
                        ),
                        format: ImageFormat::RawZst,
                        target_id: "root".to_owned(),
                    },
                ],
                disks: vec![Disk {
                    id: "os".to_owned(),
                    device: PathBuf::from("/dev/disk/by-bus/foobar"),
                    partition_table_type: PartitionTableType::Gpt,
                    partitions: vec![
                        Partition {
                            id: "efi".to_owned(),
                            partition_type: PartitionType::Esp,
                            size: PartitionSize::from_str("100M").unwrap(),
                        },
                        Partition {
                            id: "root".to_owned(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "home".to_owned(),
                            partition_type: PartitionType::Home,
                            size: PartitionSize::from_str("10G").unwrap(),
                        },
                        Partition {
                            id: "swap".to_owned(),
                            partition_type: PartitionType::Swap,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                    ],
                    ..Default::default()
                }],
                internal_mount_points: vec![
                    InternalMountPoint {
                        path: PathBuf::from("/boot/efi"),
                        filesystem: FileSystemType::Vfat,
                        options: vec!["umask=0077".to_owned()],
                        target_id: "efi".to_owned(),
                    },
                    InternalMountPoint {
                        path: PathBuf::from(constants::ROOT_MOUNT_POINT_PATH),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["errors=remount-ro".to_owned()],
                        target_id: "root".to_owned(),
                    },
                    InternalMountPoint {
                        path: PathBuf::from("/home"),
                        filesystem: FileSystemType::Ext4,
                        options: vec!["defaults".to_owned(), "x-systemd.makefs".to_owned()],
                        target_id: "home".to_owned(),
                    },
                    InternalMountPoint {
                        path: PathBuf::from(SWAP_MOUNT_POINT),
                        filesystem: FileSystemType::Swap,
                        options: vec!["sw".to_owned()],
                        target_id: "swap".to_owned(),
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let host_status = HostStatus {
            servicing_type: Some(ServicingType::CleanInstall),
            servicing_state: ServicingState::Staging,
            spec: host_config.clone(),
            storage: StorageStatus {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "efi".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 0,
                        contents: BlockDeviceContents::Image {
                            sha256: "2cb228bc3bbbc2174585327b255a7196075559ecd0c49bf710dfd5432af8f9ec".to_owned(),
                            length: 738484224,
                            url: "file:///root.raw.zst".to_owned(),
                        },
                    },
                    "home".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "swap".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/swap"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        assert_eq!(
            from_mountpoints(&host_status, &host_config.storage.internal_mount_points)
                .unwrap()
                .render(),
            expected_fstab
        );

        let mut mount_points = host_config.storage.internal_mount_points;
        mount_points.push(InternalMountPoint {
            filesystem: FileSystemType::Overlay,
            options: vec![
                "lowerdir=/mnt".to_owned(),
                "upperdir=/mnt/newroot".to_owned(),
                "workdir=/mnt/work".to_owned(),
            ],
            path: PathBuf::from("/foo"),
            target_id: "".to_owned(),
        });
        assert_eq!(
            from_mountpoints(&host_status, &mount_points)
                .unwrap()
                .render(),
            format!("{expected_fstab}overlay /foo overlay lowerdir=/mnt,upperdir=/mnt/newroot,workdir=/mnt/work 0 2\n")
        );
    }

    #[test]
    fn test_get() {
        let tab_file_contents = indoc::indoc! {r#"
                /dev/sda1 /boot/efi vfat defaults 0 0
                /dev/sda2 / ext4 errors=remount-ro 0 0
                /dev/sdb1 /random ext4 defaults 0 2
            "#}
        .to_owned();

        // Save that temporary file
        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile.write_all(tab_file_contents.as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        assert_eq!(
            get_device_path(tmpfile.path(), Path::new(constants::ESP_MOUNT_POINT_PATH)).unwrap(),
            PathBuf::from("/dev/sda1")
        );

        assert_eq!(
            get_device_path(tmpfile.path(), Path::new(constants::ROOT_MOUNT_POINT_PATH)).unwrap(),
            PathBuf::from("/dev/sda2")
        );

        assert_eq!(
            get_device_path(tmpfile.path(), Path::new("/random")).unwrap(),
            PathBuf::from("/dev/sdb1")
        );

        // non-existing mount point
        assert_eq!(
            get_device_path(tmpfile.path(), Path::new("/foobar"))
                .err()
                .unwrap()
                .to_string(),
            format!("Failed to find \"/foobar\" in {:?}", tmpfile.path())
        );

        // non-existing input file
        assert_eq!(
            get_device_path(Path::new("/does-not-exist"), Path::new("/foobar"))
                .err()
                .unwrap()
                .to_string(),
            "Failed to find \"/foobar\" in \"/does-not-exist\""
        );

        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile.write_all("malformed".as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        // malformed input file
        assert_eq!(
            get_device_path(tmpfile.path(), Path::new("/foobar"))
                .err()
                .unwrap()
                .to_string(),
            format!("Failed to find \"/foobar\" in {:?}", tmpfile.path())
        );

        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile
            .write_all((tab_file_contents + "\n/dev/sdb1q /random ext4 defaults 0 2").as_bytes())
            .unwrap();
        tmpfile.flush().unwrap();

        // pick the latter
        assert_eq!(
            get_device_path(tmpfile.path(), Path::new("/random")).unwrap(),
            PathBuf::from("/dev/sdb1q")
        );
    }

    #[test]
    fn test_parse_findmnt_entry() {
        let input_json = r#"{"source":"foo","target":"bar"}"#;
        let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap();

        assert_eq!(
            super::parse_findmnt_entry(&input).unwrap(),
            (PathBuf::from("bar"), PathBuf::from("foo"))
        );

        // missing target
        let input_json = r#"{"source":"foo"}"#;
        let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap();
        assert!(super::parse_findmnt_entry(&input).is_err());

        // missing source
        let input_json = r#"{"target":"foo"}"#;
        let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap();
        assert!(super::parse_findmnt_entry(&input).is_err());

        // missing target and source
        let input_json = r#"{"foo":"foo"}"#;
        let input = serde_json::from_str::<serde_json::Value>(input_json).unwrap();
        assert!(super::parse_findmnt_entry(&input).is_err());
    }

    #[test]
    fn test_parse_findmnt_output() {
        let input = r#"{"filesystems": [{"source":"foo","target":"bar"}]}"#;
        let output: HashMap<PathBuf, PathBuf> = [(PathBuf::from("bar"), PathBuf::from("foo"))]
            .iter()
            .cloned()
            .collect();
        assert_eq!(super::parse_findmnt_output(input).unwrap(), output);

        // missing target
        let input = r#"{"filesystems": [{"source":"foo"}]}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        // missing source
        let input = r#"{"filesystems": [{"target":"foo"}]}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        // missing target and source
        let input = r#"{"filesystems": [{"foo":"foo"}]}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        let input = r#"{"filesystems": []}"#;
        assert!(super::parse_findmnt_output(input).unwrap().is_empty());

        let input = r#"{"filesystems": [{"source":"foo","target":"bar"},{"source":"foo2","target":"bar2"}]}"#;
        assert_eq!(super::parse_findmnt_output(input).unwrap().len(), 2);

        // no filesystems
        let input = r#"{"foo": []}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        // filesystems is not an array
        let input = r#"{"filesystems": {"foo": "bar"}}"#;
        assert!(super::parse_findmnt_output(input).is_err());

        // one entry is malformed
        let input = r#"{"filesystems": [{"source":"foo","target":"bar"},{"sourcssse":"foo2","target":"bar"},{"source":"foo2","target":"bar"}]}"#;
        assert!(super::parse_findmnt_output(input).is_err());
    }
}
