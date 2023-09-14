use anyhow::{bail, Context, Error};
use configparser::ini::Ini;
use log::info;
use serde_json::Value;
use std::{
    collections::HashSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Output},
};
use sys_mount::Mount;
use uuid::Uuid;

use trident_api::{
    config::{HostConfiguration, Partition, PartitionType},
    status::{self, BlockDeviceContents, HostStatus, UpdateKind},
};

use crate::{get_block_device, modules::Module};

#[derive(Default, Debug)]
pub struct StorageModule;
impl Module for StorageModule {
    fn name(&self) -> &'static str {
        "storage"
    }

    fn refresh_host_status(&mut self, host_status: &mut HostStatus) -> Result<(), Error> {
        // Remove disks that no longer exist.
        host_status
            .storage
            .disks
            .retain(|_id, disk| disk.path.exists());

        Ok(())
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        // Ensure block device naming is unique across all supported block
        // device types.
        let mut block_device_ids = std::collections::HashSet::new();

        StorageModule::check_multiple_instances(
            &mut block_device_ids,
            &mut host_config.storage.disks.iter().map(|d| &d.id),
        )?;

        let partition_ids: Vec<&String> = host_config
            .storage
            .disks
            .iter()
            .flat_map(|d| &d.partitions)
            .map(|p| &p.id)
            .collect();
        let partition_ids_set: HashSet<&String> = partition_ids.iter().cloned().collect();
        let mut image_target_ids: HashSet<&String> = partition_ids_set.clone();

        StorageModule::check_multiple_instances(
            &mut block_device_ids,
            &mut partition_ids.clone().into_iter(),
        )?;

        if let Some(ab_update) = &host_config.imaging.ab_update {
            let ab_volume_ids: Vec<&String> =
                ab_update.volume_pairs.iter().map(|v| &v.id).collect();
            image_target_ids.extend(ab_volume_ids.clone());
            StorageModule::check_multiple_instances(
                &mut block_device_ids,
                &mut ab_volume_ids.into_iter(),
            )?;
        }

        // Ensure valid references.
        if let Some(ab_update) = &host_config.imaging.ab_update {
            for p in &ab_update.volume_pairs {
                for block_device_id in [&p.volume_a_id, &p.volume_b_id] {
                    if !partition_ids_set.contains(block_device_id) {
                        bail!(
                            "Block device id '{id}' was set as dependency of an A/B update volume '{parent}', but is not defined elsewhere",
                            id = block_device_id,
                            parent = p.id,
                        );
                    }
                }
            }
        }

        for image in &host_config.imaging.images {
            if !image_target_ids.contains(&image.target_id) {
                bail!(
                    "Block device id '{id}' was set as dependency of an image, but is not defined elsewhere",
                    id = image.target_id,
                );
            }
        }

        for mount_point in &host_config.storage.mount_points {
            if !image_target_ids.contains(&mount_point.target_id) {
                bail!(
                    "Block device id '{id}' was set as dependency of a mount point, but is not defined elsewhere",
                    id = mount_point.target_id,
                );
            }
        }

        // Ensure mutual exclusivity
        if let Some(ab_update) = &host_config.imaging.ab_update {
            for p in &ab_update.volume_pairs {
                if p.volume_a_id == p.volume_b_id {
                    bail!(
                        "A/B update volume id '{id}' has the same target_id both both volumes",
                        id = p.id,
                    );
                }
            }
        }

        Ok(())
    }

    fn select_update_kind(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Option<UpdateKind> {
        Some(UpdateKind::HotPatch)
    }

    fn reconcile(
        &mut self,
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        update_fstab_file(host_status, host_config)

        // TODO: update /etc/repart.d directly for the matching disk, derive
        // from where is the root located
    }
}

pub fn mount_partition(partition: &Path, mount_point: &Path) -> Result<Mount, Error> {
    fs::create_dir_all(mount_point)?;
    info!("Mounting disk");
    Ok(Mount::builder()
        .fstype("ext4")
        .mount(partition, mount_point)?)
}

