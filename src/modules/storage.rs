use anyhow::{bail, Context, Error};
use log::info;
use std::{
    collections::HashMap,
    ffi::OsString,
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};
use uuid::Uuid;

use trident_api::{
    config::HostConfiguration,
    status::{self, HostStatus, UpdateKind},
};

use crate::modules::Module;

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
            .retain(|path, _disk| path.exists());

        Ok(())
    }

    fn validate_host_config(
        &self,
        _host_status: &HostStatus,
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
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
        _host_status: &mut HostStatus,
        _host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        let fstab = fs::read_to_string("/etc/fstab").context("Failed to read /etc/fstab")?;

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
            match tokens.get(1) {
                Some(&"/") => {
                    writeln!(
                        &mut edited_fstab,
                        "PARTLABEL=mariner-root-a {}",
                        &tokens[1..].join(" ")
                    )
                    .unwrap();
                }
                Some(&"/boot/efi") => {
                    writeln!(
                        &mut edited_fstab,
                        "PARTLABEL=mariner-esp {}",
                        &tokens[1..].join(" ")
                    )
                    .unwrap();
                }
                _ => {
                    writeln!(&mut edited_fstab, "{}", line)?;
                }
            }
        }
        fs::write("/etc/fstab", edited_fstab).context("Failed to write new /etc/fstab")?;

        Ok(())
    }
}

impl StorageModule {
    pub fn create_partitions(
        host_status: &mut HostStatus,
        host_config: &HostConfiguration,
    ) -> Result<(), Error> {
        // The commands in this function are run using flock because of past issues with the
        // Mariner toolkit. The commands sometimes would not block when later commands were
        // expecting them to.
        //
        // TODO: Investigate whether this is still necessary.

        for disk in &host_config.storage.disks {
            let disk_path = disk.device.canonicalize().context(format!(
                "Failed to lookup device '{}'",
                disk.device.display()
            ))?;

            let disk_bus_path =
                find_symlink_for_target(&disk_path, Path::new("/dev/disk/by-path")).context(
                    format!("Failed to find bus path of '{}'", disk_path.display()),
                )?;

            run(Command::new("sfdisk")
                .arg("--delete")
                .arg(disk_path.as_os_str()))?;
            run(Command::new("flock")
                .arg("--timeout")
                .arg("5")
                .arg(disk_path.as_os_str())
                .arg("parted")
                .arg(disk_path.as_os_str())
                .arg("--script")
                .arg("mklabel")
                .arg("gpt"))?;

            // set the disk UUID
            let disk_uuid = Uuid::new_v4();
            run(Command::new("flock")
                .arg("--timeout")
                .arg("5")
                .arg(disk_path.as_os_str())
                .arg("sgdisk")
                .arg("--disk-guid")
                .arg(disk_uuid.as_hyphenated().to_string())
                .arg(disk_path.as_os_str()))?;

            host_status.storage.disks.insert(
                disk_path.clone(),
                status::Disk {
                    uuid: disk_uuid,
                    bus_path: disk_bus_path,
                    partitions: Vec::new(),
                    capacity: None,
                },
            );
            let disk_status = host_status.storage.disks.get_mut(&disk_path).unwrap();

            // Allocate partitions in 4KB increments, starting at 4MB to leave space for the
            // partition table.
            let mut start = 4 * 1024 * 1024;
            let mut partition_kind_counts = HashMap::new();
            for (index, partition) in disk.partitions.iter().enumerate() {
                let count = partition_kind_counts
                    .entry(partition.partition_type)
                    .or_insert(0);
                *count += 1;

                let kind = partition.partition_type.to_label_str();
                let name = if *count == 1 {
                    kind.to_owned()
                } else {
                    format!("{kind}{count}")
                };

                let size = parse_size(&partition.size).context(format!(
                    "Failed to parse size ('{}') for partition '{name}'",
                    partition.size
                ))?;
                // Round up to a multiple of 4K
                let size = (size.saturating_sub(1) / 4096 + 1) * 4096;

                // TODO: find a more robust way to determine the physical block size rather than
                // hardcoding 512 bytes.
                run(Command::new("flock")
                    .arg("--timeout")
                    .arg("5")
                    .arg(disk_path.as_os_str())
                    .arg("parted")
                    .arg(disk_path.as_os_str())
                    .arg("--script")
                    .arg("mkpart")
                    .arg(&name)
                    .arg(format!("{start}B"))
                    .arg(format!("{}B", start + size - 512)))?;

                partprobe(&disk_path)?;

                let part_path = device_to_partition(&disk_path, index + 1);
                info!("part_path: {}", part_path.display());

                disk_status.partitions.push(status::Partition {
                    path: part_path,
                    start,
                    end: start + size,
                    ty: partition.partition_type,
                    contents: status::PartitionContents::Unknown,
                });

                start += size;
            }
        }

        Ok(())
    }
}

fn run(command: &mut Command) -> Result<(), Error> {
    let output = command.output()?;
    if !output.status.success() {
        bail!(
            "Command failed: {:?}\n\nstdout:\n{}\n\nstderr:\n{}",
            command,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn partprobe(disk_path: &Path) -> Result<(), Error> {
    run(Command::new("flock")
        .arg("--timeout")
        .arg("5")
        .arg(disk_path.as_os_str())
        .arg("partprobe")
        .arg("-s")
        .arg(disk_path.as_os_str()))
    .context("Failed to probe partitions")
}

fn find_symlink_for_target(target: &Path, directory: &Path) -> Result<OsString, Error> {
    for f in fs::read_dir(directory)?.flatten() {
        if let Ok(target_path) = f.path().canonicalize() {
            if target_path == target {
                return Ok(f.file_name());
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

fn device_to_partition(p: &Path, index: usize) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    if s.to_string_lossy()
        .chars()
        .last()
        .map(|c| c.is_ascii_digit())
        .unwrap_or(false)
    {
        s.push("p");
    }
    s.push(&index.to_string());
    s.into()
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn test_device_to_partition() {
        assert_eq!(
            device_to_partition(Path::new("/dev/sda"), 1),
            Path::new("/dev/sda1")
        );
        assert_eq!(
            device_to_partition(Path::new("/dev/nvme0n1"), 2),
            Path::new("/dev/nvme0n1p2")
        );
    }
}
