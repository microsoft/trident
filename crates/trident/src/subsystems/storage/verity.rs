use std::{fs, path::Path};

use anyhow::{Context, Error};
use log::debug;

use osutils::{dependencies::Dependency, tabfile::TabFileEntry};
use trident_api::constants::{
    MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH, TRIDENT_OVERLAY_LOWER_RELATIVE_PATH,
    TRIDENT_OVERLAY_PATH, TRIDENT_OVERLAY_UPPER_RELATIVE_PATH, TRIDENT_OVERLAY_WORK_RELATIVE_PATH,
};

/// Create read-only /etc/ overlay mount point representation.
pub(super) fn create_etc_overlay_mount_point() -> TabFileEntry {
    // inject the /etc overlay used for verity setups
    debug!("Creating /etc overlay mount point for verity setups");
    TabFileEntry::new_overlay(
        Path::new(ROOT_MOUNT_POINT_PATH).join(TRIDENT_OVERLAY_LOWER_RELATIVE_PATH),
    )
    .with_options(vec![
        format!("lowerdir=/{TRIDENT_OVERLAY_LOWER_RELATIVE_PATH}"),
        format!("upperdir={TRIDENT_OVERLAY_PATH}/{TRIDENT_OVERLAY_UPPER_RELATIVE_PATH}"),
        format!("workdir={TRIDENT_OVERLAY_PATH}/{TRIDENT_OVERLAY_WORK_RELATIVE_PATH}"),
        MOUNT_OPTION_READ_ONLY.to_owned(),
    ])
}

pub(super) fn create_machine_id(new_root_path: &Path) -> Result<(), Error> {
    let machine_id_path = new_root_path.join("etc/machine-id");
    if machine_id_path.exists() {
        fs::remove_file(&machine_id_path).context(format!(
            "Failed to remove existing machine-id file at '{}'",
            machine_id_path.display()
        ))?;
    }
    Dependency::SystemdFirstboot
        .cmd()
        .arg("--root")
        .arg(new_root_path)
        .arg("--setup-machine-id")
        .run_and_check()
        .context("Failed to generate machine-id")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use osutils::tabfile::{TabDevice, TabMountPoint};
    use sysdefs::filesystems::NodevFilesystemType;

    #[test]
    fn test_create_etc_overlay_mount_point() {
        assert_eq!(
            create_etc_overlay_mount_point(),
            TabFileEntry {
                device: TabDevice::Overlay,
                mount_point: TabMountPoint::Path(
                    Path::new(ROOT_MOUNT_POINT_PATH).join(TRIDENT_OVERLAY_LOWER_RELATIVE_PATH)
                ),
                fs_type: NodevFilesystemType::Overlay.into(),
                options: vec![
                    "lowerdir=/etc".into(),
                    "upperdir=/var/lib/trident-overlay/etc/upper".into(),
                    "workdir=/var/lib/trident-overlay/etc/work".into(),
                    MOUNT_OPTION_READ_ONLY.into()
                ],
                disabled_reason: None,
            },
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;

    use std::fs;

    use pytest_gen::functional_test;

    #[functional_test]
    fn test_create_machine_id() {
        let root_dir = tempfile::tempdir().unwrap();
        let machine_id_path = root_dir.path().join("etc/machine-id");
        create_machine_id(root_dir.path()).unwrap();
        assert!(machine_id_path.exists());
        let machine_id = fs::read_to_string(&machine_id_path).unwrap();
        assert_eq!(machine_id.trim().len(), 32);

        create_machine_id(root_dir.path()).unwrap();
        assert!(machine_id_path.exists());
        let machine_id2 = fs::read_to_string(machine_id_path).unwrap();
        assert_eq!(machine_id2.trim().len(), 32);

        assert_ne!(machine_id, machine_id2);
    }
}
