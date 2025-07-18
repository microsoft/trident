use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use log::trace;

use osutils::{
    filesystems::TabFileSystemType,
    tabfile::{TabFile, TabFileEntry},
};
use trident_api::{config::Swap, BlockDeviceId};

use crate::engine::{filesystem::FileSystemData, EngineContext};

use super::verity;

pub(super) const DEFAULT_FSTAB_PATH: &str = "/etc/fstab";

const DISABLED_REASON_VERITY: &str = "Mounting is handled by veritysetup generator";

/// Create a tabfile that captures all the desired as per the spec in engine context.
pub(super) fn generate_fstab(ctx: &EngineContext, output_path: &Path) -> Result<(), Error> {
    // Helper closure to find the block device path for a given device id.
    let device_finder = |device_id: &BlockDeviceId| -> Result<PathBuf, Error> {
        ctx.get_block_device_path(device_id)
            .context(format!("Failed to find block device with id '{device_id}'"))
    };

    // Helper to check if a mount point should be disabled and provide a reason if so.
    let check_disabled = |mount_point_path: &Path| -> Result<Option<String>, Error> {
        // Check if this mount point is on a verity device, if so, disable it as it is handled by the veritysetup generator.
        if ctx
            .storage_graph
            .verity_device_for_filesystem(mount_point_path)
            .is_some()
        {
            trace!(
                "Skipping filesystem '{}' mounted on verity device",
                mount_point_path.display()
            );
            return Ok(Some(DISABLED_REASON_VERITY.into()));
        }

        Ok(None)
    };

    // Iterate over all filesystems in the context and create entries for them.
    let mut entries = ctx
        .filesystems()
        .filter_map(|fsdata| {
            entry_from_fs_data(check_disabled, device_finder, fsdata)
                .context("Failed to create fstab entry for filesystem")
                .transpose()
        })
        .collect::<Result<Vec<_>, _>>()?;

    if ctx.storage_graph.root_fs_is_verity() {
        entries.push(verity::create_etc_overlay_mount_point());
    }

    let swap_entries = ctx
        .spec
        .storage
        .swap
        .iter()
        .map(|swap| entry_from_swap(device_finder, swap))
        .collect::<Result<Vec<_>, _>>()?;

    // Add the swap entries to the list of entries
    entries.extend(swap_entries);

    let fstab = TabFile { entries };

    fstab
        .write(output_path)
        .context(format!("Failed to write {}", output_path.display()))?;

    trace!("Wrote '{}', contents: '{:?}'", output_path.display(), fstab);

    Ok(())
}

fn entry_from_fs_data(
    check_disabled: impl Fn(&Path) -> Result<Option<String>, Error>,
    device_finder: impl Fn(&BlockDeviceId) -> Result<PathBuf, Error>,
    fsd: FileSystemData,
) -> Result<Option<TabFileEntry>, Error> {
    let (device_id, fs_type, mount_point) = match fsd {
        FileSystemData::Overlay(ofs) => {
            return Ok(Some(
                TabFileEntry::new_overlay(&ofs.mount_point.path)
                    .with_options(ofs.mount_point.options.to_string_vec()),
            ))
        }

        FileSystemData::Tmpfs(tmpfs) => {
            return Ok(Some(
                TabFileEntry::new_tmpfs(&tmpfs.mount_point.path)
                    .with_options(tmpfs.mount_point.options.to_string_vec()),
            ))
        }

        FileSystemData::Image(ifs) => (
            ifs.device_id,
            ifs.fs_type
                .map(Into::into)
                .unwrap_or(TabFileSystemType::Auto),
            Some(ifs.mount_point),
        ),

        FileSystemData::Adopted(afs) => (
            afs.device_id,
            afs.fs_type
                .map_or(TabFileSystemType::Auto, |fs_type| fs_type.into()),
            afs.mount_point,
        ),
        FileSystemData::New(nfs) => (nfs.device_id, nfs.fs_type.into(), nfs.mount_point),
    };

    let Some(mount_point) = mount_point else {
        // Only continue if there is a mount point.
        return Ok(None);
    };

    let device_path = device_finder(&device_id)?;

    // Check if this entry should be disabled, and if so, get the reason.
    let disabled_reason = check_disabled(&mount_point.path).context(format!(
        "Failed to check if mount point '{}' is disabled",
        mount_point.path.display()
    ))?;

    Ok(Some(
        TabFileEntry::new_path(device_path, &mount_point.path, fs_type)
            .with_options(mount_point.options.to_string_vec())
            .with_disabled_reason(disabled_reason),
    ))
}

