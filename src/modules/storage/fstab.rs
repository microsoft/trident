use std::{
    collections::HashMap,
    fs,
    path::{self, Path, PathBuf},
};

use anyhow::{Context, Error};
use trident_api::{
    config::{HostConfiguration, MountPoint},
    status::HostStatus,
};

use crate::get_block_device;

pub(crate) struct Fstab {
    fstab_lines: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub(crate) struct FstabLine {
    pub device_path: PathBuf,
    pub mount_path: PathBuf,
    pub _filesystem: String,
    pub _options: Vec<String>,
    pub _dump: u32,
    pub _fsck_pass: u32,
}

pub const DEFAULT_FSTAB_PATH: &str = "/etc/fstab";

impl Fstab {
    pub fn read(fstab_path: &Path) -> Result<Self, Error> {
        let fstab =
            fs::read_to_string(fstab_path).context(format!("Failed to read {:?}", fstab_path))?;
        let fstab_lines: Vec<String> = fstab.lines().map(|l| l.to_owned()).collect();
        Ok(Self { fstab_lines })
    }

    pub fn from_mount_points(
        host_status: &HostStatus,
        mount_points: &Vec<MountPoint>,
        path_prefix: &path::Path,
        required_by: &path::Path,
    ) -> Result<Self, Error> {
        let mut fstab_lines = Vec::new();
        for mp in mount_points {
            if mp.path.starts_with("/") {
                let fstab_line = Self::mount_point_to_fstab_line(
                    host_status,
                    mp,
                    Some(path_prefix),
                    Some(vec![
                        "x-systemd.required-by=".to_owned()
                            + required_by.to_str().context(format!(
                                "Failed to convert path {:?} to string",
                                required_by
                            ))?,
                        "x-systemd.before=".to_owned()
                            + required_by.to_str().context(format!(
                                "Failed to convert path {:?} to string",
                                required_by
                            ))?,
                    ]),
                )?;
                fstab_lines.push(fstab_line);
            }
        }
        Ok(Self { fstab_lines })
    }

    pub fn write(&self, fstab_path: &Path) -> Result<(), Error> {
        fs::write(fstab_path, self.fstab_lines.join("\n").as_bytes()).context(format!(
            "Failed to write new {}",
            fstab_path.to_string_lossy()
        ))?;
        Ok(())
    }