impl StorageModule {
    fn check_multiple_instances<'a>(
        detected_ids: &mut HashSet<&'a String>,
        input_ids: &mut dyn Iterator<Item = &'a String>,
    ) -> Result<(), Error> {
        for name in input_ids {
            if !detected_ids.insert(name) {
                bail!("Block device name '{name}' is used more than once");
            }
        }

        Ok(())
    }

    fn partition_config_to_repart_config(partition: &Partition) -> Result<Ini, Error> {
        let partition_type_str = partition_type_to_string(&partition.partition_type)?;

        parse_size(&partition.size).context(format!(
            "Failed to parse size ('{}') for partition '{}'",
            partition.size, partition.id
        ))?;

        let mut repart_config = Ini::new_cs();

        let repart_partition_section = "Partition";

        repart_config.set(repart_partition_section, "Type", Some(partition_type_str));
        repart_config.set(
            repart_partition_section,
            "Label",
            Some(partition.id.clone()),
        );

        repart_config.set(
            repart_partition_section,
            "SizeMinBytes",
            Some(partition.size.clone()),
        );
        repart_config.set(
            repart_partition_section,
            "SizeMaxBytes",
            Some(partition.size.clone()),
        );

        Ok(repart_config)
    }

    pub fn create_partitions(
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        for disk in &host_config.storage.disks {
            let disk_path = disk.device.canonicalize().context(format!(
                "Failed to lookup device '{}'",
                disk.device.display()
            ))?;

            let disk_bus_path =
                find_symlink_for_target(&disk_path, Path::new("/dev/disk/by-path")).context(
                    format!("Failed to find bus path of '{}'", disk_path.display()),
                )?;

            let repart_root = tempfile::tempdir()
                .context("Failed to create temporary directory for systemd-repart files")?;
            let repart_config_path = repart_root.path().join(&disk.id);
            info!(
                "Generating systemd-repart configuration in {}",
                repart_root.path().to_string_lossy()
            );
            generate_repart_config(&repart_config_path, disk)?;

            let repart_output_json = run(Command::new("systemd-repart")
                .arg(disk_bus_path.as_os_str())
                .arg("--dry-run=no")
                .arg("--empty=force")
                .arg("--seed=random")
                .arg("--json=short")
                .arg("--definitions")
                .arg(repart_config_path))
            .context(format!("Failed to initialize disk {}", disk.id))?;
            let partitions_status: Value = serde_json::from_slice(&repart_output_json.stdout)
                .context("Failed to deserialize output of disk initialization command")?;

            let sfdisk_output_json = run(Command::new("sfdisk")
                .arg("-J")
                .arg(disk_bus_path.as_os_str()))
            .context(format!(
                "Failed to fetch disk GPT UUID for disk {}",
                disk.id
            ))?;
            let disk_status: Value = serde_json::from_slice(&sfdisk_output_json.stdout)
                .context("Failed to deserialize output of disk status querying command")?;
            let disk_uuid_str = disk_status["partitiontable"]["id"]
                .as_str()
                .context(format!("Failed to find GPT UUID for disk {}", disk.id))?;
            let disk_uuid = Uuid::parse_str(disk_uuid_str)
                .context(format!("Failed to parse disk UUID: '{}'", disk_uuid_str))?;

            host_status.storage.disks.insert(
                disk.id.clone(),
                status::Disk {
                    uuid: disk_uuid,
                    path: disk_bus_path.clone(),
                    partitions: Vec::new(),
                    capacity: None,
                    contents: BlockDeviceContents::Unknown,
                },
            );

            let disk_status = host_status.storage.disks.get_mut(&disk.id).unwrap();

            // ensure all /dev/disk/* symlinks are created
            run(Command::new("udevadm").arg("settle"))?;

            for (index, partition) in disk.partitions.iter().enumerate() {
                let partition_uuid_str =
                    partitions_status[index]["uuid"].as_str().context(format!(
                        "Failed to find UUID for partition {} on disk {}",
                        partition.id, disk.id
                    ))?;
                let part_path = Path::new("/dev/disk/by-partuuid").join(partition_uuid_str);
                info!("part_path: {}", part_path.display());

                let start = partitions_status[index]["offset"]
                    .as_u64()
                    .context(format!(
                        "Failed to find start offset for partition {} on disk {}",
                        partition.id, disk.id
                    ))?;
                let size = partitions_status[index]["raw_size"]
                    .as_u64()
                    .context(format!(
                        "Failed to find size for partition {} on disk {}",
                        partition.id, disk.id
                    ))?;
                disk_status.partitions.push(status::Partition {
                    id: partition.id.clone(),
                    path: part_path,
                    start,
                    end: start + size,
                    ty: partition.partition_type,
                    contents: BlockDeviceContents::Unknown,
                    uuid: Uuid::parse_str(partition_uuid_str)?,
                });
            }

            disk_status.contents = status::BlockDeviceContents::Initialized;
        }

        Ok(())
    }
}

