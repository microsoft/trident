use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{bail, Context, Error};
use log::warn;

use osutils::dependencies::{Dependency, DependencyResultExt};

use sysdefs::filesystems::{KernelFilesystemType, RealFilesystemType};
use trident_api::{
    config::{HostConfigurationDynamicValidationError, SelinuxMode},
    constants::SELINUX_CONFIG,
    error::{InvalidInputError, ReportError, ServicingError, TridentError},
    status::ServicingType,
};

use crate::engine::{EngineContext, Subsystem};

/// List of filesystems that support SELinux. Based on
/// https://wiki.gentoo.org/wiki/SELinux/FAQ#Can_I_use_SELinux_with_any_file_system.3F
///
/// For simplicity and stability only the intersection of that set and the set
/// of filesystems allowed in the API is included in this list.
const SELINUX_SUPPORTED_FILESYSTEMS: &[RealFilesystemType] =
    &[RealFilesystemType::Ext4, RealFilesystemType::Xfs];

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
    fn configure(&mut self, ctx: &EngineContext) -> Result<(), TridentError> {
        // Only continue if the servicing type is a clean install or AB update.
        if !(ctx.servicing_type == ServicingType::CleanInstall
            || ctx.servicing_type == ServicingType::AbUpdate)
        {
            return Ok(());
        }

        let hc_selinux_state = ctx.spec.os.selinux.mode;

        // Try to get the OS's SELinux mode when the file exists. A None value
        // indicates that the OS does not have a SELinux config file.
        let os_selinux_state = Path::new(SELINUX_CONFIG)
            .exists()
            .then(|| get_selinux_mode(SELINUX_CONFIG).structured(ServicingError::GetSelinuxMode))
            .transpose()?;

        // Get the final SELinux state based on the Host Configuration and OS
        // states. Return an error for invalid states.
        let final_selinux_state = calculate_final_selinux_state(hc_selinux_state, os_selinux_state)
            .structured(InvalidInputError::SelinuxEnabledButNotFound(format!(
                "'{SELINUX_CONFIG}' not found"
            )))?;

        // If the final SELinux state is not present, return early, no
        // relabeling is necessary.
        let Some(final_selinux_mode) = final_selinux_state else {
            return Ok(());
        };

        // If the final SELinux state is disabled, return early, no relabeling
        // is necessary.
        if final_selinux_mode == SelinuxMode::Disabled {
            return Ok(());
        }

        // If we're relabeling, ensure that the setfiles binary exists.
        Dependency::Setfiles
            .path()
            .structured(InvalidInputError::SelinuxEnabledButNotFound(format!(
                "'{}' binary not found",
                Dependency::Setfiles
            )))?;

        // If a verity filesystem is mounted at root, ensure that SELinux is not
        // in enforcing mode and warn if it is in permissive mode
        if ctx.storage_graph.root_fs_is_verity() && !ctx.is_uki()? {
            match final_selinux_mode {
                SelinuxMode::Enforcing => {
                    return Err(TridentError::new(InvalidInputError::from(
                        HostConfigurationDynamicValidationError::RootVerityAndSelinuxUnsupported {
                            selinux_mode: final_selinux_mode.to_string(),
                        },
                    )));
                }
                SelinuxMode::Permissive => warn!(
                    "The use of SELinux with verity is not supported. SELinux mode is currently \
                set to '{}', but should be 'disabled'.",
                    final_selinux_mode.to_string()
                ),
                _ => (),
            }
        }

        perform_relabel(ctx)
    }
}

/// Returns the resulting state of SELinux given the HC and the OS states.
///
/// The resulting state is determined by the following table, where the rows
/// represent the HC state and the columns represent the OS state:0
///
/// | HC \ OS       | NOT PRESENT | DISABLED  | PERMISSIVE | ENFORCING |
/// |---------------|-------------|-----------|------------|-----------|
/// | NOT PRESENT   | NOT PRESENT | DISABLED  | PERMISSIVE | ENFORCING |
/// | DISABLED      | NOT PRESENT | DISABLED  | DISABLED   | DISABLED  |
/// | PERMISSIVE    | Error       | PERMISSIVE| PERMISSIVE | PERMISSIVE|
/// | ENFORCING     | Error       | ENFORCING | ENFORCING  | ENFORCING |
///
/// In code, states are represented as `Option<SelinuxMode>`, where:
/// - `None` represents the state not being present.
/// - `Some(SelinuxMode::Disabled)` represents the state being disabled.
/// - `Some(SelinuxMode::Permissive)` represents the state being permissive.
/// - `Some(SelinuxMode::Enforcing)` represents the state being enforcing.
///
fn calculate_final_selinux_state(
    hc_selinux_mode: Option<SelinuxMode>,
    os_selinux_mode: Option<SelinuxMode>,
) -> Result<Option<SelinuxMode>, Error> {
    Ok(match (hc_selinux_mode, os_selinux_mode) {
        // When the HC is not present, the state is the same as the OS. (First
        // row in the table)
        (None, os_mode) => os_mode,

        // If the Host Configuration disables SELinux, the resulting state is
        // not present or disabled. (Second row in the table)
        (Some(SelinuxMode::Disabled), None) => None,
        (Some(SelinuxMode::Disabled), _) => Some(SelinuxMode::Disabled),

        // If the Host Configuration enables SELinux, but the OS does not
        // have a SELinux config file, return an error.
        (Some(SelinuxMode::Permissive), None) | (Some(SelinuxMode::Enforcing), None) => {
            bail!(
                "SELinux is enabled in the Host Configuration, but the OS does not have SELinux capabilities"
            );
        }

        // For all other cases, the resulting state is the same as the Host Configuration.
        (Some(mode), _) => Some(mode),
    })
}

