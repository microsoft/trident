use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    process::Command,
};

use anyhow::{bail, Error};

use osutils::exe::RunAndCheck;
use trident_api::{
    config::FileSystemType,
    constants::SELINUX_CONFIG,
    error::{ReportError, ServicingError, TridentError},
    status::{HostStatus, ServicingType},
};

use super::Subsystem;

/// Gets the seinux type from the selinux config file.
fn get_selinux_type(selinux_config_path: impl AsRef<Path>) -> Result<String, Error> {
    let file = File::open(selinux_config_path.as_ref())?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        if let Some(selinux_type) = line.strip_prefix("SELINUXTYPE=") {
            return Ok(selinux_type.to_string());
        }
    }

    bail!(
        "Could not find SELinux type in file {}",
        selinux_config_path.as_ref().display()
    );
}

#[derive(Default)]
pub struct SelinuxSubsystem;
impl Subsystem for SelinuxSubsystem {
    fn name(&self) -> &'static str {
        "selinux"
    }

    fn configure(
        &mut self,
        host_status: &HostStatus,
        _exec_root: &Path,
    ) -> Result<(), TridentError> {
        if let ServicingType::CleanInstall | ServicingType::AbUpdate = host_status.servicing_type {
            // Get the mount points for the filesystems that are not of type vfat as setfiles does
            // not support vfat
            let mount_paths: Vec<&trident_api::config::MountPoint> = host_status
                .spec
                .storage
                .filesystems
                .iter()
                .filter(|filesystem| filesystem.fs_type != FileSystemType::Vfat)
                .filter_map(|filesystem| filesystem.mount_point.as_ref())
                .collect();

            let selinux_type =
                get_selinux_type(SELINUX_CONFIG).structured(ServicingError::GetSelinuxType)?;

            Command::new("setfiles")
                .arg("-m")
                .arg("-v")
                .arg(
                    Path::new("/etc/selinux")
                        .join(selinux_type)
                        .join("contexts/files/file_contexts"),
                )
                .args(
                    mount_paths
                        .iter()
                        .map(|mount_point| mount_point.path.as_os_str()),
                )
                .run_and_check()
                .structured(ServicingError::RunSetFiles)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use std::io::Write;

    use tempfile::NamedTempFile;

    #[test]
    fn test_get_selinux_type_success() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "SELINUXTYPE=targeted").unwrap();

        let selinux_type = get_selinux_type(temp_file.path().to_str().unwrap()).unwrap();
        assert_eq!(selinux_type, "targeted");
    }

    #[test]
    fn test_get_selinux_type_no_match() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "SELINUX=disabled").unwrap();

        let result = get_selinux_type(temp_file.path().to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_get_selinux_type_empty_file() {
        let temp_file = NamedTempFile::new().unwrap();

        let result = get_selinux_type(temp_file.path().to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_get_selinux_type_multiple_entries() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "SELINUX=permissive").unwrap();
        writeln!(temp_file, "SELINUXTYPE=targeted").unwrap();
        writeln!(temp_file, "SELINUXTYPE=strict").unwrap();

        let selinux_type = get_selinux_type(temp_file.path().to_str().unwrap()).unwrap();
        assert_eq!(selinux_type, "targeted");
    }
}