fn generate_repart_config(
    repart_config_path: &PathBuf,
    disk: &trident_api::config::Disk,
) -> Result<(), Error> {
    if disk.partitions.len() >= 100 {
        bail!(
            "Too many partitions ({}), maximum is 99",
            disk.partitions.len()
        );
    }

    fs::create_dir_all(repart_config_path).context(format!(
        "Failed to create {}",
        repart_config_path.to_string_lossy()
    ))?;

    for (index, partition) in disk.partitions.iter().enumerate() {
        parse_size(&partition.size).context(format!(
            "Failed to parse size ('{}') for partition '{}'",
            partition.size, partition.id
        ))?;

        let repart_config =
            StorageModule::partition_config_to_repart_config(partition).context(format!(
                "Failed to generate partition configuration for partition {} on disk {}",
                partition.id, disk.id
            ))?;

        let partition_config_path = repart_config_path.join(format!(
            "{:02}-{}.conf",
            index,
            partition_type_to_string(&partition.partition_type)?
        ));

        repart_config
            .write(&partition_config_path)
            .context(format!(
                "Failed to create partition configuration file {}",
                partition_config_path.to_string_lossy()
            ))?;
    }

    Ok(())
}

fn partition_type_to_string(partition_type: &PartitionType) -> Result<String, Error> {
    Ok(serde_json::to_value(partition_type)?
        .as_str()
        .unwrap()
        .to_owned())
}