    pub fn get(&self, path: &Path) -> Option<FstabLine> {
        self.fstab_lines.iter().find_map(|line| {
            let line = Fstab::parse_fstab_line(line);
            match line {
                Ok(line) => {
                    if line.mount_path == path {
                        Some(line)
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        })
    }

    fn parse_fstab_line(line: &str) -> Result<FstabLine, Error> {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        if tokens.is_empty() || tokens[0].starts_with('#') {
            return Err(anyhow::anyhow!("Invalid fstab line: {}", line));
        }

        let device_path = PathBuf::from(tokens[0]);
        let mount_path = PathBuf::from(tokens[1]);
        let filesystem = tokens[2].to_owned();
        let options = tokens[3].split(',').map(|s| s.to_owned()).collect();
        let dump = tokens[4].parse::<u32>()?;
        let fsck_pass = tokens[5].parse::<u32>()?;

        Ok(FstabLine {
            device_path,
            mount_path,
            _filesystem: filesystem,
            _options: options,
            _dump: dump,
            _fsck_pass: fsck_pass,
        })
    }

    fn process_line<'a>(
        host_status: &HostStatus,
        host_config: &'a HostConfiguration,
        line: &str,
    ) -> Result<(String, Option<&'a MountPoint>), Error> {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        if tokens.is_empty() || tokens[0].starts_with('#') {
            return Ok((line.to_owned(), None));
        }

        // The first column of /etc/fstab is the device identifier and the second column is the
        // mount point. Thus we match against the second token (index 1 given 0-based indexing)
        // and overwrite the first column with the partition label.
        let mount_dir = tokens[1];

        // Try to find the mount point in HostConfiguration corresponding to the current line
        let it = host_config
            .storage
            .mount_points
            .iter()
            .find(|mp| mp.path.to_str() == Some(mount_dir));
        match it {
            Some(mp) => Ok((
                Self::mount_point_to_fstab_line(host_status, mp, None, None)?,
                Some(mp),
            )),
            None => Ok((line.to_owned(), None)),
        }
    }

    fn mount_point_to_fstab_line(
        host_status: &HostStatus,
        mp: &MountPoint,
        path_prefix: Option<&path::Path>,
        extra_options: Option<Vec<String>>,
    ) -> Result<String, Error> {
        let mount_device_path = get_block_device(host_status, &mp.target_id)
            .context(format!(
                "Failed to find block device with id {}",
                mp.target_id
            ))?
            .path;
        let mount_device_path_str = mount_device_path.to_str().context(format!(
            "Failed to convert mount device path {:?} to string",
            mount_device_path
        ))?;
        let mount_path = match path_prefix {
            Some(prefix) => {
                if mp.path == Path::new("/") {
                    prefix.to_path_buf()
                } else {
                    prefix.join(mp.path.strip_prefix("/")?)
                }
            }
            None => mp.path.clone(),
        };
        let mount_path_str = mount_path.to_str().context(format!(
            "Failed to convert mount path {:?} to string",
            mount_path
        ))?;
        let filesystem = &mp.filesystem;
        let mut options = mp.options.clone();
        // add makefs option to make sure filesystem is created if it does not
        // exist
        // TODO support skipping makefs if we are placing an image onto the
        // partition anyway
        if !options.contains(&"x-systemd.makefs".to_owned()) {
            options.push("x-systemd.makefs".to_owned());
        }
        // TODO extend the fs list
        if !options.contains(&"x-systemd.growfs".to_owned()) && filesystem == "ext4" {
            options.push("x-systemd.growfs".to_owned());
        }
        if let Some(extra_options) = extra_options {
            options.extend(extra_options);
        }
        let options_str = options.join(",");
        let dump = 0;
        let fsck_pass = match mp.path.to_string_lossy().as_ref() {
            "none" => 0, // swap is not checked
            "/" => 1,    // root is checked first
            _ => 2,      // all other filesystems are checked after root
        };

        Ok(format!(
            "{mount_device_path_str} {mount_path_str} {filesystem} {options_str} {dump} {fsck_pass}",
        ))
    }

    pub fn update(
        &mut self,
        host_status: &HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<Self, Error> {
        let mut updated_fstab_lines = Vec::new();
        let mut mount_points: HashMap<&PathBuf, &MountPoint> = host_config
            .storage
            .mount_points
            .iter()
            .map(|mp| (&mp.path, mp))
            .collect();
        for line in &self.fstab_lines {
            let (updated_line, mp) = Self::process_line(host_status, host_config, line)?;
            updated_fstab_lines.push(updated_line);
            if let Some(mp) = mp {
                mount_points.remove(&mp.path);
            }
        }
        // Add new mount points specified in HostConfiguration
        for mp in mount_points.values() {
            let new_line = Self::mount_point_to_fstab_line(host_status, mp, None, None)?;
            updated_fstab_lines.push(new_line);
        }
        Ok(Self {
            fstab_lines: updated_fstab_lines,
        })
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;
    use std::path::{Path, PathBuf};

    use trident_api::{
        config::{HostConfiguration, MountPoint},
        status::HostStatus,
    };

    use crate::modules::storage::fstab::{Fstab, FstabLine};

    /// Validates /etc/fstab line generation logic.
    #[test]
    fn test_mount_point_to_fstab_line() {
        let host_status_yaml = indoc! {r#"
            storage:
                mount-points:
                disks:
                    os:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: null
                        contents: unknown
                        partitions:
                          - id: efi
                            path: /dev/disk/by-partlabel/osp1
                            contents: unknown
                            start: 0
                            end: 0
                            type: esp
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: root
                            path: /dev/disk/by-partlabel/osp2
                            contents: unknown
                            start: 0
                            end: 0
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: home
                            path: /dev/disk/by-partlabel/osp3
                            contents: unknown
                            start: 0
                            end: 0
                            type: home
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: swap
                            path: /dev/disk/by-partlabel/swap
                            contents: unknown
                            start: 0
                            end: 0
                            type: swap
                            uuid: 00000000-0000-0000-0000-000000000000
            imaging:
            reconcile-state: clean-install
        "#};
        let host_status = serde_yaml::from_str::<HostStatus>(host_status_yaml)
            .expect("Failed to parse host status");

        assert_eq!(
            Fstab::mount_point_to_fstab_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/boot/efi"),
                    filesystem: "vfat".to_owned(),
                    options: vec!["umask=0077".to_owned()],
                    target_id: "efi".to_owned(),
                },
                None,
                None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp1 /boot/efi vfat umask=0077,x-systemd.makefs 0 2"
        );

        assert_eq!(
            Fstab::mount_point_to_fstab_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/"),
                    filesystem: "ext4".to_owned(),
                    options: vec!["errors=remount-ro".to_owned()],
                    target_id: "root".to_owned(),
                },
                None,
                None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp2 / ext4 errors=remount-ro,x-systemd.makefs,x-systemd.growfs 0 1"
        );

        assert_eq!(
            Fstab::mount_point_to_fstab_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/"),
                    filesystem: "vfat".to_owned(),
                    options: vec!["errors=remount-ro".to_owned()],
                    target_id: "root".to_owned(),
                },
                None,
                None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp2 / vfat errors=remount-ro,x-systemd.makefs 0 1"
        );

        assert_eq!(
            Fstab::mount_point_to_fstab_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/home"),
                    filesystem: "ext4".to_owned(),
                    options: vec!["defaults".to_owned(), "x-systemd.makefs".to_owned()],
                    target_id: "home".to_owned(),
                },
                None,
                None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp3 /home ext4 defaults,x-systemd.makefs,x-systemd.growfs 0 2"
        );

        assert!(Fstab::mount_point_to_fstab_line(
            &host_status,
            &MountPoint {
                path: PathBuf::from("/random"),
                filesystem: "ext4".to_owned(),
                options: vec![],
                target_id: "foobar".to_owned(),
            },
            None,
            None
        )
        .is_err());

        assert_eq!(
            Fstab::mount_point_to_fstab_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("none"),
                    filesystem: "swap".to_owned(),
                    options: vec!["sw".to_owned()],
                    target_id: "swap".to_owned(),
                },
                None,
                None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/swap none swap sw,x-systemd.makefs 0 0"
        );

