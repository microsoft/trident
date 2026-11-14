use std::{
    collections::BTreeSet,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use nix::sys::statvfs;

use osutils::uki as uki_utils;

use trident_api::{config::UefiFallbackMode, constants::EFI_DEFAULT_BIN_RELATIVE_PATH};

use crate::engine::{
    boot::uki::{self, UKI_DIRECTORY},
    EngineContext,
};

// Required top-level x64 boot files; secondary payloads vary between images.
pub(crate) const FILES: &[&str] = &[
    "bootx64.efi",
    "grubx64.efi",
    "second.efi",
    "HvLoader.efi",
    "lxhvloader.dll",
];

/// Optional bundle-relative directory containing secondary MSHV payload binaries.
/// Contents vary between images, so discover its direct files instead of
/// maintaining a fixed binary list. Subdirectories are ignored.
const MSHV_ESP_PAYLOAD_DIR: &str = "Windows/System32";
const SPACE_RESERVE: u64 = 1024 * 1024;

/// Detects MSHV in an installed bootloader directory; marked bundles must be complete.
pub(crate) fn has_slot_mshv_bundle(bundle: &Path) -> Result<bool, Error> {
    for marker in ["HvLoader.efi", "lxhvloader.dll", "Windows"] {
        match fs::symlink_metadata(bundle.join(marker)) {
            Ok(_) => {
                validate_bundle(bundle)?;
                return Ok(true);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Failed to inspect MSHV boot payload"),
        }
    }
    Ok(false)
}

/// Validates entries and discovers files without requiring completeness or following symlinks.
/// System32 subdirectories are ignored; `.new` and `.old` suffixes are reserved for staging.
fn validate_tree(root: &Path, directory: &Path, temporary: bool) -> Result<Vec<String>, Error> {
    ensure!(
        fs::symlink_metadata(directory)?.is_dir(),
        "Invalid payload directory"
    );
    let secondary = directory
        .strip_prefix(root)?
        .to_str()
        .context("Non-UTF-8 MSHV payload path")?
        .eq_ignore_ascii_case(MSHV_ESP_PAYLOAD_DIR);
    let mut files = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?;
        let relative_name = relative.to_str().context("Non-UTF-8 MSHV payload path")?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            if secondary {
                continue;
            }
            ensure!(
                relative_name.eq_ignore_ascii_case("Windows")
                    || relative_name.eq_ignore_ascii_case(MSHV_ESP_PAYLOAD_DIR),
                "Unexpected payload directory: {}",
                relative.display()
            );
            files.extend(validate_tree(root, &path, temporary)?);
        } else {
            ensure!(
                kind.is_file(),
                "Payload entry is not a regular file: {}",
                path.display()
            );
            ensure!(
                secondary
                    && (temporary
                        || !relative_name.rsplit_once('.').is_some_and(|(_, suffix)| {
                            suffix.eq_ignore_ascii_case("new") || suffix.eq_ignore_ascii_case("old")
                        }))
                    || FILES.iter().any(|name| {
                        relative_name.eq_ignore_ascii_case(name)
                            || temporary
                                && (relative_name.eq_ignore_ascii_case(&format!("{name}.new"))
                                    || relative_name.eq_ignore_ascii_case(&format!("{name}.old")))
                    }),
                "Unexpected payload file: {}",
                relative.display()
            );
            files.push(relative_name.to_owned());
        }
    }
    Ok(files)
}

/// Discovers a complete bundle, relying on FAT's case-insensitive lookups.
fn bundle_files(root: &Path) -> Result<Vec<String>, Error> {
    let mut files = validate_tree(root, root, false)?;
    for name in FILES {
        ensure!(
            fs::symlink_metadata(root.join(name))
                .with_context(|| format!("Missing MSHV payload file: {name}"))?
                .is_file(),
            "Invalid MSHV payload file: {name}"
        );
    }
    files.sort();
    Ok(files)
}

/// Requires the top-level boot files; optional System32 payloads are discovered.
/// Does not validate executable contents or hypervisor identity.
pub(crate) fn validate_bundle(root: &Path) -> Result<(), Error> {
    bundle_files(root)?;
    Ok(())
}

