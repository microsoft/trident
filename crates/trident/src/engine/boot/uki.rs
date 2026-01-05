use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use const_format::formatcp;
use log::{debug, trace};
use procfs::sys::kernel::Version;

use osutils::efivar;
use osutils::path::join_relative;
use trident_api::error::{
    InternalError, ReportError, ServicingError, TridentError, TridentResultExt,
};
use trident_api::{
    constants::{ESP_EFI_DIRECTORY, ESP_MOUNT_POINT_PATH},
    status::AbVolumeSelection,
};

use crate::engine::EngineContext;

/// Temporary name for the UKI file before renaming.
pub const TMP_UKI_NAME: &str = "vmlinuz-0.efi.staged";
pub const UKI_DIRECTORY: &str = formatcp!("{ESP_EFI_DIRECTORY}/Linux");

/// Returns the UKI file suffix, given the current active volume and install index.
fn uki_suffix(ctx: &EngineContext) -> String {
    match ctx.ab_active_volume {
        Some(AbVolumeSelection::VolumeA) => format!("azlb{}.efi", ctx.install_index),
        None | Some(AbVolumeSelection::VolumeB) => format!("azla{}.efi", ctx.install_index),
    }
}

/// Return whether there is a staged UKI file on the ESP.
pub fn is_staged(esp_dir_path: &Path) -> bool {
    esp_dir_path.join(UKI_DIRECTORY).join(TMP_UKI_NAME).exists()
}

/// Copies the UKI file from the mounted image to the ESP directory.
pub fn stage_uki_on_esp(temp_mount_dir: &Path, mount_point: &Path) -> Result<(), Error> {
    let uki_source_dir = temp_mount_dir.join(UKI_DIRECTORY);
    let ukis: Vec<_> = uki_source_dir
        .read_dir()
        .context("Could not read UKI directory")?
        .collect::<Result<Vec<_>, _>>()
        .context("Failed while reading UKI directory")?
        .into_iter()
        .map(|entry| entry.path())
        .collect();

    ensure!(!ukis.is_empty(), "No UKI files found within the image");
    ensure!(ukis.len() == 1, "Multiple UKI files found within the image");

    let dest_path = join_relative(mount_point, ESP_MOUNT_POINT_PATH)
        .join(UKI_DIRECTORY)
        .join(TMP_UKI_NAME);
    debug!("Staging UKI file at '{}'", dest_path.display());
    fs::copy(&ukis[0], dest_path).context("Failed to copy UKI to the ESP")?;

    Ok(())
}

/// Prepares the ESP directory structure required for UKI boot.
pub fn prepare_esp_for_uki(root_mount_point: &Path) -> Result<(), Error> {
    let esp_root_path = join_relative(root_mount_point, ESP_MOUNT_POINT_PATH);
    let esp_uki_directory = esp_root_path.join(UKI_DIRECTORY);

    fs::create_dir_all(&esp_uki_directory)
        .context(format!("Failed to create '{UKI_DIRECTORY}' on the ESP"))?;

    fs::create_dir_all(esp_root_path.join("loader"))
        .context("Failed to create directory loader")?;
    fs::write(esp_root_path.join("loader/entries.srel"), "type1\n")
        .context("Failed to write entries.srel")?;

    Ok(())
}

/// Enumerates existing UKIs in the given directory, returning their indices and suffixes.
fn enumerate_existing_ukis(
    esp_uki_directory: &Path,
) -> Result<Vec<(usize, String, PathBuf)>, Error> {
    let mut uki_entries = Vec::new();

    for entry in fs::read_dir(esp_uki_directory).context(format!(
        "Failed to read directory '{}'",
        esp_uki_directory.display()
    ))? {
        let entry = entry.context("Failed to read entry")?;
        let filename = entry.file_name();

        if let Some((index, suffix)) = filename
            .to_str()
            .and_then(|filename| filename.strip_prefix("vmlinuz-"))
            .and_then(|f| f.split_once('-'))
            .and_then(|(index, suffix)| Some((index.parse::<usize>().ok()?, suffix.to_string())))
        {
            uki_entries.push((index, suffix, entry.path()));
        } else {
            trace!(
                "Ignoring existing UKI file '{}' that does not match Trident naming scheme",
                entry.path().display()
            );
        }
    }

    Ok(uki_entries)
}