fn entry_from_swap(
    device_finder: impl Fn(&BlockDeviceId) -> Result<PathBuf, Error>,
    swap: &Swap,
) -> Result<TabFileEntry, Error> {
    Ok(TabFileEntry::new_swap(device_finder(&swap.device_id)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs, path::PathBuf, str::FromStr};

    use anyhow::bail;
    use const_format::formatcp;
    use indoc::indoc;
    use maplit::btreemap;
    use uuid::Uuid;

    use sysdefs::{filesystems::RealFilesystemType, partition_types::DiscoverablePartitionType};
    use tempfile::NamedTempFile;
    use trident_api::{
        config::{
            Disk, FileSystem, FileSystemSource, HostConfiguration, MountOptions, MountPoint,
            NewFileSystemType, Partition, PartitionSize, PartitionTableType, PartitionType,
            Storage, VerityDevice,
        },
        constants::{
            ESP_MOUNT_POINT_PATH, MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH,
            USR_MOUNT_POINT_PATH,
        },
        status::ServicingType,
    };

    use crate::{
        engine::filesystem::{
            FileSystemDataAdopted, FileSystemDataImage, FileSystemDataNew, FileSystemDataOverlay,
            FileSystemDataTmpfs,
        },
        osimage::{
            mock::{MockImage, MockOsImage},
            OsImage, OsImageFileSystemType,
        },
    };

    fn device_finder(device_id: &BlockDeviceId) -> Result<PathBuf, Error> {
        Ok(match device_id.as_str() {
            "os" => PathBuf::from("/dev/disk/by-bus/foobar"),
            "efi" => PathBuf::from("/dev/disk/by-partlabel/osp1"),
            "root" => PathBuf::from("/dev/disk/by-partlabel/osp2"),
            "home" => PathBuf::from("/dev/disk/by-partlabel/osp3"),
            "swap" => PathBuf::from("/dev/disk/by-partlabel/swap"),
            u => bail!("Unknown device id '{}'", u),
        })
    }

    #[test]
    fn test_entry_from_fs_data_image() {
        assert_eq!(
            entry_from_fs_data(
                |_| Ok(None),
                device_finder,
                FileSystemDataImage {
                    mount_point: MountPoint {
                        path: PathBuf::from("/boot/efi"),
                        options: MountOptions::new("umask=0077"),
                    },
                    fs_type: Some(RealFilesystemType::Vfat),
                    device_id: "efi".to_owned(),
                }
                .into(),
            )
            .unwrap()
            .unwrap(),
            TabFileEntry::new_path(
                "/dev/disk/by-partlabel/osp1",
                "/boot/efi",
                RealFilesystemType::Vfat.into(),
            )
            .with_options(vec!["umask=0077".to_owned()])
        );
    }

    #[test]
    fn test_entry_from_fs_data_new() {
        assert_eq!(
            entry_from_fs_data(
                |_| Ok(None),
                device_finder,
                FileSystemDataNew {
                    mount_point: Some(MountPoint::from_str("/mnt/data").unwrap()),
                    fs_type: RealFilesystemType::Ext4,
                    device_id: "os".to_owned(),
                }
                .into(),
            )
            .unwrap()
            .unwrap(),
            TabFileEntry::new_path(
                "/dev/disk/by-bus/foobar",
                "/mnt/data",
                RealFilesystemType::Ext4.into(),
            )
            .with_options(vec!["defaults".to_owned()])
        );
    }

    #[test]
    fn test_entry_from_fs_data_adopted() {
        assert_eq!(
            entry_from_fs_data(
                |_| Ok(None),
                device_finder,
                FileSystemDataAdopted {
                    mount_point: Some(MountPoint::from_str("/mnt/data").unwrap()),
                    fs_type: Some(RealFilesystemType::Ext4),
                    device_id: "os".to_owned(),
                }
                .into(),
            )
            .unwrap()
            .unwrap(),
            TabFileEntry::new_path(
                "/dev/disk/by-bus/foobar",
                "/mnt/data",
                RealFilesystemType::Ext4.into(),
            )
            .with_options(vec!["defaults".to_owned()])
        );
    }

    #[test]
    fn test_entry_from_fs_data_adopted_unmounted() {
        assert_eq!(
            entry_from_fs_data(
                |_| Ok(None),
                device_finder,
                FileSystemDataAdopted {
                    mount_point: None,
                    fs_type: Some(RealFilesystemType::Ext4),
                    device_id: "os".to_owned(),
                }
                .into(),
            )
            .unwrap(),
            None
        );
    }

    #[test]
    fn test_entry_from_fs_data_tmpfs() {
        assert_eq!(
            entry_from_fs_data(
                |_| Ok(None),
                device_finder,
                FileSystemDataTmpfs {
                    mount_point: MountPoint::from_str("/tmp").unwrap(),
                }
                .into(),
            )
            .unwrap()
            .unwrap(),
            TabFileEntry::new_tmpfs("/tmp").with_options(vec!["defaults".to_owned()])
        );
    }

    #[test]
    fn test_entry_from_fs_data_overlay() {
        assert_eq!(
            entry_from_fs_data(
                |_| Ok(None),
                device_finder,
                FileSystemDataOverlay {
                    mount_point: MountPoint {
                        path: PathBuf::from("/etc"),
                        options: MountOptions::new("")
                            .with("lowerdir=/etc")
                            .with("upperdir=/var/lib/trident-overlay/etc/upper")
                            .with("workdir=/var/lib/trident-overlay/etc/work")
                            .with(MOUNT_OPTION_READ_ONLY),
                    }
                }
                .into(),
            )
            .unwrap()
            .unwrap(),
            TabFileEntry::new_overlay("/etc").with_options(vec![
                "lowerdir=/etc".into(),
                "upperdir=/var/lib/trident-overlay/etc/upper".into(),
                "workdir=/var/lib/trident-overlay/etc/work".into(),
                MOUNT_OPTION_READ_ONLY.into()
            ])
        );
    }

    #[test]
    fn test_entry_from_swap() {
        assert_eq!(
            entry_from_swap(
                device_finder,
                &Swap {
                    device_id: "swap".to_owned(),
                },
            )
            .unwrap(),
            TabFileEntry::new_swap("/dev/disk/by-partlabel/swap")
        );
    }

    #[test]
    fn test_disabled_entry() {
        assert_eq!(
            entry_from_fs_data(
                |_| Ok(Some("Mounting is handled by veritysetup generator".into())),
                device_finder,
                FileSystemDataImage {
                    mount_point: MountPoint {
                        path: PathBuf::from("/boot/efi"),
                        options: MountOptions::new("umask=0077"),
                    },
                    fs_type: Some(RealFilesystemType::Vfat),
                    device_id: "efi".to_owned(),
                }
                .into(),
            )
            .unwrap()
            .unwrap(),
            TabFileEntry::new_path(
                "/dev/disk/by-partlabel/osp1",
                "/boot/efi",
                RealFilesystemType::Vfat.into(),
            )
            .with_disabled_reason(Some(
                "Mounting is handled by veritysetup generator".to_owned(),
            ))
            .with_options(vec!["umask=0077".to_owned()])
        );
    }

    #[test]
    fn test_generate_fstab_regular() {
        let expected_fstab = indoc! {r#"
            /dev/disk/by-partlabel/osp1 /boot/efi vfat umask=0077 0 2
            /dev/disk/by-partlabel/osp2 / ext4 errors=remount-ro 0 1
            /dev/disk/by-partlabel/osp3 /home ext4 defaults,x-systemd.makefs 0 2
            /dev/disk/by-partlabel/swap none swap defaults 0 0
        "#};

        let ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            spec: HostConfiguration {
                storage: Storage {
                    swap: vec![Swap {
                        device_id: "swap".to_owned(),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            filesystems: vec![
                FileSystemDataImage {
                    mount_point: MountPoint {
                        path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                        options: "umask=0077".into(),
                    },
                    fs_type: Some(RealFilesystemType::Vfat),
                    device_id: "efi".to_owned(),
                }
                .into(),
                FileSystemDataImage {
                    mount_point: MountPoint {
                        path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                        options: "errors=remount-ro".into(),
                    },
                    fs_type: Some(RealFilesystemType::Ext4),
                    device_id: "root".to_owned(),
                }
                .into(),
                FileSystemDataNew {
                    mount_point: Some(MountPoint {
                        path: PathBuf::from("/home"),
                        options: "defaults,x-systemd.makefs".into(),
                    }),
                    fs_type: RealFilesystemType::Ext4,
                    device_id: "home".to_owned(),
                }
                .into(),
            ],
            partition_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "home".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "swap".into() => PathBuf::from("/dev/disk/by-partlabel/swap"),
            },
            ..Default::default()
        };

        let tmp_file = NamedTempFile::new().unwrap();
        generate_fstab(&ctx, tmp_file.path()).unwrap();
        assert_eq!(fs::read_to_string(tmp_file.path()).unwrap(), expected_fstab);
    }

    #[test]
    fn test_generate_fstab_verity() {
        /// Produces the expected fstab with an optional component added before swap.
        fn expected_fstab(extra: Option<&str>) -> String {
            [
                "/dev/disk/by-partlabel/osp4 /home ext4 defaults,x-systemd.makefs 0 2",
                "/dev/disk/by-partlabel/osp1 /boot/efi vfat umask=0077 0 2",
                formatcp!("# {DISABLED_REASON_VERITY}"),
                "# /dev/mapper/root / ext4 ro 0 1",
            ]
            .into_iter()
            .chain(extra)
            .chain(["/dev/disk/by-partlabel/swap none swap defaults 0 0"])
            .fold(String::new(), |acc, item| acc + item + "\n")
        }

        let hc = HostConfiguration {
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
                            id: "root-data".to_owned(),
                            partition_type: PartitionType::Root,
                            size: PartitionSize::from_str("1G").unwrap(),
                        },
                        Partition {
                            id: "root-hash".to_owned(),
                            partition_type: PartitionType::RootVerity,
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
                verity: vec![VerityDevice {
                    id: "root".to_owned(),
                    name: "root".to_owned(),
                    data_device_id: "root-data".to_owned(),
                    hash_device_id: "root-hash".to_owned(),
                    ..Default::default()
                }],
                filesystems: vec![
                    FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                            options: "umask=0077".into(),
                        }),
                        device_id: Some("efi".into()),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            options: "ro".into(),
                        }),
                        device_id: Some("root".into()),
                        source: FileSystemSource::Image,
                    },
                    FileSystem {
                        mount_point: Some(MountPoint {
                            path: PathBuf::from("/home"),
                            options: "defaults,x-systemd.makefs".into(),
                        }),
                        device_id: Some("home".into()),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                    },
                ],
                swap: vec![Swap {
                    device_id: "swap".to_owned(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        let os_image = MockOsImage::new().with_images(vec![
            MockImage::new(
                PathBuf::from(ROOT_MOUNT_POINT_PATH),
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::Esp,
                None::<&str>,
            ),
            MockImage::new(
                PathBuf::from(ESP_MOUNT_POINT_PATH),
                OsImageFileSystemType::Vfat,
                DiscoverablePartitionType::Root,
                Some(Uuid::new_v4().to_string()),
            ),
        ]);

        let mut ctx = EngineContext {
            storage_graph: hc.storage.build_graph().unwrap(),
            spec: hc,
            image: Some(OsImage::mock(os_image)),
            servicing_type: ServicingType::CleanInstall,
            filesystems: Vec::new(), // Will be populated in populate_filesystems
            partition_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root-data".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "root-hash".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "root".into() => PathBuf::from("/dev/mapper/root"),
                "home".into() => PathBuf::from("/dev/disk/by-partlabel/osp4"),
                "swap".into() => PathBuf::from("/dev/disk/by-partlabel/swap"),
            },
            ..Default::default()
        };

        // Populate the filesystems in the context
        ctx.populate_filesystems()
            .expect("Failed to populate filesystems");

        let tmp_file = NamedTempFile::new().unwrap();
        generate_fstab(&ctx, tmp_file.path()).unwrap();
        assert_eq!(
            fs::read_to_string(tmp_file.path()).unwrap(),
            expected_fstab(Some(
                verity::create_etc_overlay_mount_point().render().trim()
            ))
        );
    }

    #[test]
    fn test_generate_fstab_usrverity() {
        let expected_fstab = indoc! {r#"
            /dev/disk/by-partlabel/osp1 /boot/efi vfat umask=0077 0 2
            /dev/disk/by-partlabel/osp2 / ext4 defaults 0 1
            # Mounting is handled by veritysetup generator
            # /dev/mapper/usr /usr ext4 ro 0 2
            /dev/disk/by-partlabel/swap none swap defaults 0 0
        "#};

        let ctx = EngineContext::default()
            .with_partition_paths(
                [
                    ("os", "/dev/disk/by-bus/foobar"),
                    ("efi", "/dev/disk/by-partlabel/osp1"),
                    ("root", "/dev/disk/by-partlabel/osp2"),
                    ("usr-data", "/dev/disk/by-partlabel/osp3"),
                    ("usr-hash", "/dev/disk/by-partlabel/osp4"),
                    ("swap", "/dev/disk/by-partlabel/swap"),
                ]
                .into_iter(),
            )
            .with_spec(HostConfiguration {
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
                                partition_type: PartitionType::Home,
                                size: PartitionSize::from_str("10G").unwrap(),
                            },
                            Partition {
                                id: "usr-data".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "usr-hash".to_owned(),
                                partition_type: PartitionType::RootVerity,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                            Partition {
                                id: "swap".to_owned(),
                                partition_type: PartitionType::Swap,
                                size: PartitionSize::from_str("1G").unwrap(),
                            },
                        ],
                        ..Default::default()
                    }],
                    verity: vec![VerityDevice {
                        id: "usr".to_owned(),
                        name: "usr".to_owned(),
                        data_device_id: "usr-data".to_owned(),
                        hash_device_id: "usr-hash".to_owned(),
                        ..Default::default()
                    }],
                    filesystems: vec![
                        FileSystem {
                            mount_point: Some(MountPoint {
                                path: PathBuf::from(ESP_MOUNT_POINT_PATH),
                                options: "umask=0077".into(),
                            }),
                            device_id: Some("efi".into()),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            mount_point: Some(MountPoint::from(ROOT_MOUNT_POINT_PATH)),
                            device_id: Some("root".into()),
                            source: FileSystemSource::Image,
                        },
                        FileSystem {
                            mount_point: Some(MountPoint {
                                path: PathBuf::from(USR_MOUNT_POINT_PATH),
                                options: "ro".into(),
                            }),
                            device_id: Some("usr".into()),
                            source: FileSystemSource::Image,
                        },
                    ],
                    swap: vec![Swap {
                        device_id: "swap".to_owned(),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            })
            .with_image(MockOsImage::new().with_images([
                MockImage::new(
                    PathBuf::from(ESP_MOUNT_POINT_PATH),
                    OsImageFileSystemType::Vfat,
                    DiscoverablePartitionType::Esp,
                    None::<&str>,
                ),
                MockImage::new(
                    PathBuf::from(ROOT_MOUNT_POINT_PATH),
                    OsImageFileSystemType::Ext4,
                    DiscoverablePartitionType::Root,
                    None::<&str>,
                ),
                MockImage::new(
                    PathBuf::from(USR_MOUNT_POINT_PATH),
                    OsImageFileSystemType::Ext4,
                    DiscoverablePartitionType::Usr,
                    Some("usrverity-roothash"),
                ),
            ]))
            .with_filesystem_data();

        let tmp_file = NamedTempFile::new().unwrap();
        generate_fstab(&ctx, tmp_file.path()).unwrap();
        assert_eq!(fs::read_to_string(tmp_file.path()).unwrap(), expected_fstab);
    }

    #[test]
    fn test_empty_mount_options_fstab_entry_creation() {
        let host_config_with_mount_options_as_empty_string = r#"
            path: /boot
            options: ''
        "#;
        let mount_point: MountPoint =
            serde_yaml::from_str(host_config_with_mount_options_as_empty_string).unwrap();
        assert_eq!(mount_point.options, MountOptions::empty());

        let fstab_entry =
            TabFileEntry::new_path("/foo", &mount_point.path, TabFileSystemType::Auto)
                .with_options(mount_point.options.to_string_vec())
                .render();
        print!("Fstab entry: {fstab_entry}");
        assert!(fstab_entry.contains("defaults"));
    }
}
