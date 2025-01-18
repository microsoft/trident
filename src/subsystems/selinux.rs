use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
    str::FromStr,
};

use anyhow::{bail, Context, Error};

use log::warn;
use osutils::dependencies::{Dependency, DependencyResultExt};
use trident_api::{
    config::{FileSystemType, HostConfigurationDynamicValidationError, SelinuxMode},
    constants::{MOUNT_OPTION_READ_ONLY, SELINUX_CONFIG},
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
    status::ServicingType,
};

use crate::engine::{EngineContext, Subsystem};

/// Gets the SELinux type from the SELinux config file.
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

/// Gets the SELinux mode (enforcing, permissive, disabled) from the SELinux config file.
fn get_selinux_mode(selinux_config_path: impl AsRef<Path>) -> Result<SelinuxMode, Error> {
    let file = File::open(selinux_config_path.as_ref()).with_context(|| {
        format!(
            "Failed to open file '{}'",
            selinux_config_path.as_ref().display()
        )
    })?;
    let reader = BufReader::new(file);

    for line in reader.lines() {
        let line = line?;
        if let Some(selinux_mode) = line.strip_prefix("SELINUX=") {
            return SelinuxMode::from_str(selinux_mode);
        }
    }

    bail!(
        "Could not find SELinux mode in file {}",
        selinux_config_path.as_ref().display()
    );
}

#[derive(Default)]
pub struct SelinuxSubsystem;
impl Subsystem for SelinuxSubsystem {
    fn name(&self) -> &'static str {
        "selinux"
    }

    #[tracing::instrument(name = "selinux_configuration", skip_all)]
    fn configure(&mut self, ctx: &EngineContext, _exec_root: &Path) -> Result<(), TridentError> {
        if let ServicingType::CleanInstall | ServicingType::AbUpdate = ctx.servicing_type {
            // If a verity filesystem is mounted at root, ensure that SELinux is not in enforcing mode and
            // warn if it is in permissive mode
            if !ctx.spec.storage.verity_filesystems.is_empty() {
                // If image does not have SELinux, config file will not exist
                if !Path::new(SELINUX_CONFIG).exists() {
                    return Ok(());
                }

                let selinux_mode =
                    get_selinux_mode(SELINUX_CONFIG).structured(ServicingError::GetSelinuxMode)?;

                match selinux_mode {
                    SelinuxMode::Enforcing => {
                        return Err(TridentError::new(InvalidInputError::from(
                            HostConfigurationDynamicValidationError::VerityAndSelinuxUnsupported {
                                selinux_mode: selinux_mode.to_string(),
                            },
                        )));
                    }
                    SelinuxMode::Permissive => warn!("The use of SELinux with verity is not supported. SELinux mode is currently set to '{}', but should be 'disabled'.", selinux_mode.to_string()),
                    _ => (),
                }
            }

            // Get the mount points for all filesystems, except for vfat and NTFS. These two FS
            // types cannot be used in conjunction with SELinux, so the setfiles command will be
            // skipped for them.
            let mount_paths: Vec<&trident_api::config::MountPoint> = ctx
                .spec
                .storage
                .filesystems
                .iter()
                // Filter out vfat and NTFS filesystems
                .filter(|filesystem| {
                    filesystem.fs_type != FileSystemType::Vfat
                        && filesystem.fs_type != FileSystemType::Ntfs
                })
                // Filter out filesystems that are not mounted
                .filter_map(|filesystem| filesystem.mount_point.as_ref())
                // Filter out read-only mount points
                .filter(|mp| !mp.options.contains(MOUNT_OPTION_READ_ONLY))
                .collect();

            let selinux_type =
                get_selinux_type(SELINUX_CONFIG).structured(ServicingError::GetSelinuxType)?;

            // Get SELinux mode from Host Configuration
            let selinux_mode = ctx.spec.os.selinux.mode;
            match selinux_mode {
                Some(SelinuxMode::Disabled) => return Ok(()),
                Some(SelinuxMode::Permissive) | Some(SelinuxMode::Enforcing) => {
                    // Host Configuration enables SELinux, but OS does not contain SELinux
                    if let Err(e) = Dependency::Setfiles.path() {
                        return Err(TridentError::with_source(
                            InvalidInputError::SelinuxEnabledButNotFound,
                            e.into(),
                        ));
                    }
                }
                None => (),
            }

            // Check if setfiles exists, implicitly checking if SELinux is in OS
            if Dependency::Setfiles.exists() {
                Dependency::Setfiles
                    .cmd()
                    .arg("-m")
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
                    .message("Failed to run setfiles command")?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    use tempfile::NamedTempFile;

    #[test]
    fn test_get_selinux_mode_success_enforcing() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "SELINUX=enforcing").unwrap();

        let selinux_mode = get_selinux_mode(temp_file.path().to_str().unwrap());
        assert_eq!(selinux_mode.unwrap(), SelinuxMode::Enforcing);
    }

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

        let mode = get_selinux_mode(temp_file.path().to_str().unwrap()).unwrap();
        assert!(mode == SelinuxMode::Disabled);
    }

    #[test]
    fn test_get_selinux_type_and_mode_empty_file() {
        let temp_file = NamedTempFile::new().unwrap();

        let result_type = get_selinux_type(temp_file.path().to_str().unwrap());
        assert!(result_type.is_err());

        let result_mode = get_selinux_mode(temp_file.path().to_str().unwrap());
        assert!(result_mode.is_err());
    }

    #[test]
    fn test_get_selinux_type_multiple_entries() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "SELINUX=permissive").unwrap();
        writeln!(temp_file, "SELINUXTYPE=targeted").unwrap();
        writeln!(temp_file, "SELINUXTYPE=strict").unwrap();

        let selinux_type = get_selinux_type(temp_file.path().to_str().unwrap()).unwrap();
        assert_eq!(selinux_type, "targeted");

        let selinux_mode = get_selinux_mode(temp_file.path().to_str().unwrap()).unwrap();
        assert_eq!(selinux_mode, SelinuxMode::Permissive);
    }
}