/// Updates the boot order by renaming the UKI file according to Trident's naming scheme.
pub fn update_uki_boot_order(
    ctx: &EngineContext,
    esp_dir_path: &Path,
    oneshot: bool,
) -> Result<(), TridentError> {
    let esp_uki_directory = esp_dir_path.join(UKI_DIRECTORY);
    let existing_ukis =
        enumerate_existing_ukis(&esp_uki_directory).structured(ServicingError::EnumerateUkis)?;
    let uki_suffix = uki_suffix(ctx);

    let mut max_index = 99;
    for (index, suffix, path) in existing_ukis {
        if suffix == uki_suffix {
            fs::remove_file(&path)
                .structured(ServicingError::UpdateUki)
                .message(format!("Failed to remove file '{}'", path.display()))?;
        } else {
            max_index = max_index.max(index);
        }
    }

    let dest_path = esp_uki_directory.join(format!("vmlinuz-{}-{uki_suffix}", max_index + 1));
    let entry_name = dest_path
        .file_name() // TODO: should be `file_stem` but systemd-boot doesn't seem to be following the spec.
        .structured(InternalError::Internal("Failed to get file stem"))?
        .to_str()
        .structured(InternalError::Internal("Boot entry name isn't valid UTF-8"))?;

    debug!("Renaming UKI file to '{}'", dest_path.display());
    fs::rename(esp_uki_directory.join(TMP_UKI_NAME), &dest_path)
        .structured(ServicingError::UpdateUki)
        .message("Failed to rename staged UKI")?;

    if oneshot {
        debug!("Setting oneshot boot entry to '{entry_name}'");
        efivar::set_oneshot(entry_name)?;
    } else {
        debug!("Setting default boot entry to '{entry_name}'");
        efivar::set_default(entry_name)?;
    }
    Ok(())
}

fn enumerate_preexisting_ukis(esp_uki_directory: &Path) -> Result<Vec<(Version, PathBuf)>, Error> {
    let mut preexisting_uki_entries = Vec::new();

    for entry in fs::read_dir(esp_uki_directory).context(format!(
        "Failed to read directory '{}'",
        esp_uki_directory.display()
    ))? {
        let entry = entry.context("Failed to read entry")?;
        let filename = entry.file_name();

        if let Some(version) = filename
            .to_str()
            .and_then(|filename| filename.strip_prefix("vmlinuz-"))
            .and_then(|f| f.strip_suffix(".azl3.efi"))
        {
            match version.parse() {
                Ok(v) => preexisting_uki_entries.push((v, entry.path())),
                _ => {
                    debug!(
                        "Ignoring preexisting UKI file '{}' with unparseable version '{}'",
                        entry.path().display(),
                        version
                    );
                }
            }
        } else {
            debug!(
                "Ignoring preexisting UKI file '{}' that does not match expected preexisting UKI naming scheme",
                entry.path().display()
            );
        }
    }

    Ok(preexisting_uki_entries)
}

pub fn find_previous_uki(esp_dir_path: &Path) -> Result<PathBuf, TridentError> {
    let esp_uki_directory = esp_dir_path.join(UKI_DIRECTORY);
    let trident_managed_ukis = enumerate_existing_ukis(&esp_uki_directory)
        .structured(ServicingError::EnumerateUkis)
        .message("Failed to enumerate Trident-managed UKIs")?;
    let mut uki_entries: Vec<_> = trident_managed_ukis
        .into_iter()
        .filter(|(_, suffix, _)| suffix.ends_with(".efi"))
        .collect();
    uki_entries.sort_by_key(|(index, _, _)| *index);
    println!("Found Trident-managed UKI entries: [{:?}]", uki_entries);

    let preexising_ukis = enumerate_preexisting_ukis(&esp_uki_directory)
        .structured(ServicingError::EnumerateUkis)
        .message("Failed to enumerate preexisting UKIs")?;
    let mut preexisting_uki_entries: Vec<_> = preexising_ukis.into_iter().collect();
    preexisting_uki_entries.sort_by_key(|(version, _)| *version);
    println!(
        "Found preexisting UKI entries: [{:?}]",
        preexisting_uki_entries
    );

    if uki_entries.len() >= 2 {
        // If Trident has managed more than 2 versions, return the second most recent
        let (_, _, previous_uki_entry_path) = &uki_entries[uki_entries.len() - 2];
        Ok(previous_uki_entry_path.clone())
    } else if uki_entries.len() == 1 && !preexisting_uki_entries.is_empty() {
        // If Trident has managed 1 version and there is at least 1 preexisting UKI
        // (this is commonly the VM or offline-init case), return the most recent
        // pre-existing UKI.
        let (_, previous_uki_entry_path) =
            &preexisting_uki_entries[preexisting_uki_entries.len() - 1];
        Ok(previous_uki_entry_path.clone())
    } else {
        // Otherwise, there are not enough UKI entries found to perform a rollback
        Err(TridentError::new(ServicingError::ManualRollback {
            message: "Failed to find more than 1 UKI entries",
        }))
    }
}

