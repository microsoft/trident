use anyhow::{Context, Error};

use osutils::{
    filesystems::TabFileSystemType,
    tabfile::{TabFile, TabFileEntry},
};
use trident_api::config::{FileSystemType, InternalMountPoint};

use crate::engine::EngineContext;

pub(super) const DEFAULT_FSTAB_PATH: &str = "/etc/fstab";

pub(crate) fn from_mountpoints(
    ctx: &EngineContext,
    mount_points: &[InternalMountPoint],
) -> Result<TabFile, Error> {
    // Generate a list of entries for the tab file
    let entries = mount_points
        .iter()
        .map(|mp| entry_from_mountpoint(ctx, mp))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(TabFile { entries })
}

fn entry_from_mountpoint(
    hs: &EngineContext,
    mp: &InternalMountPoint,
) -> Result<TabFileEntry, Error> {
    Ok(match mp.filesystem {
        // First, check the types that do not depend on a block device
        FileSystemType::Overlay => TabFileEntry::new_overlay(&mp.path),
        FileSystemType::Tmpfs => TabFileEntry::new_tmpfs(&mp.path),

        // Now, for all the types that *do* require a block device:
        fs_type => {
            // Try to look up the block device
            let device = hs.get_block_device_path(&mp.target_id).context(format!(
                "Failed to find block device with id {}",
                mp.target_id
            ))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    use std::{path::PathBuf, str::FromStr};

    use indoc::indoc;
    use maplit::btreemap;

    use trident_api::{
        config::{
            Disk, FileSystemType, HostConfiguration, Partition, PartitionSize, PartitionTableType,
            PartitionType, Storage,
        },
        constants::{self, MOUNT_OPTION_READ_ONLY, SWAP_MOUNT_POINT},
        status::ServicingType,
    };

    fn get_ctx() -> EngineContext {
        EngineContext {
            servicing_type: ServicingType::CleanInstall,
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
            partition_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "home".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "swap".into() => PathBuf::from("/dev/disk/by-partlabel/swap"),
            },
            ..Default::default()
        }
    }

    #[test]
    fn test_entry_from_mountpoint_regular() {
        let ctx = get_ctx();

        assert_eq!(
            entry_from_mountpoint(
                &ctx,
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
        let ctx = get_ctx();

        assert_eq!(
            entry_from_mountpoint(
                &ctx,
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
        let ctx = get_ctx();

        assert_eq!(
            entry_from_mountpoint(
                &ctx,
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
        let ctx = get_ctx();

        assert_eq!(
            entry_from_mountpoint(
                &ctx,
                &InternalMountPoint {
                    path: PathBuf::from("/etc"),
                    filesystem: FileSystemType::Overlay,
                    options: vec![
                        "lowerdir=/etc".into(),
                        "upperdir=/var/lib/trident-overlay/etc/upper".into(),
                        "workdir=/var/lib/trident-overlay/etc/work".into(),
                        MOUNT_OPTION_READ_ONLY.into()
                    ],
                    target_id: "".to_owned(),
                },
            )
            .unwrap(),
            TabFileEntry::new_overlay(PathBuf::from("/etc")).with_options(vec![
                "lowerdir=/etc".into(),
                "upperdir=/var/lib/trident-overlay/etc/upper".into(),
                "workdir=/var/lib/trident-overlay/etc/work".into(),
                MOUNT_OPTION_READ_ONLY.into()
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

        let ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            spec: host_config.clone(),
            partition_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "home".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "swap".into() => PathBuf::from("/dev/disk/by-partlabel/swap"),
            },
            ..Default::default()
        };

        assert_eq!(
            from_mountpoints(&ctx, &host_config.storage.internal_mount_points)
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
            from_mountpoints(&ctx, &mount_points)
                .unwrap()
                .render(),
            format!("{expected_fstab}overlay /foo overlay lowerdir=/mnt,upperdir=/mnt/newroot,workdir=/mnt/work 0 2\n")
        );
    }
}