/// Runs the setfiles command to relabel the required filesystems.
fn perform_relabel(ctx: &EngineContext) -> Result<(), TridentError> {
    let selinux_type =
        get_selinux_type(SELINUX_CONFIG).structured(ServicingError::GetSelinuxType)?;

    Dependency::Setfiles
        .cmd()
        .arg("-m")
        .arg(
            Path::new("/etc/selinux")
                .join(selinux_type)
                .join("contexts/files/file_contexts"),
        )
        .args(filesystems_to_relabel(ctx)?)
        .run_and_check()
        .message("Failed to run setfiles command")
}

/// Returns a list of mount points of the filesystems that need to be relabeled.
///
/// This function reads the filesystems from the Host Configuration and filters
/// them based on the following criteria:
///
/// - The filesystem supports SELinux.
/// - The filesystem is mounted.
/// - The filesystem is not read-only.
///
fn filesystems_to_relabel(ctx: &EngineContext) -> Result<Vec<PathBuf>, TridentError> {
    let mut out = Vec::new();

    for filesystem in &ctx.filesystems {
        // Filter to only Real filesystems
        let Some(KernelFilesystemType::Real(fs_type)) = &filesystem.fs_type() else {
            continue;
        };

        // Filter to ONLY filesystems that support SELinux
        if !SELINUX_SUPPORTED_FILESYSTEMS.contains(fs_type) {
            continue;
        }

        // Only consider mounted filesystems
        let Some(mount_point_path) = filesystem.mount_point_path() else {
            continue;
        };

        // Skip read-only filesystems
        if filesystem.is_read_only() {
            continue;
        }

        out.push(mount_point_path.to_path_buf());
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{io::Write, path::PathBuf};

    use strum::IntoEnumIterator;
    use uuid::Uuid;

    use sysdefs::{osuuid::OsUuid, partition_types::DiscoverablePartitionType};
    use tempfile::NamedTempFile;

    use trident_api::{
        config::{FileSystem, FileSystemSource, MountOptions, MountPoint, NewFileSystemType},
        constants::MOUNT_OPTION_READ_ONLY,
    };

    use crate::{
        engine::filesystem::{FileSystemData, FileSystemDataImage},
        osimage::{
            mock::{MockImage, MockOsImage},
            OsImage, OsImageFileSystemType,
        },
    };

    #[test]
    fn test_filesystems_to_relabel() {
        let mut ctx = EngineContext::default();
        ctx.populate_filesystems().unwrap();
        // Empty filesystems
        assert_eq!(filesystems_to_relabel(&ctx).unwrap(), Vec::<&Path>::new());

        // Filesystem with supported type
        let good_mtp: &str = "/mnt/thing1";
        let good_fs = FileSystem {
            device_id: Some("sda1".into()),
            mount_point: Some(MountPoint::from_str(good_mtp).unwrap()),
            source: Default::default(),
        };
        let good_img = MockOsImage::new().with_image(MockImage {
            mount_point: good_mtp.into(),
            fs_type: OsImageFileSystemType::Ext4,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::LinuxGeneric,
            verity: None,
        });

        let reset = |ctx: &mut EngineContext| {
            ctx.spec.storage.filesystems = vec![good_fs.clone()];
            ctx.image = Some(OsImage::mock(good_img.clone()));
        };

        // Filesystem with supported type
        reset(&mut ctx);
        ctx.populate_filesystems().unwrap();
        assert_eq!(
            filesystems_to_relabel(&ctx).unwrap(),
            vec![Path::new(good_mtp)]
        );

        // Filesystem with unsupported type (NTFS)
        reset(&mut ctx);
        let mut bad_img = good_img.clone();
        bad_img.images[0].fs_type = OsImageFileSystemType::Ntfs;
        ctx.image = Some(OsImage::mock(bad_img));
        ctx.populate_filesystems().unwrap();
        assert_eq!(filesystems_to_relabel(&ctx).unwrap(), Vec::<&Path>::new());

        // Filesystem with read-only mount point (do not need to update OS image)
        reset(&mut ctx);
        ctx.spec.storage.filesystems[0].mount_point = Some(MountPoint {
            path: good_mtp.into(),
            options: MountOptions::new(MOUNT_OPTION_READ_ONLY),
        });
        ctx.populate_filesystems().unwrap();
        assert_eq!(filesystems_to_relabel(&ctx).unwrap(), Vec::<&Path>::new());

        // Filesystem with no mount point
        reset(&mut ctx);
        ctx.spec.storage.filesystems[0].mount_point = None;
        ctx.spec.storage.filesystems[0].source =
            FileSystemSource::New(NewFileSystemType::default());
        ctx.image = None;
        ctx.populate_filesystems().unwrap();
        assert_eq!(filesystems_to_relabel(&ctx).unwrap(), Vec::<&Path>::new());

        // Check all filesystem types to make sure we only get the ones that
        // support SELinux!
        let mut expected = Vec::new();
        ctx.filesystems = RealFilesystemType::iter()
            .enumerate()
            .map(|(i, fs_type)| {
                let mtp_path = format!("/mnt/fs{i}");

                if SELINUX_SUPPORTED_FILESYSTEMS.contains(&fs_type) {
                    expected.push(PathBuf::from(&mtp_path));
                }

                FileSystemData::Image(FileSystemDataImage {
                    mount_point: MountPoint {
                        path: mtp_path.into(),
                        options: MountOptions::empty(),
                    },
                    fs_type: Some(fs_type),
                    device_id: format!("dev{i}"),
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(filesystems_to_relabel(&ctx).unwrap(), expected);
    }

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