pub fn use_previous_uki_as_default(esp_dir_path: &Path) -> Result<(), TridentError> {
    let previous_uki_entry_path = find_previous_uki(esp_dir_path)?;
    let entry_name = previous_uki_entry_path
        .file_name()
        .structured(InternalError::Internal("Failed to get file stem"))?
        .to_str()
        .structured(InternalError::Internal("Boot entry name isn't valid UTF-8"))?;

    debug!("Setting default boot entry to previous UKI '{entry_name}'");
    efivar::set_default(entry_name)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use tempfile::tempdir;

    #[test]
    fn test_uki_suffix() {
        use trident_api::status::AbVolumeSelection;

        let mut ctx = EngineContext {
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            install_index: 1,
            ..Default::default()
        };
        assert_eq!(uki_suffix(&ctx), "azlb1.efi");

        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        ctx.install_index = 2;
        assert_eq!(uki_suffix(&ctx), "azla2.efi");

        ctx.ab_active_volume = None;
        ctx.install_index = 3;
        assert_eq!(uki_suffix(&ctx), "azla3.efi");
    }

    #[test]
    fn test_is_staged() {
        let mock_esp = tempdir().unwrap();
        let uki_dir = mock_esp.path().join(UKI_DIRECTORY);
        fs::create_dir_all(&uki_dir).unwrap();
        assert!(!is_staged(mock_esp.path()));

        fs::write(uki_dir.join(TMP_UKI_NAME), b"dummy").unwrap();
        assert!(is_staged(mock_esp.path()));
    }

    #[test]
    fn test_copy_uki_to_esp() {
        // Create source EFI/Linux directory and a dummy UKI file
        let temp_mount = tempdir().unwrap();
        let src_uki_dir = temp_mount.path().join("EFI/Linux");
        fs::create_dir_all(&src_uki_dir).unwrap();
        fs::write(src_uki_dir.join("dummy-uki.efi"), b"uki-content").unwrap();

        let mount_point = tempdir().unwrap();
        prepare_esp_for_uki(mount_point.path()).unwrap();

        // Should succeed when exactly one UKI file is present
        stage_uki_on_esp(temp_mount.path(), mount_point.path()).unwrap();

        // Check that the file was copied to the correct destination
        let dest_uki_file = join_relative(mount_point.path(), ESP_MOUNT_POINT_PATH)
            .join(UKI_DIRECTORY)
            .join(TMP_UKI_NAME);
        assert_eq!(fs::read(&dest_uki_file).unwrap(), b"uki-content");

        // Should fail if there are multiple UKI files
        let extra_uki_file = src_uki_dir.join("another.efi");
        fs::write(&extra_uki_file, b"other").unwrap();
        stage_uki_on_esp(temp_mount.path(), mount_point.path()).unwrap_err();
    }

    #[test]
    fn test_prepare_esp_for_uki() {
        let root_mount = tempdir().unwrap();
        prepare_esp_for_uki(root_mount.path()).unwrap();

        let esp_root_path = join_relative(root_mount.path(), ESP_MOUNT_POINT_PATH);
        assert!(esp_root_path.join(UKI_DIRECTORY).exists());
        assert!(esp_root_path.join("loader").exists());
        assert!(esp_root_path.join("loader/entries.srel").exists());
        let content = fs::read_to_string(esp_root_path.join("loader/entries.srel")).unwrap();
        assert_eq!(content, "type1\n");
    }

    #[test]
    fn test_enumerate_existing_ukis_empty_directory() {
        let dir = tempdir().unwrap();
        let entries = enumerate_existing_ukis(dir.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_enumerate_existing_ukis_single_valid_entry() {
        let dir = tempdir().unwrap();
        let uki_path = dir.path().join("vmlinuz-1-azla1.efi");
        File::create(&uki_path).unwrap();

        let entries = enumerate_existing_ukis(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], (1, "azla1.efi".to_string(), uki_path));
    }

    #[test]
    fn test_enumerate_existing_ukis_multiple_valid_entries() {
        let dir = tempdir().unwrap();
        let uki_path1 = dir.path().join("vmlinuz-1-azla1.efi");
        let uki_path2 = dir.path().join("vmlinuz-2-azlb2.efi");
        File::create(&uki_path1).unwrap();
        File::create(&uki_path2).unwrap();

        let mut entries = enumerate_existing_ukis(dir.path()).unwrap();
        entries.sort_by_key(|e| e.0);

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0], (1, "azla1.efi".to_string(), uki_path1));
        assert_eq!(entries[1], (2, "azlb2.efi".to_string(), uki_path2));
    }

    #[test]
    fn test_enumerate_existing_ukis_ignores_invalid_entries() {
        let dir = tempdir().unwrap();
        let valid_uki = dir.path().join("vmlinuz-3-azla3.efi");
        let invalid_uki1 = dir.path().join("invalid-file.efi");
        let invalid_uki2 = dir.path().join("vmlinuz-noindex-azla.efi");
        File::create(&valid_uki).unwrap();
        File::create(&invalid_uki1).unwrap();
        File::create(&invalid_uki2).unwrap();

        let entries = enumerate_existing_ukis(dir.path()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0], (3, "azla3.efi".to_string(), valid_uki));
    }

    #[test]
    fn test_enumerate_existing_ukis_non_numeric_index() {
        let dir = tempdir().unwrap();
        let invalid_uki = dir.path().join("vmlinuz-abc-azla.efi");
        File::create(&invalid_uki).unwrap();

        let entries = enumerate_existing_ukis(dir.path()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn test_find_previous_uki() {
        let dir = tempdir().unwrap();
        let efi_dir = dir.path().join("EFI/Linux");
        fs::create_dir_all(&efi_dir).unwrap();

        // No UKI files in directory, should error
        assert!(find_previous_uki(dir.path()).is_err());

        let uki_path1 = efi_dir.join("vmlinuz-100-azla1.efi");
        File::create(&uki_path1).unwrap();
        // 1 UKI file in directory, should error
        assert!(find_previous_uki(dir.path()).is_err());

        let uki_path2 = efi_dir.join("vmlinuz-101-azlb2.efi");
        File::create(&uki_path2).unwrap();
        // 2 UKI file in directory, should return uki_path1
        assert_eq!(find_previous_uki(dir.path()).unwrap(), uki_path1);

        let uki_path3 = efi_dir.join("vmlinuz-102-azla2.efi");
        File::create(&uki_path3).unwrap();
        // 3 UKI file in directory, should return uki_path2
        assert_eq!(find_previous_uki(dir.path()).unwrap(), uki_path2);
    }

    #[test]
    fn test_find_previous_uki_offline_init() {
        let dir = tempdir().unwrap();
        let efi_dir = dir.path().join("EFI/Linux");
        fs::create_dir_all(&efi_dir).unwrap();

        // No UKI files in directory, should error
        assert!(find_previous_uki(dir.path()).is_err());

        let preexisting_uki_path = efi_dir.join("vmlinuz-6.6.117.1-1.azl3.efi");
        File::create(&preexisting_uki_path).unwrap();
        // only 1 pre-existing UKI file in directory, should error
        assert!(find_previous_uki(dir.path()).is_err());

        let uki_path1 = efi_dir.join("vmlinuz-100-azla1.efi");
        File::create(&uki_path1).unwrap();
        // 1 trident-managed UKI file and 1 pre-exsiting file in directory,
        // should return preexisting_uki_path
        assert_eq!(find_previous_uki(dir.path()).unwrap(), preexisting_uki_path);

        let uki_path2 = efi_dir.join("vmlinuz-101-azlb2.efi");
        File::create(&uki_path2).unwrap();
        // 2 UKI file in directory, should return uki_path1
        assert_eq!(find_previous_uki(dir.path()).unwrap(), uki_path1);
    }
}
