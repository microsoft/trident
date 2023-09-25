use std::{
    collections::HashMap,
    fs,
    path::{self, Path, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Error};
use serde_json::Value;

use trident_api::{config::MountPoint, status::HostStatus};

use crate::{get_block_device, run_command};

pub(crate) struct TabFile {
    tab_file_contents: String,
}

pub const DEFAULT_FSTAB_PATH: &str = "/etc/fstab";

impl TabFile {
    pub fn from_mount_points(
        host_status: &HostStatus,
        mount_points: &Vec<MountPoint>,
        path_prefix: Option<&path::Path>,
        required_by: Option<&path::Path>,
    ) -> Result<Self, Error> {
        let mut tab_file_lines = Vec::new();
        let extra_options = required_by
            .map(|rb| {
                rb.to_str()
                    .context(format!("Failed to convert path {:?} to string", rb))
            })
            .transpose()?
            .map(|rb| {
                vec![
                    "x-systemd.required-by=".to_owned() + rb,
                    "x-systemd.before=".to_owned() + rb,
                ]
            });
        for mp in mount_points {
            if mp.path.starts_with("/") {
                let tab_file_line =
                    Self::mount_point_to_line(host_status, mp, &path_prefix, &extra_options)?;
                tab_file_lines.push(tab_file_line);
            } else if path_prefix.is_none() {
                tab_file_lines.push(Self::mount_point_to_line(host_status, mp, &None, &None)?);
            }
        }
        Ok(Self {
            tab_file_contents: tab_file_lines.join("\n"),
        })
    }

    pub fn write(&self, tab_file_path: &Path) -> Result<(), Error> {
        fs::write(tab_file_path, self.tab_file_contents.as_bytes())
            .context(format!("Failed to write new {}", tab_file_path.display()))?;
        Ok(())
    }

    pub fn get_device_path(tab_file_path: &Path, path: &Path) -> Result<PathBuf, Error> {
        let findmnt_output_json = run_command(
            Command::new("findmnt")
                .arg("--tab-file")
                .arg(tab_file_path)
                .arg("--json")
                .arg("--output")
                .arg("source,target,fstype,vfs-options,fs-options,freq,passno")
                .arg("--mountpoint")
                .arg(path),
        )
        .context(format!("Failed to load {:?}", tab_file_path))?;
        let map = parse_findmnt_output(findmnt_output_json.stdout.as_slice())?;
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

    fn mount_point_to_line(
        host_status: &HostStatus,
        mp: &MountPoint,
        path_prefix: &Option<&path::Path>,
        extra_options: &Option<Vec<String>>,
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
            options.extend(extra_options.iter().cloned());
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
}

fn parse_findmnt_output(findmnt_output: &[u8]) -> Result<HashMap<PathBuf, PathBuf>, Error> {
    let payload: Value = serde_json::from_slice(findmnt_output)
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
    use indoc::indoc;
    use std::{
        collections::HashMap,
        io::Write,
        path::{Path, PathBuf},
    };
    use tempfile::NamedTempFile;

    use trident_api::{
        config::{HostConfiguration, MountPoint},
        status::HostStatus,
    };

    use crate::modules::storage::tabfile::TabFile;

    /// Validates /etc/fstab line generation logic.
    #[test]
    fn test_mount_point_to_line() {
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
            TabFile::mount_point_to_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/boot/efi"),
                    filesystem: "vfat".to_owned(),
                    options: vec!["umask=0077".to_owned()],
                    target_id: "efi".to_owned(),
                },
                &None,
                &None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp1 /boot/efi vfat umask=0077,x-systemd.makefs 0 2"
        );

        assert_eq!(
            TabFile::mount_point_to_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/"),
                    filesystem: "ext4".to_owned(),
                    options: vec!["errors=remount-ro".to_owned()],
                    target_id: "root".to_owned(),
                },
                &None,
                &None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp2 / ext4 errors=remount-ro,x-systemd.makefs,x-systemd.growfs 0 1"
        );

        assert_eq!(
            TabFile::mount_point_to_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/"),
                    filesystem: "vfat".to_owned(),
                    options: vec!["errors=remount-ro".to_owned()],
                    target_id: "root".to_owned(),
                },
                &None,
                &None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp2 / vfat errors=remount-ro,x-systemd.makefs 0 1"
        );

        assert_eq!(
            TabFile::mount_point_to_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/home"),
                    filesystem: "ext4".to_owned(),
                    options: vec!["defaults".to_owned(), "x-systemd.makefs".to_owned()],
                    target_id: "home".to_owned(),
                },
                &None,
                &None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp3 /home ext4 defaults,x-systemd.makefs,x-systemd.growfs 0 2"
        );

        assert!(TabFile::mount_point_to_line(
            &host_status,
            &MountPoint {
                path: PathBuf::from("/random"),
                filesystem: "ext4".to_owned(),
                options: vec![],
                target_id: "foobar".to_owned(),
            },
            &None,
            &None
        )
        .is_err());

        assert_eq!(
            TabFile::mount_point_to_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("none"),
                    filesystem: "swap".to_owned(),
                    options: vec!["sw".to_owned()],
                    target_id: "swap".to_owned(),
                },
                &None,
                &None
            )
            .unwrap(),
            "/dev/disk/by-partlabel/swap none swap sw,x-systemd.makefs 0 0"
        );

        assert!(TabFile::mount_point_to_line(
            &host_status,
            &MountPoint {
                path: PathBuf::from("none"),
                filesystem: "swap".to_owned(),
                options: vec!["sw".to_owned()],
                target_id: "swap".to_owned(),
            },
            &Some(Path::new("/mnt")),
            &Some(vec!["foobar".to_owned()])
        )
        .is_err());

        assert_eq!(
            TabFile::mount_point_to_line(
                &host_status,
                &MountPoint {
                    path: PathBuf::from("/home"),
                    filesystem: "ext4".to_owned(),
                    options: vec!["defaults".to_owned(), "x-systemd.makefs".to_owned()],
                    target_id: "home".to_owned(),
                },
                &Some(Path::new("/mnt")),
                &Some(vec!["foobar".to_owned()])
            )
            .unwrap(),
            "/dev/disk/by-partlabel/osp3 /mnt/home ext4 defaults,x-systemd.makefs,x-systemd.growfs,foobar 0 2"
        );
    }

    #[test]
    fn test_from_mount_points() {
        let expected_fstab = indoc! {r#"
            /dev/disk/by-partlabel/osp1 /mnt/boot/efi vfat umask=0077,x-systemd.makefs,x-systemd.required-by=update-fs.target,x-systemd.before=update-fs.target 0 2
            /dev/disk/by-partlabel/osp2 /mnt ext4 errors=remount-ro,x-systemd.makefs,x-systemd.growfs,x-systemd.required-by=update-fs.target,x-systemd.before=update-fs.target 0 1
            /dev/disk/by-partlabel/osp3 /mnt/home ext4 defaults,x-systemd.makefs,x-systemd.growfs,x-systemd.required-by=update-fs.target,x-systemd.before=update-fs.target 0 2
        "#};

        let expected_fstab2 = indoc! {r#"
            /dev/disk/by-partlabel/osp1 /boot/efi vfat umask=0077,x-systemd.makefs 0 2
            /dev/disk/by-partlabel/osp2 / ext4 errors=remount-ro,x-systemd.makefs,x-systemd.growfs 0 1
            /dev/disk/by-partlabel/osp3 /home ext4 defaults,x-systemd.makefs,x-systemd.growfs 0 2
            /dev/disk/by-partlabel/swap none swap sw,x-systemd.makefs 0 0
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
                      - id: swap
                        type: swap
                        size: 1GiB
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
                  - path: none
                    filesystem: swap
                    options:
                      - sw
                    target-id: swap
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
            TabFile::from_mount_points(
                &host_status,
                &host_config.storage.mount_points,
                Some(Path::new("/mnt")),
                Some(Path::new("update-fs.target"))
            )
            .unwrap()
            .tab_file_contents
                + "\n",
            expected_fstab
        );

        assert_eq!(
            TabFile::from_mount_points(&host_status, &host_config.storage.mount_points, None, None)
                .unwrap()
                .tab_file_contents
                + "\n",
            expected_fstab2
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
            TabFile::get_device_path(tmpfile.path(), Path::new("/boot/efi")).unwrap(),
            PathBuf::from("/dev/sda1")
        );

        assert_eq!(
            TabFile::get_device_path(tmpfile.path(), Path::new("/")).unwrap(),
            PathBuf::from("/dev/sda2")
        );

        assert_eq!(
            TabFile::get_device_path(tmpfile.path(), Path::new("/random")).unwrap(),
            PathBuf::from("/dev/sdb1")
        );

        // non-existing mount point
        assert!(TabFile::get_device_path(tmpfile.path(), Path::new("/foobar")).is_err());

        // non-existing input file
        assert!(
            TabFile::get_device_path(Path::new("/does-not-exist"), Path::new("/foobar")).is_err()
        );

        let mut tmpfile = NamedTempFile::new().unwrap();
        tmpfile.write_all("malformed".as_bytes()).unwrap();
        tmpfile.flush().unwrap();

        // malformed input file
        assert!(TabFile::get_device_path(tmpfile.path(), Path::new("/foobar")).is_err());
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
        assert_eq!(
            super::parse_findmnt_output(input.as_bytes()).unwrap(),
            output
        );

        // missing target
        let input = r#"{"filesystems": [{"source":"foo"}]}"#;
        assert!(super::parse_findmnt_output(input.as_bytes()).is_err());

        // missing source
        let input = r#"{"filesystems": [{"target":"foo"}]}"#;
        assert!(super::parse_findmnt_output(input.as_bytes()).is_err());

        // missing target and source
        let input = r#"{"filesystems": [{"foo":"foo"}]}"#;
        assert!(super::parse_findmnt_output(input.as_bytes()).is_err());

        let input = r#"{"filesystems": []}"#;
        assert!(super::parse_findmnt_output(input.as_bytes())
            .unwrap()
            .is_empty());

        let input = r#"{"filesystems": [{"source":"foo","target":"bar"},{"source":"foo2","target":"bar2"}]}"#;
        assert_eq!(
            super::parse_findmnt_output(input.as_bytes()).unwrap().len(),
            2
        );

        // no filesystems
        let input = r#"{"foo": []}"#;
        assert!(super::parse_findmnt_output(input.as_bytes()).is_err());

        // filesystems is not an array
        let input = r#"{"filesystems": {"foo": "bar"}}"#;
        assert!(super::parse_findmnt_output(input.as_bytes()).is_err());

        // one entry is malformed
        let input = r#"{"filesystems": [{"source":"foo","target":"bar"},{"sourcssse":"foo2","target":"bar"},{"source":"foo2","target":"bar"}]}"#;
        assert!(super::parse_findmnt_output(input.as_bytes()).is_err());
    }
}
