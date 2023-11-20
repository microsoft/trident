use std::path::{Path, PathBuf};

use anyhow::{Context, Error};
use duct::cmd;

pub const MOUNT_UNIT_SUFFIX: &str = "mount";

/// Takes in a path and a suffix, and returns a systemd-escaped unit name.
///
/// Example:
///
/// - /mnt, mount -> mnt.mount
/// - /mnt/foo, mount -> mnt-foo.mount
pub fn escape_mount_unit_name<S>(path: &S, suffix: &str) -> Result<PathBuf, Error>
where
    S: AsRef<Path>,
{
    Ok(cmd!(
        "systemd-escape",
        "-p",
        format!("--suffix={}", suffix),
        path.as_ref()
    )
    .read()
    .context("Failed to escape unit name")?
    .trim()
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_mount_unit_name() {
        let mount_path = Path::new("/mnt");
        let mount_unit = escape_mount_unit_name(&mount_path, MOUNT_UNIT_SUFFIX).unwrap();
        assert_eq!(mount_unit, PathBuf::from("mnt.mount"));

        let mount_path = Path::new("/mnt/foo");
        let mount_unit = escape_mount_unit_name(&mount_path, MOUNT_UNIT_SUFFIX).unwrap();
        assert_eq!(mount_unit, PathBuf::from("mnt-foo.mount"));
    }
}