/// Estimates cluster-rounded file bytes plus one cluster per directory.
/// Missing paths contribute zero; symlinks and special files are rejected.
fn allocation(path: &Path, cluster: u64) -> Result<u64, Error> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    if metadata.is_file() {
        return Ok(metadata.len().div_ceil(cluster) * cluster);
    }
    ensure!(
        metadata.is_dir(),
        "Not a regular file or directory: {}",
        path.display()
    );
    let mut total = cluster;
    for entry in fs::read_dir(path)? {
        total += allocation(&entry?.path(), cluster)?;
    }
    Ok(total)
}

/// Budgets selected bundle files and their parents, excluding ignored subdirectories.
fn bundle_allocation(root: &Path, cluster: u64) -> Result<u64, Error> {
    match fs::symlink_metadata(root) {
        Ok(_) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    }
    let mut directories = BTreeSet::from([PathBuf::new()]);
    let mut total = 0;
    for name in validate_tree(root, root, true)? {
        total += allocation(&root.join(&name), cluster)?;
        directories.extend(Path::new(&name).ancestors().skip(1).map(Path::to_path_buf));
    }
    Ok(total + directories.len() as u64 * cluster)
}

/// Requires room for the incoming allocation plus a reserve, crediting planned reclamation.
fn check_space(available: u64, reclaimed: u64, required: u64) -> Result<(), Error> {
    ensure!(
        available.saturating_add(reclaimed) >= required.saturating_add(SPACE_RESERVE),
        "Insufficient ESP capacity: need {required} bytes plus {SPACE_RESERVE} bytes reserve, \
         have {available} bytes free and {reclaimed} bytes reclaimable from the inactive slot"
    );
    Ok(())
}

/// Budgets bundles, UKIs, addons, and fallback, crediting planned pre-staging cleanup.
pub(crate) fn preflight_staging(
    ctx: &EngineContext,
    source_esp: &Path,
    destination_esp: &Path,
    target_bundle: &Path,
) -> Result<(), Error> {
    let stats = statvfs::statvfs(destination_esp)?;
    let cluster = stats.fragment_size();
    ensure!(cluster > 0, "Invalid ESP allocation unit");
    let bundle = bundle_allocation(&source_esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH), cluster)?;
    let mut required = bundle + allocation(&source_esp.join(UKI_DIRECTORY), cluster)?;
    // Account for slot-addon templates whose selected file may replace a smaller live addon.
    required += allocation(&source_esp.join("acl/uki-addons"), cluster)?;
    if ctx.spec.os.uefi_fallback != UefiFallbackMode::Disabled {
        let existing_fallback = destination_esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        required += bundle.max(bundle_allocation(&existing_fallback, cluster)?);
    }
    let mut reclaimed = allocation(target_bundle, cluster)?;
    for path in uki::find_ukis_to_remove_before_staging(ctx, &destination_esp.join(UKI_DIRECTORY))?
    {
        reclaimed +=
            allocation(&path, cluster)? + allocation(&uki_utils::uki_addon_dir(&path), cluster)?;
    }
    check_space(stats.blocks_available() * cluster, reclaimed, required)
}

/// Checks room for a new bundle copy without reclaiming existing ESP contents.
pub(crate) fn preflight_bootstrap(from: &Path, esp: &Path) -> Result<(), Error> {
    let stats = statvfs::statvfs(esp)?;
    let cluster = stats.fragment_size();
    ensure!(cluster > 0, "Invalid ESP allocation unit");
    check_space(
        stats.blocks_available() * cluster,
        0,
        bundle_allocation(from, cluster)?,
    )
}