fn run(command: &mut Command) -> Result<Output, Error> {
    let output = command.output()?;
    if !output.status.success() {
        bail!(
            "Command failed: {:?}\n\nstdout:\n{}\n\nstderr:\n{}",
            command,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output)
}

/// Returns the path of the first symlink in directory whose canonical path is target.
/// Requires that target is already a canonical path.
fn find_symlink_for_target(target: &Path, directory: &Path) -> Result<PathBuf, Error> {
    for f in fs::read_dir(directory)?.flatten() {
        if let Ok(target_path) = f.path().canonicalize() {
            if target_path == target {
                return Ok(f.path());
            }
        }
    }

    bail!("Failed to find symlink for '{}'", target.display())
}

fn parse_size(value: &str) -> Result<u64, Error> {
    Ok(if let Some(n) = value.strip_suffix('K') {
        n.parse::<u64>()? << 10
    } else if let Some(n) = value.strip_suffix('M') {
        n.parse::<u64>()? << 20
    } else if let Some(n) = value.strip_suffix('G') {
        n.parse::<u64>()? << 30
    } else if let Some(n) = value.strip_suffix('T') {
        n.parse::<u64>()? << 40
    } else {
        value.parse()?
    })
}

fn update_fstab_file(
    host_status: &HostStatus,
    host_config: &HostConfiguration,
) -> Result<(), Error> {
    let fstab = fs::read_to_string("/etc/fstab").context("Failed to read /etc/fstab")?;

    let edited_fstab = update_fstab_contents(fstab, host_config, host_status)?;
    fs::write("/etc/fstab", edited_fstab).context("Failed to write new /etc/fstab")?;

    Ok(())
}

fn update_fstab_contents(
    fstab: String,
    host_config: &HostConfiguration,
    host_status: &HostStatus,
) -> Result<Vec<u8>, Error> {
    let mut edited_fstab = Vec::new();
    for line in fstab.lines() {
        let tokens = line.split_whitespace().collect::<Vec<_>>();
        if tokens.is_empty() || tokens[0].starts_with('#') {
            writeln!(&mut edited_fstab, "{}", line).unwrap();
            continue;
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
            Some(mp) => {
                writeln!(
                    &mut edited_fstab,
                    "{} {}",
                    get_block_device(host_status, &mp.target_id)
                        .unwrap()
                        .path
                        .to_str()
                        .unwrap(),
                    &tokens[1..].join(" "), // TODO use values from HostConfiguration
                )
                .unwrap();
                continue;
            }
            None => {
                writeln!(&mut edited_fstab, "{}", line)?;
            }
        }
    }
    Ok(edited_fstab)
}

#[cfg(test)]
mod tests {
    use indoc::indoc;
    use trident_api::config::{HostConfiguration, Partition, PartitionType};

    use super::*;

    #[test]
    fn test_parse_size() {
        assert_eq!(parse_size("1").unwrap(), 1);
        assert_eq!(parse_size("1K").unwrap(), 1024);
        assert_eq!(parse_size("1M").unwrap(), 1024 * 1024);
        assert_eq!(parse_size("12G").unwrap(), 12 * 1024 * 1024 * 1024);
        assert_eq!(parse_size("321T").unwrap(), 321 * 1024 * 1024 * 1024 * 1024);

        assert!(parse_size("1Z").is_err());
        assert!(parse_size("abc").is_err());
        assert!(parse_size("T1").is_err());
        assert!(parse_size("-3").is_err());
        assert!(parse_size("0x23K").is_err());
    }

    /// Validates Storage module HostConfiguration validation logic.
    #[test]
    fn test_validate_host_config() {
        let empty_host_config_yaml = indoc! {r#"
            storage:
                disks:
            imaging:
                images:
        "#};
        let empty_host_config = serde_yaml::from_str::<HostConfiguration>(empty_host_config_yaml)
            .expect("Failed to parse empty host config");

        let empty_host_status_yaml = indoc! {r#"
            reconcile-state: clean-install
            storage:
                disks:
            imaging:
                image-deployment:
        "#};
        let empty_host_status = serde_yaml::from_str(empty_host_status_yaml)
            .expect("Failed to parse empty host status");

        let storage_module = StorageModule::default();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &empty_host_config)
            .is_ok());

        let host_config_yaml = indoc! {r#"
            storage:
                disks:
                  - id: disk1
                    device: /dev/sda
                    partition-table-type: gpt
                    partitions:
                  - id: disk2
                    device: /dev/sdb
                    partition-table-type: gpt
                    partitions:
                      - id: part1
                        type: esp
                        size: 1M
                      - id: part2
                        type: root
                        size: 1G
                mount-points:
                  - filesystem: ext4
                    options: []
                    target-id: part1
                    path: /
            imaging:
                images:
                  - target-id: part1
                    url: ""
                    sha256: ""
                    format: raw-zstd
                ab-update:
                    volume-pairs:
                      - id: ab1
                        volume-a-id: part1
                        volume-b-id: part2
        "#};
        let mut host_config = serde_yaml::from_str::<HostConfiguration>(host_config_yaml)
            .expect("Failed to parse host config");

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_ok());

        let host_config_golden = host_config.clone();

        // fail on duplicate id
        host_config.storage.disks.get_mut(0).unwrap().partitions = vec![Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::Esp,
            size: "1M".to_owned(),
        }];

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on duplicate id
        host_config.imaging.ab_update.as_mut().unwrap().volume_pairs[0].id = "disk1".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on missing reference (disk4 does not exist)
        host_config.imaging.ab_update.as_mut().unwrap().volume_pairs[0].volume_a_id =
            "disk4".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on missing reference (disk4 does not exist)
        host_config.imaging.images[0].target_id = "disk4".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on missing reference (disk4 does not exist)
        host_config.storage.mount_points[0].target_id = "disk4".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());

        host_config = host_config_golden.clone();

        // fail on bad block device type
        host_config.imaging.images[0].target_id = "disk1".to_owned();

        assert!(storage_module
            .validate_host_config(&empty_host_status, &host_config)
            .is_err());
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
        "#};
        let expected_fstab = indoc! {r#"
            # /etc/fstab: static file system information.
            #
            # <file system> <mount point>   <type>  <options>       <dump>  <pass>
            /dev/disk/by-partlabel/osp1 /boot/efi vfat defaults 0 0
            /dev/disk/by-partlabel/osp2 / ext4 defaults 0 0
        "#};
        let host_config_yaml = indoc! {r#"
            imaging:
                images:
                  - url: file:///path/to/image
                    sha256: 1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef
                    format: raw-zstd
                    target-id: efi
                  - url: file:///path/to/image
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
                mount-points:
                  - path: /boot/efi
                    filesystem: vfat
                    options:
                      - defaults
                    target-id: efi
                  - path: /
                    filesystem: ext4
                    options:
                      - defaults
                    target-id: root
        "#};
        let host_config: HostConfiguration =
            serde_yaml::from_str(host_config_yaml).expect("Failed to parse host config");
        let host_status_yaml = indoc! {r#"
            storage:
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
            imaging:
                image-deployment:
                    efi:
                        url: file:///path/to/image
                    root:
                        url: file:///path/to/image
            reconcile-state: clean-install
        "#};
        let host_status = serde_yaml::from_str::<HostStatus>(host_status_yaml)
            .expect("Failed to parse host status");

        let edited_fstab =
            update_fstab_contents(input_fstab.to_string(), &host_config, &host_status)
                .unwrap()
                .into_iter()
                .map(|b| b as char)
                .collect::<String>();
        assert_eq!(edited_fstab, expected_fstab);
    }

    /// Validates that partition_type_to_string returns the correct string for each PartitionType.
    #[test]
    fn test_partition_type_to_string() {
        assert_eq!(
            partition_type_to_string(&PartitionType::Esp).unwrap(),
            "esp"
        );
        assert_eq!(
            partition_type_to_string(&PartitionType::Root).unwrap(),
            "root"
        );
        assert_eq!(
            partition_type_to_string(&PartitionType::RootVerity).unwrap(),
            "root-verity"
        );
        assert_eq!(
            partition_type_to_string(&PartitionType::Swap).unwrap(),
            "swap"
        );
    }

    /// Validates that partition_config_to_repart_config returns the correct Ini for each Partition.
    #[test]
    fn test_partition_config_to_repart_config() {
        let partition = Partition {
            id: "part1".to_owned(),
            partition_type: PartitionType::Esp,
            size: "1M".to_owned(),
        };
        let repart_config = StorageModule::partition_config_to_repart_config(&partition).unwrap();
        assert_eq!(
            repart_config.get("Partition", "Type").unwrap(),
            "esp".to_owned()
        );
        assert_eq!(
            repart_config.get("Partition", "Label").unwrap(),
            "part1".to_owned()
        );
        assert_eq!(
            repart_config.get("Partition", "SizeMinBytes").unwrap(),
            "1M".to_owned()
        );
        assert_eq!(
            repart_config.get("Partition", "SizeMaxBytes").unwrap(),
            "1M".to_owned()
        );
    }
}
