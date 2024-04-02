use std::path::Path;

use log::debug;
use osutils::files;
use sys_mount::{FilesystemType, Mount, MountFlags, UnmountDrop, UnmountFlags};
use trident_api::{
    constants::{
        TRIDENT_OVERLAY_LOWER_RELATIVE_PATH, TRIDENT_OVERLAY_RELATIVE_PATH,
        TRIDENT_OVERLAY_UPPER_RELATIVE_PATH, TRIDENT_OVERLAY_WORK_RELATIVE_PATH,
    },
    error::{ManagementError, ReportError, TridentError},
};

/// Sets up the overlay for the /etc directory, using
/// TRIDENT_OVERLAY_RELATIVE_PATH for the work and upper directories.
pub(super) fn create(
    mount_path: &Path,
    writable: bool,
) -> Result<UnmountDrop<Mount>, TridentError> {
    debug!(
        "Setting up overlay for /etc at '{}', writable: {writable}",
        mount_path.display()
    );

    let overlay_upper_path = mount_path
        .join(TRIDENT_OVERLAY_RELATIVE_PATH)
        .join(TRIDENT_OVERLAY_UPPER_RELATIVE_PATH);
    files::create_dirs(&overlay_upper_path).structured(ManagementError::CreateDirectory {
        dir: overlay_upper_path.clone(),
    })?;
    let overlay_work_path = mount_path
        .join(TRIDENT_OVERLAY_RELATIVE_PATH)
        .join(TRIDENT_OVERLAY_WORK_RELATIVE_PATH);
    files::create_dirs(&overlay_work_path).structured(ManagementError::CreateDirectory {
        dir: overlay_work_path.clone(),
    })?;
    let target_path = mount_path.join(TRIDENT_OVERLAY_LOWER_RELATIVE_PATH);
    let etc_overlay_mount = Mount::builder()
        .fstype(FilesystemType::from("overlay"))
        .flags(if writable {
            MountFlags::empty()
        } else {
            MountFlags::RDONLY
        })
        .data(
            format!(
                "lowerdir={},upperdir={},workdir={}",
                &target_path
                    .to_str()
                    .structured(ManagementError::PathIsNotUnicode {
                        path: target_path.clone()
                    })?,
                &overlay_upper_path
                    .to_str()
                    .structured(ManagementError::PathIsNotUnicode {
                        path: overlay_upper_path.clone()
                    })?,
                &overlay_work_path
                    .to_str()
                    .structured(ManagementError::PathIsNotUnicode {
                        path: overlay_work_path.clone()
                    })?,
            )
            .as_str(),
        )
        .mount_autodrop("overlay", &target_path, UnmountFlags::empty())
        .structured(ManagementError::MountOverlay {
            target: target_path,
        })?;
    Ok(etc_overlay_mount)
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use std::fs;

    use super::*;
    use pytest_gen::functional_test;

    #[functional_test]
    fn test_setup_etc_overlay_success() {
        let mount_dir = tempfile::tempdir().unwrap();
        let mount_path = mount_dir.path();

        let target_path = mount_path.join(TRIDENT_OVERLAY_LOWER_RELATIVE_PATH);
        let test_path = target_path.join("test");
        let test_path2 = target_path.join("test2");

        let upper_path = mount_path
            .join(TRIDENT_OVERLAY_RELATIVE_PATH)
            .join(TRIDENT_OVERLAY_UPPER_RELATIVE_PATH);
        let work_path = mount_path.join(TRIDENT_OVERLAY_RELATIVE_PATH);

        files::create_dirs(&test_path).unwrap();

        {
            let _etc_overlay_mount = create(mount_path, true).unwrap();

            assert!(upper_path.exists());
            assert!(work_path.exists());

            assert!(test_path.exists());
            fs::remove_dir(&test_path).unwrap();
            assert!(!test_path.exists());

            files::create_dirs(&test_path2).unwrap();

            assert!(upper_path.join("test2").exists());
        }

        assert!(upper_path.join("test2").exists());
        assert!(!test_path2.exists());
        assert!(test_path.exists());

        {
            let _etc_overlay_mount = create(mount_path, false).unwrap();

            assert!(upper_path.exists());
            assert!(work_path.exists());

            assert!(!test_path.exists());
            assert!(upper_path.join("test2").exists());

            assert_eq!(
                fs::remove_dir(&test_path2)
                    .unwrap_err()
                    .raw_os_error()
                    .unwrap(),
                30 // read only file system
            );
        }

        assert!(upper_path.join("test2").exists());
        assert!(!test_path2.exists());
        assert!(test_path.exists());
    }
}