/// Copies the required top-level boot files listed in `FILES` and any direct
/// secondary MSHV payload files from the optional `MSHV_ESP_PAYLOAD_DIR`,
/// preserving bundle-relative paths. Subdirectories are ignored, and an absent
/// secondary directory is allowed.
///
/// Validates and discovers the source payload before writing, so missing
/// required files or traversal errors do not leave a partially copied payload.
/// Copying itself is non-atomic and can leave partial output on an I/O failure.
/// For atomic publication, copy to a temporary directory and rename it.
pub(crate) fn copy_bundle(from: &Path, to: &Path) -> Result<(), Error> {
    let files = bundle_files(from)?;
    fs::create_dir_all(to)?;
    validate_tree(to, to, false)?;
    for name in &files {
        let destination = to.join(name);
        fs::create_dir_all(destination.parent().context("Missing payload parent")?)?;
        fs::copy(from.join(name), destination)?;
    }
    Ok(())
}

/// Reconciles `.old` backups and stages all `.new` files before replacing fallback.
/// Removes retired secondary payload files after publishing the selected bundle.
/// Whole-bundle replacement is not atomic.
pub(crate) fn replace_bundle(from: &Path, to: &Path) -> Result<(), Error> {
    let files = bundle_files(from)?;
    let existing = match fs::symlink_metadata(to) {
        Ok(_) => {
            let existing = validate_tree(to, to, true)?;
            ensure!(
                fs::canonicalize(from)? != fs::canonicalize(to)?,
                "Source and fallback directories must differ"
            );
            existing
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Vec::new(),
        Err(error) => return Err(error.into()),
    };
    let stats = statvfs::statvfs(to.parent().context("Missing fallback parent")?)?;
    let cluster = stats.fragment_size();
    ensure!(cluster > 0, "Invalid ESP allocation unit");
    let required = bundle_allocation(from, cluster)?;
    let mut reclaimed = 0;
    for name in &files {
        reclaimed += allocation(&to.join(format!("{name}.new")), cluster)?;
    }
    check_space(stats.blocks_available() * cluster, reclaimed, required)?;
    for name in &files {
        let destination = to.join(name);
        let previous = to.join(format!("{name}.old"));
        fs::create_dir_all(destination.parent().context("Missing payload parent")?)?;
        if previous.exists() {
            if destination.exists() {
                fs::remove_file(&previous)?;
            } else {
                fs::rename(&previous, &destination)?;
            }
        }
        fs::copy(from.join(name), to.join(format!("{name}.new")))?;
    }
    for name in &files {
        let destination = to.join(name);
        if destination.exists() {
            fs::rename(&destination, to.join(format!("{name}.old")))?;
        }
    }
    for name in &files {
        fs::rename(to.join(format!("{name}.new")), to.join(name))?;
    }
    for name in &files {
        let previous = to.join(format!("{name}.old"));
        if previous.exists() {
            fs::remove_file(previous)?;
        }
    }
    for name in existing {
        let base = name
            .rsplit_once('.')
            .filter(|(_, suffix)| {
                suffix.eq_ignore_ascii_case("new") || suffix.eq_ignore_ascii_case("old")
            })
            .map_or(name.as_str(), |(base, _)| base);
        if !files.iter().any(|file| file.eq_ignore_ascii_case(base)) {
            fs::remove_file(to.join(name))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs::File, os::unix::fs as unix_fs};

    use tempfile::TempDir;

    use trident_api::status::{AbVolumeSelection, ServicingState, ServicingType};

    use crate::subsystems::esp;

    /// Creates a complete test bundle.
    fn bundle(root: &Path, contents: &[u8]) {
        for name in FILES {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
    }

    /// Candidate copying preserves the retained slot.
    #[test]
    fn copies_complete_bundle_without_touching_other_slots() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let active = temp.path().join("AZLA");
        let target = temp.path().join("AZLB");
        bundle(&source, b"new");
        bundle(&active, b"retained");
        copy_bundle(&source, &target).unwrap();
        for name in FILES {
            assert_eq!(fs::read(target.join(name)).unwrap(), b"new");
            assert_eq!(fs::read(active.join(name)).unwrap(), b"retained");
        }
    }

    #[test]
    fn discovers_optional_secondary_payload_without_recursing() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        let secondary = source.join("Windows/System32");
        bundle(&source, b"new");
        fs::create_dir_all(secondary.join("drivers")).unwrap();
        fs::write(secondary.join("image-specific.bin"), b"\0\xffpayload").unwrap();
        fs::write(secondary.join("drivers/ignored.dll"), b"ignored").unwrap();

        copy_bundle(&source, &target).unwrap();
        assert_eq!(
            fs::read(target.join("Windows/System32/image-specific.bin")).unwrap(),
            b"\0\xffpayload"
        );
        assert!(!target.join("Windows/System32/drivers").exists());
        assert!(has_slot_mshv_bundle(&target).unwrap());

        fs::remove_dir_all(source.join("Windows")).unwrap();
        let without_secondary = temp.path().join("without-secondary");
        copy_bundle(&source, &without_secondary).unwrap();
        assert!(!without_secondary.join("Windows").exists());
        fs::create_dir_all(&secondary).unwrap();
        let empty_secondary = temp.path().join("empty-secondary");
        copy_bundle(&source, &empty_secondary).unwrap();
        assert!(!empty_secondary.join("Windows").exists());

        for name in ["payload.dll.new", "payload.dll.old"] {
            let path = secondary.join(name);
            fs::write(&path, b"reserved temporary name").unwrap();
            assert!(validate_bundle(&source).is_err());
            fs::remove_file(path).unwrap();
        }
        unix_fs::symlink(temp.path().join("outside"), secondary.join("linked.dll")).unwrap();
        let rejected = temp.path().join("rejected");
        assert!(copy_bundle(&source, &rejected).is_err());
        assert!(!rejected.exists());
    }

    #[test]
    fn replaces_changing_secondary_payload_and_recovers_partial_publication() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let fallback = temp.path().join("fallback");
        bundle(&source, b"new");
        bundle(&fallback, b"old");
        let secondary = "Windows/System32";
        fs::create_dir_all(source.join(secondary)).unwrap();
        fs::create_dir_all(fallback.join(secondary)).unwrap();
        fs::write(source.join(secondary).join("current.dll"), b"current").unwrap();
        fs::write(
            fallback.join(secondary).join("current.dll.old"),
            b"retained",
        )
        .unwrap();
        fs::write(
            fallback.join(secondary).join("current.dll.new"),
            b"interrupted",
        )
        .unwrap();
        for name in ["obsolete.dll", "obsolete.dll.old", "obsolete.dll.new"] {
            fs::write(fallback.join(secondary).join(name), b"obsolete").unwrap();
        }

        replace_bundle(&source, &fallback).unwrap();
        replace_bundle(&source, &fallback).unwrap();
        assert_eq!(
            fs::read(fallback.join(secondary).join("current.dll")).unwrap(),
            b"current"
        );
        for name in [
            "current.dll.old",
            "current.dll.new",
            "obsolete.dll",
            "obsolete.dll.old",
            "obsolete.dll.new",
        ] {
            assert!(!fallback.join(secondary).join(name).exists());
        }

        fs::remove_dir_all(source.join("Windows")).unwrap();
        replace_bundle(&source, &fallback).unwrap();
        assert!(!fallback.join(secondary).join("current.dll").exists());
        validate_bundle(&fallback).unwrap();
    }

    #[test]
    fn budgets_discovered_secondary_payload_but_not_ignored_subdirectories() {
        let temp = TempDir::new().unwrap();
        let source_esp = temp.path().join("source");
        let source = source_esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        let esp = temp.path().join("esp");
        let fallback = esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        let target = esp.join("EFI/AZLB");
        bundle(&source, b"new");
        bundle(&fallback, b"retained");
        let secondary = source.join("Windows/System32");
        fs::create_dir_all(&secondary).unwrap();
        let large = secondary.join("image-specific.bin");
        let stats = statvfs::statvfs(&esp).unwrap();
        File::create(&large)
            .unwrap()
            .set_len(stats.blocks_available() * stats.fragment_size() + SPACE_RESERVE)
            .unwrap();
        let mut ctx = EngineContext {
            servicing_type: ServicingType::AbUpdate,
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            ..Default::default()
        };
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Disabled;

        assert!(preflight_staging(&ctx, &source_esp, &esp, &target).is_err());
        assert!(preflight_bootstrap(&source, &esp).is_err());
        assert!(replace_bundle(&source, &fallback).is_err());
        assert_eq!(
            fs::read(fallback.join("HvLoader.efi")).unwrap(),
            b"retained"
        );
        assert!(!target.exists());

        fs::create_dir(secondary.join("ignored")).unwrap();
        fs::rename(&large, secondary.join("ignored/large.bin")).unwrap();
        preflight_staging(&ctx, &source_esp, &esp, &target).unwrap();
        preflight_bootstrap(&source, &esp).unwrap();
        replace_bundle(&source, &fallback).unwrap();
        assert_eq!(fs::read(fallback.join("HvLoader.efi")).unwrap(), b"new");
        assert!(!fallback.join("Windows/System32/ignored").exists());
    }

    /// Allowlist matching ignores case; temporary names require opt-in.
    #[test]
    fn accepts_fat_filename_casing_but_rejects_unknown_entries() {
        let temp = TempDir::new().unwrap();
        for name in FILES {
            let path = temp.path().join(name.to_ascii_uppercase());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"payload").unwrap();
        }
        validate_tree(temp.path(), temp.path(), false).unwrap();
        let pending = temp.path().join("SECOND.EFI.NEW");
        fs::write(&pending, b"pending").unwrap();
        assert!(validate_tree(temp.path(), temp.path(), false).is_err());
        validate_tree(temp.path(), temp.path(), true).unwrap();
        fs::remove_file(pending).unwrap();
        fs::write(temp.path().join("IDENTITY.JSON"), b"unexpected").unwrap();
        assert!(validate_tree(temp.path(), temp.path(), true).is_err());
    }

    /// Invalid sources fail before destination creation.
    #[test]
    fn rejects_incomplete_and_unexpected_payload_before_copy() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        bundle(&source, b"new");
        fs::remove_file(source.join("lxhvloader.dll")).unwrap();
        assert!(copy_bundle(&source, &target).is_err());
        assert!(!target.exists());
        bundle(&source, b"new");
        fs::write(source.join("identity.json"), b"unexpected").unwrap();
        assert!(copy_bundle(&source, &target).is_err());
        assert!(!target.exists());
    }

    /// Validation rejects source and destination symlinks.
    #[test]
    fn rejects_source_and_destination_symlinks() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        let outside = temp.path().join("outside");
        bundle(&source, b"new");
        fs::create_dir(&outside).unwrap();
        unix_fs::symlink(&outside, &target).unwrap();
        assert!(replace_bundle(&source, &target).is_err());
        fs::remove_file(&target).unwrap();
        fs::remove_file(source.join("lxhvloader.dll")).unwrap();
        unix_fs::symlink(outside.join("dll"), source.join("lxhvloader.dll")).unwrap();
        assert!(copy_bundle(&source, &target).is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
    }

    /// Retrying publication replaces stale files and removes backups.
    #[test]
    fn retries_partial_publication_and_removes_only_known_temporary_files() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("AZLB");
        let fallback = temp.path().join("BOOT");
        bundle(&source, b"new");
        bundle(&fallback, b"old");
        fs::rename(
            fallback.join("HvLoader.efi"),
            fallback.join("HvLoader.efi.old"),
        )
        .unwrap();
        fs::write(fallback.join("second.efi.new"), b"partial").unwrap();
        fs::write(fallback.join("lxhvloader.dll.old"), b"stale").unwrap();
        replace_bundle(&source, &fallback).unwrap();
        replace_bundle(&source, &fallback).unwrap();
        validate_bundle(&fallback).unwrap();
        for name in FILES {
            assert_eq!(fs::read(fallback.join(name)).unwrap(), b"new");
            assert!(!fallback.join(format!("{name}.new")).exists());
            assert!(!fallback.join(format!("{name}.old")).exists());
        }
        assert!(replace_bundle(&source, &source).is_err());
    }

    /// Capacity includes cluster rounding, reclamation, and a reserve.
    #[test]
    fn capacity_accounts_for_clusters_and_preserves_a_reserve() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("file"), b"x").unwrap();
        assert_eq!(allocation(&temp.path().join("file"), 4096).unwrap(), 4096);
        assert!(check_space(SPACE_RESERVE, 0, 1).is_err());
        assert!(check_space(SPACE_RESERVE, 4096, 4096).is_ok());
    }

    /// Failed preflight leaves both slots untouched.
    #[test]
    fn preflight_rejects_peak_allocation_before_mutating_inactive_or_retained_files() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let destination = temp.path().join("esp");
        let target = destination.join("EFI/AZLB");
        let retained = destination.join("EFI/AZLA");
        bundle(&source.join(EFI_DEFAULT_BIN_RELATIVE_PATH), b"new");
        bundle(&target, b"inactive");
        bundle(&retained, b"committed");
        fs::create_dir_all(source.join(UKI_DIRECTORY)).unwrap();
        let stats = statvfs::statvfs(&destination).unwrap();
        File::create(source.join(UKI_DIRECTORY).join("new.efi"))
            .unwrap()
            .set_len(stats.blocks_available() * stats.fragment_size() + SPACE_RESERVE)
            .unwrap();
        let ctx = EngineContext {
            servicing_type: ServicingType::AbUpdate,
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            ..Default::default()
        };
        assert!(preflight_staging(&ctx, &source, &destination, &target).is_err());
        for name in FILES {
            assert_eq!(fs::read(target.join(name)).unwrap(), b"inactive");
            assert_eq!(fs::read(retained.join(name)).unwrap(), b"committed");
        }
    }

    /// Enabled fallback policies copy whole bundles and preserve the retained slot.
    #[test]
    fn fallback_modes_use_the_full_selected_bundle_during_trial_commit_and_rollback() {
        for mode in [
            UefiFallbackMode::Conservative,
            UefiFallbackMode::Optimistic,
            UefiFallbackMode::Disabled,
        ] {
            let temp = TempDir::new().unwrap();
            let efi = temp.path().join("EFI");
            let active = efi.join("AZL2A");
            let target = efi.join("AZL2B");
            let fallback = efi.join("BOOT");
            bundle(&active, b"retained");
            bundle(&target, b"candidate");
            bundle(&fallback, b"retained");
            let secondary = "Windows/System32/image-defined.bin";
            for (root, contents) in [
                (&active, b"retained".as_slice()),
                (&target, b"candidate".as_slice()),
                (&fallback, b"retained".as_slice()),
            ] {
                let path = root.join(secondary);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, contents).unwrap();
            }
            let mut ctx = EngineContext {
                servicing_type: ServicingType::AbUpdate,
                ab_active_volume: Some(AbVolumeSelection::VolumeA),
                install_index: 1,
                esp_mount_path: ".".into(),
                ..Default::default()
            };
            ctx.spec.os.uefi_fallback = mode;
            esp::set_uefi_fallback_contents(&ctx, ServicingState::AbUpdateStaged, temp.path())
                .unwrap();
            for name in FILES.iter().copied().chain([secondary]) {
                let expected = if mode == UefiFallbackMode::Optimistic {
                    b"candidate".as_slice()
                } else {
                    b"retained".as_slice()
                };
                assert_eq!(fs::read(fallback.join(name)).unwrap(), expected);
            }
            esp::set_uefi_fallback_contents(&ctx, ServicingState::AbUpdateFinalized, temp.path())
                .unwrap();
            for name in FILES.iter().copied().chain([secondary]) {
                let expected = if mode == UefiFallbackMode::Disabled {
                    b"retained".as_slice()
                } else {
                    b"candidate".as_slice()
                };
                assert_eq!(fs::read(fallback.join(name)).unwrap(), expected);
                assert_eq!(fs::read(active.join(name)).unwrap(), b"retained");
            }
            ctx.servicing_type = ServicingType::ManualRollbackAb;
            ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
            esp::set_uefi_fallback_contents(
                &ctx,
                ServicingState::ManualRollbackAbStaged,
                temp.path(),
            )
            .unwrap();
            for name in FILES.iter().copied().chain([secondary]) {
                assert_eq!(fs::read(fallback.join(name)).unwrap(), b"retained");
            }
        }
    }
}