        assert!(Fstab::mount_point_to_fstab_line(
            &host_status,
            &MountPoint {
                path: PathBuf::from("none"),
                filesystem: "swap".to_owned(),
                options: vec!["sw".to_owned()],
                target_id: "swap".to_owned(),
            },
            Some(Path::new("/mnt")),
            Some(vec!["foobar".to_owned()])
        )
        .is_err());

        assert_eq!(
            Fstab::mount_point_to_fstab_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/home"),
                    filesystem: "ext4".to_owned(),
                    options: vec!["defaults".to_owned(), "x-systemd.makefs".to_owned()],
                    target_id: "home".to_owned(),
                },
                Some(Path::new("/mnt")),
                Some(vec!["foobar".to_owned()])
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp3 /mnt/home ext4 defaults,x-systemd.makefs,x-systemd.growfs,foobar 0 2"
        );
    }

    /// Validates /etc/fstab update logic which is used to update devices to mount.
    #[test]
    fn test_update_fstab_contents() {
        let input_fstab = indoc! {r#"
            # /etc/fstab: static file system information.
            #
            # <file system> <mount point>   <type>  <options>       <dump>  <pass>
            /dev/sda1 /boot/efi vfat defaults 0 0
            /dev/sda2 / ext4 defaults 0 0
            /dev/sdb1 /random ext4 defaults 0 2
        "#};
        let expected_fstab = indoc! {r#"
            # /etc/fstab: static file system information.
            #
            # <file system> <mount point>   <type>  <options>       <dump>  <pass>
            /dev/disk/by-partlabel/osp1 /boot/efi vfat umask=0077,x-systemd.makefs 0 2
            /dev/disk/by-partlabel/osp2 / ext4 errors=remount-ro,x-systemd.makefs,x-systemd.growfs 0 1
            /dev/sdb1 /random ext4 defaults 0 2
            /dev/disk/by-partlabel/osp3 /home ext4 defaults,x-systemd.makefs,x-systemd.growfs 0 2
        "#};
        let host_config_yaml = indoc! {r#"
            imaging:
                images:
                  - url: file:///path/to/efi-image
                    sha256: 1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
                    format: raw-zstd
                    target-id: efi
                  - url: file:///path/to/root-image
                    sha256: 1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
                    format: raw-zstd
                    target-id: root
            storage:
                disks:
                  - id: os
                    device: /dev/disk/by-bus/foobar
                    partition-table-type: gpt
                    partitions:
                      - id: efi
                        type: esp
                        size: 100MiB
                      - id: root
                        type: root
                        size: 1GiB
                      - id: home
                        type: home
                        size: 10GiB
                mount-points:
                  - path: /boot/efi
                    filesystem: vfat
                    options:
                      - umask=0077
                    target-id: efi
                  - path: /
                    filesystem: ext4
                    options:
                      - errors=remount-ro
                    target-id: root
                  - path: /home
                    filesystem: ext4
                    options:
                      - defaults
                      - x-systemd.makefs
                    target-id: home
        "#};
        let host_config: HostConfiguration =
            serde_yaml::from_str(host_config_yaml).expect("Failed to parse host config");
        let host_status_yaml = indoc! {r#"
            storage:
                mount-points:
                disks:
                    os:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: null
                        contents: unknown
                        partitions:
                          - id: efi
                            path: /dev/disk/by-partlabel/osp1
                            contents: unknown
                            start: 0
                            end: 0
                            type: esp
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: root
                            path: /dev/disk/by-partlabel/osp2
                            contents: unknown
                            start: 0
                            end: 0
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: home
                            path: /dev/disk/by-partlabel/osp3
                            contents: unknown
                            start: 0
                            end: 0
                            type: home
                            uuid: 00000000-0000-0000-0000-000000000000
            imaging:
            reconcile-state: clean-install
        "#};
        let host_status = serde_yaml::from_str::<HostStatus>(host_status_yaml)
            .expect("Failed to parse host status");

        let edited_fstab = Fstab {
            fstab_lines: input_fstab.lines().map(|l| l.to_owned()).collect(),
        }
        .update(&host_status, &host_config)
        .unwrap()
        .fstab_lines
        .join("\n");
        assert_eq!(edited_fstab + "\n", expected_fstab);
    }

    #[test]
    fn test_from_mount_points() {
        let expected_fstab = indoc! {r#"
            /dev/disk/by-partlabel/osp1 /mnt/boot/efi vfat umask=0077,x-systemd.makefs,x-systemd.required-by=update-fs.target,x-systemd.before=update-fs.target 0 2
            /dev/disk/by-partlabel/osp2 /mnt ext4 errors=remount-ro,x-systemd.makefs,x-systemd.growfs,x-systemd.required-by=update-fs.target,x-systemd.before=update-fs.target 0 1
            /dev/disk/by-partlabel/osp3 /mnt/home ext4 defaults,x-systemd.makefs,x-systemd.growfs,x-systemd.required-by=update-fs.target,x-systemd.before=update-fs.target 0 2
        "#};

        let host_config_yaml = indoc! {r#"
            imaging:
                images:
                  - url: file:///path/to/efi-image
                    sha256: 1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
                    format: raw-zstd
                    target-id: efi
                  - url: file:///path/to/root-image
                    sha256: 1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
                    format: raw-zstd
                    target-id: root
            storage:
                disks:
                  - id: os
                    device: /dev/disk/by-bus/foobar
                    partition-table-type: gpt
                    partitions:
                      - id: efi
                        type: esp
                        size: 100MiB
                      - id: root
                        type: root
                        size: 1GiB
                      - id: home
                        type: home
                        size: 10GiB
                mount-points:
                  - path: /boot/efi
                    filesystem: vfat
                    options:
                      - umask=0077
                    target-id: efi
                  - path: /
                    filesystem: ext4
                    options:
                      - errors=remount-ro
                    target-id: root
                  - path: /home
                    filesystem: ext4
                    options:
                      - defaults
                      - x-systemd.makefs
                    target-id: home
        "#};
        let host_config: HostConfiguration =
            serde_yaml::from_str(host_config_yaml).expect("Failed to parse host config");

        let host_status_yaml = indoc! {r#"
            storage:
                mount-points:
                disks:
                    os:
                        path: /dev/disk/by-bus/foobar
                        uuid: 00000000-0000-0000-0000-000000000000
                        capacity: null
                        contents: unknown
                        partitions:
                          - id: efi
                            path: /dev/disk/by-partlabel/osp1
                            contents: unknown
                            start: 0
                            end: 0
                            type: esp
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: root
                            path: /dev/disk/by-partlabel/osp2
                            contents: unknown
                            start: 0
                            end: 0
                            type: root
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: home
                            path: /dev/disk/by-partlabel/osp3
                            contents: unknown
                            start: 0
                            end: 0
                            type: home
                            uuid: 00000000-0000-0000-0000-000000000000
                          - id: swap
                            path: /dev/disk/by-partlabel/swap
                            contents: unknown
                            start: 0
                            end: 0
                            type: swap
                            uuid: 00000000-0000-0000-0000-000000000000
            imaging:
            reconcile-state: clean-install
        "#};
        let host_status = serde_yaml::from_str::<HostStatus>(host_status_yaml)
            .expect("Failed to parse host status");

        assert_eq!(
            Fstab::from_mount_points(
                &host_status,
                &host_config.storage.mount_points,
                Path::new("/mnt"),
                Path::new("update-fs.target")
            )
            .unwrap()
            .fstab_lines
            .join("\n")
                + "\n",
            expected_fstab
        );
    }

    #[test]
    fn test_parse_fstab_line() {
        assert_eq!(
            Fstab::parse_fstab_line("/dev/sda1 /boot/efi vfat defaults 0 0").unwrap(),
            FstabLine {
                device_path: PathBuf::from("/dev/sda1"),
                mount_path: PathBuf::from("/boot/efi"),
                _filesystem: "vfat".to_owned(),
                _options: vec!["defaults".to_owned()],
                _dump: 0,
                _fsck_pass: 0,
            }
        );

        assert_eq!(
            Fstab::parse_fstab_line("/dev/sda2 / ext4 errors=remount-ro 0 0").unwrap(),
            FstabLine {
                device_path: PathBuf::from("/dev/sda2"),
                mount_path: PathBuf::from("/"),
                _filesystem: "ext4".to_owned(),
                _options: vec!["errors=remount-ro".to_owned()],
                _dump: 0,
                _fsck_pass: 0,
            }
        );

        assert_eq!(
            Fstab::parse_fstab_line("/dev/sdb1 /random ext4 defaults,foobar 0 2").unwrap(),
            FstabLine {
                device_path: PathBuf::from("/dev/sdb1"),
                mount_path: PathBuf::from("/random"),
                _filesystem: "ext4".to_owned(),
                _options: vec!["defaults".to_owned(), "foobar".to_owned()],
                _dump: 0,
                _fsck_pass: 2,
            }
        );

        assert!(Fstab::parse_fstab_line("# /dev/sdb1 /random ext4 defaults 0 2").is_err());
    }

    #[test]
    fn test_get() {
        let fstab = Fstab {
            fstab_lines: vec![
                "/dev/sda1 /boot/efi vfat defaults 0 0".to_owned(),
                "/dev/sda2 / ext4 errors=remount-ro 0 0".to_owned(),
                "/dev/sdb1 /random ext4 defaults 0 2".to_owned(),
            ],
        };

        assert_eq!(
            fstab.get(Path::new("/boot/efi")).unwrap(),
            FstabLine {
                device_path: PathBuf::from("/dev/sda1"),
                mount_path: PathBuf::from("/boot/efi"),
                _filesystem: "vfat".to_owned(),
                _options: vec!["defaults".to_owned()],
                _dump: 0,
                _fsck_pass: 0,
            }
        );

        assert_eq!(
            fstab.get(Path::new("/")).unwrap(),
            FstabLine {
                device_path: PathBuf::from("/dev/sda2"),
                mount_path: PathBuf::from("/"),
                _filesystem: "ext4".to_owned(),
                _options: vec!["errors=remount-ro".to_owned()],
                _dump: 0,
                _fsck_pass: 0,
            }
        );

        assert_eq!(
            fstab.get(Path::new("/random")).unwrap(),
            FstabLine {
                device_path: PathBuf::from("/dev/sdb1"),
                mount_path: PathBuf::from("/random"),
                _filesystem: "ext4".to_owned(),
                _options: vec!["defaults".to_owned()],
                _dump: 0,
                _fsck_pass: 2,
            }
        );

        assert!(fstab.get(Path::new("/foobar")).is_none());
    }
}
