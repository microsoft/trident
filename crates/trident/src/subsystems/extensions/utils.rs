use std::path::{Path, PathBuf};

use anyhow::{Context, Error};

use osutils::{container, dependencies::Dependency, path};
use trident_api::{config::Extension, error::TridentResultExt};

/// Helper function to identify if the extension exists in the old Host
/// Configuration, in which case we can reuse its path.
pub(crate) fn check_for_existing_image(
    ext: &Extension,
    old_hc_extensions: &[Extension],
) -> Option<PathBuf> {
    old_hc_extensions
        .iter()
        // Extension must match on Sha384 hash
        .find(|old_ext| ext.sha384 == old_ext.sha384)?
        .path
        .clone()
}

/// Helper function that prepends host root path to a path, if Trident is
/// running in a container.
pub(crate) fn adjust_path_if_container(path: PathBuf) -> Result<PathBuf, Error> {
    Ok(
        if container::is_running_in_container()
            .unstructured("Failed to check if Trident is running in a container")?
        {
            path::join_relative(
                container::get_host_root_path().unstructured("Failed to get host root path")?,
                path,
            )
        } else {
            path
        },
    )
}

/// Helper function to mount the extension image.
pub(crate) fn attach_device_and_mount(
    image_file_path: &Path,
    mount_path: &Path,
) -> Result<String, Error> {
    let loop_device_output = Dependency::Losetup
        .cmd()
        .arg("-f")
        .arg("--show")
        .arg(image_file_path)
        .output_and_check()
        .context("Failed to attach loop device")?;
    let loop_device = loop_device_output.trim();

    // Must mount with option '-t ddi', which internally invokes systemd-dissect
    // as a helper to parse the partitions in the image.
    let mount_result = Dependency::Mount
        .cmd()
        .arg("-t")
        .arg("ddi")
        .arg(loop_device)
        .arg(mount_path)
        .run_and_check();
    if let Err(e) = mount_result {
        // Detach the loop device if mounting failed.
        Dependency::Losetup
            .cmd()
            .arg("-d")
            .arg(loop_device)
            .run_and_check()
            .context("Failed to clean up loop device after mount failed")?;
        // After detaching the loop device, return mount error.
        return Err(e.into());
    }

    Ok(loop_device.to_string())
}

/// Helper function to unmount the extension image.
pub(crate) fn detach_device_and_unmount(
    device_path: String,
    mount_path: &Path,
) -> Result<(), Error> {
    Dependency::Umount
        .cmd()
        .arg(mount_path)
        .run_and_check()
        .context("Failed to unmount extension image")?;
    Dependency::Losetup
        .cmd()
        .arg("-d")
        .arg(device_path)
        .run_and_check()
        .context("Failed to detach loop device")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use url::Url;

    use trident_api::primitives::hash::Sha384Hash;

    #[test]
    fn test_check_for_existing_image_found() {
        let hash = Sha384Hash::from("a".repeat(96));
        let path = PathBuf::from("/var/lib/extensions/ext1.raw");
        let new_ext = Extension {
            url: Url::parse("https://example.com/ext1.raw").unwrap(),
            sha384: hash.clone(),
            path: None,
        };
        let old_extensions = vec![Extension {
            url: Url::parse("https://example.com/ext1.raw").unwrap(),
            sha384: hash,
            path: Some(path.clone()),
        }];

        assert_eq!(
            check_for_existing_image(&new_ext, &old_extensions),
            Some(path)
        );
    }

    #[test]
    fn test_check_for_existing_image_not_found() {
        let hash1 = Sha384Hash::from("a".repeat(96));
        let hash2 = Sha384Hash::from("b".repeat(96));

        let new_ext = Extension {
            url: Url::parse("https://example.com/ext1.raw").unwrap(),
            sha384: hash1,
            path: None,
        };
        let old_extensions = vec![Extension {
            url: Url::parse("https://example.com/ext2.raw").unwrap(),
            sha384: hash2,
            path: Some(PathBuf::from("/var/lib/extensions/ext1.raw")),
        }];

        assert_eq!(check_for_existing_image(&new_ext, &old_extensions), None);
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_tests {
    use super::*;

    use std::fs;

    use tempfile::{NamedTempFile, TempDir};

    use pytest_gen::functional_test;

    #[functional_test]
    fn test_attach_device_and_mount_unmount() {
        // Create a minimal ext4 filesystem image
        let temp_file = NamedTempFile::new().unwrap();
        let image_path = temp_file.path();

        // Create a 1MB ext4 filesystem
        Dependency::Mkfs
            .cmd()
            .args([
                "-t",
                "ext4",
                "-q",
                "-L",
                "test",
                image_path.to_str().unwrap(),
                "1M",
            ])
            .run_and_check()
            .unwrap();

        // Create a temporary mount point
        let mount_dir = TempDir::new().unwrap();
        let mount_path = mount_dir.path();

        let device = attach_device_and_mount(image_path, mount_path).unwrap();

        // Verify the device path looks correct (should be /dev/loopX)
        assert!(device.starts_with("/dev/loop"));

        // Verify the filesystem is mounted
        let mountinfo = fs::read_to_string("/proc/mounts").unwrap();
        assert!(
            mountinfo.contains(mount_path.to_str().unwrap()),
            "Mount path should appear in /proc/mounts"
        );

        detach_device_and_unmount(device.clone(), mount_path).unwrap();

        // Verify the filesystem is no longer mounted
        let mountinfo_after = fs::read_to_string("/proc/mounts").unwrap();
        assert!(
            !mountinfo_after.contains(mount_path.to_str().unwrap()),
            "Mount path should not appear in /proc/mounts after unmount"
        );

        // Verify the loop device is no longer in use
        let losetup_output = Dependency::Losetup
            .cmd()
            .arg("-l")
            .output_and_check()
            .unwrap();
        assert!(
            !losetup_output.contains(&device),
            "Loop device should not appear in losetup -l after detach"
        );
    }

    #[functional_test]
    fn test_attach_device_mount_failure_cleanup() {
        // Create a file that cannot be mounted as a filesystem
        let temp_file = NamedTempFile::new().unwrap();
        let image_path = temp_file.path();
        fs::write(image_path, b"not a valid filesystem").unwrap();

        // Create a temporary mount point
        let mount_dir = TempDir::new().unwrap();
        let mount_path = mount_dir.path();

        // Attempt to attach and mount - should fail
        let result = attach_device_and_mount(image_path, mount_path);
        assert!(result.is_err(), "Should fail to mount invalid filesystem");

        // Verify no loop devices are left attached
        let losetup_output = Dependency::Losetup
            .cmd()
            .arg("-l")
            .output_and_check()
            .unwrap();
        assert!(
            !losetup_output.contains(image_path.to_str().unwrap()),
            "Loop device should be cleaned up after mount failure"
        );
    }
}
