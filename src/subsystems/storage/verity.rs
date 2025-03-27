use std::{fs, path::Path};

use anyhow::{bail, Context, Error};
use log::debug;

use osutils::{dependencies::Dependency, tabfile::TabFileEntry};
use trident_api::constants::{
    MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH, TRIDENT_OVERLAY_LOWER_RELATIVE_PATH,
    TRIDENT_OVERLAY_PATH, TRIDENT_OVERLAY_UPPER_RELATIVE_PATH, TRIDENT_OVERLAY_WORK_RELATIVE_PATH,
};

use crate::engine::EngineContext;

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

/// Ensures that the Host Config and the provided image have matching verity
/// configurations. Returns whether verity is enabled, or error if there is some
/// indication of misconfiguration (e.g. images are verity enabled, but HC is
/// not and vice-versa).
pub(super) fn validate_verity_compatibility(ctx: &EngineContext) -> Result<bool, Error> {
    let root_verity_in_image = if let Some(os_img) = ctx.image.as_ref() {
        // Prefer checking the OS image for verity configuration when possible.
        os_img
            .root_filesystem()
            .with_context(|| {
                format!(
                    "Failed to get root filesystem from OS image '{}'",
                    os_img.source()
                )
            })?
            .verity
            .is_some()
    } else {
        bail!("No OS image provided to validate verity compatibility")
    };

    match (root_verity_in_image, ctx.spec.storage.has_verity_device()) {
        // Image has verity but HC doesn't.
        (true, false) => bail!("Verity is enabled for the root image, but no verity definition is present in the Host Configuration"),

        // Image doesn't have verity but HC does.
        (false, true) => bail!("Verity is not enabled for the root image, but a verity definition is present in the Host Configuration"),

        // Verity and HC are in sync, return their state.
        _ => Ok(root_verity_in_image),
    }
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
