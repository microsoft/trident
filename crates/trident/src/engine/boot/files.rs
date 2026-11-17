//! Copies complete installed boot trees without interpreting their bootloader.
//!
//! Image-specific completeness belongs to staging and initialization. Relocation
//! preserves unfamiliar files and directories while enforcing structural safety.
//! Replacement owns the destination's entire contents, not individual filenames.

use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use nix::sys::statvfs;

// Leave headroom for directory growth beyond the per-directory estimate.
const SPACE_RESERVE: u64 = 1024 * 1024;

/// Validates the whole tree, including empty directories, and collects relative file paths.
/// `temporary` permits replacement suffixes in destinations left by interrupted work.
fn scan(
    root: &Path,
    directory: &Path,
    temporary: bool,
    files: &mut Vec<PathBuf>,
) -> Result<(), Error> {
    ensure!(
        fs::symlink_metadata(directory)?.is_dir(),
        "Invalid boot directory: {}",
        directory.display()
    );
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_str().context("Non-UTF-8 boot file name")?;
        ensure!(
            temporary || !is_temporary(name),
            "Reserved boot file name: {name}"
        );
        let kind = entry.file_type()?;
        if kind.is_dir() {
            scan(root, &path, temporary, files)?;
        } else {
            ensure!(
                kind.is_file(),
                "Not a regular boot file: {}",
                path.display()
            );
            files.push(path.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(())
}

/// Returns sorted root-relative file paths; empty directories are validated but not listed.
fn boot_files(root: &Path, temporary: bool) -> Result<Vec<PathBuf>, Error> {
    let mut files = Vec::new();
    scan(root, root, temporary, &mut files)?;
    files.sort();
    Ok(files)
}

/// Enumerates regular files recursively without following links.
/// Paths retain the supplied root prefix; reserved replacement names are rejected.
pub(crate) fn filepaths(root: &Path) -> Result<Vec<PathBuf>, Error> {
    Ok(boot_files(root, false)?
        .into_iter()
        .map(|path| root.join(path))
        .collect())
}

// ESP names are case-insensitive, so casing cannot escape reserved replacement suffixes.
fn is_temporary(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, suffix)| {
        suffix.eq_ignore_ascii_case("new") || suffix.eq_ignore_ascii_case("old")
    })
}

/// Resolves existing ancestors so overlap checks also cover destinations not yet created.
fn canonical_path(path: &Path) -> Result<PathBuf, Error> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(canonical_path(
            path.parent().context("Missing boot directory parent")?,
        )?
        .join(path.file_name().context("Missing boot directory name")?)),
        Err(error) => Err(error.into()),
    }
}

/// Prevents recursive copying into the source or retiring entries that contain the source.
fn ensure_disjoint(from: &Path, to: &Path) -> Result<(), Error> {
    let from = canonical_path(from)?;
    let to = canonical_path(to)?;
    ensure!(
        !from.starts_with(&to) && !to.starts_with(&from),
        "Source and destination boot trees overlap"
    );
    Ok(())
}

/// Copies an already-validated entry, creating even empty directories.
/// Existing destination-only entries are not retired, and failure can leave a partial copy.
fn copy_entry(from: &Path, to: &Path) -> Result<(), Error> {
    if fs::symlink_metadata(from)?.is_dir() {
        fs::create_dir_all(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            copy_entry(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        fs::copy(from, to)?;
    }
    Ok(())
}

/// Removes an owned entry; missing entries are expected during retry cleanup.
fn remove_entry(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

/// Copies a validated tree without retiring destination-only entries.
/// An exact copy requires an empty destination. Copy failures may leave partial
/// contents for the caller to discard, as bootstrap does with its staging directory.
pub(crate) fn copy_boot_files(from: &Path, to: &Path) -> Result<(), Error> {
    boot_files(from, false)?;
    if to.try_exists()? {
        boot_files(to, true)?;
    }
    ensure_disjoint(from, to)?;
    copy_entry(from, to)
}

/// Estimates cluster-rounded file bytes plus one cluster per directory.
/// Missing paths contribute zero; links and special files are rejected.
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

/// Checks the estimated copy budget without reclaiming current entries or backups.
/// `reclaimed` counts only matching `.new` entries that retries will discard.
fn check_space(available: u64, reclaimed: u64, required: u64) -> Result<(), Error> {
    ensure!(available.saturating_add(reclaimed) >= required.saturating_add(SPACE_RESERVE),
        "Insufficient ESP capacity: need {required} bytes plus {SPACE_RESERVE} bytes reserve, have {available} bytes free and {reclaimed} bytes reclaimable");
    Ok(())
}

/// Checks room for a complete new boot tree without reclaiming ESP contents.
/// `esp` identifies the destination filesystem; stale bootstrap staging is removed by the caller.
pub(crate) fn preflight_bootstrap(from: &Path, esp: &Path) -> Result<(), Error> {
    boot_files(from, false)?;
    let stats = statvfs::statvfs(esp)?;
    let cluster = stats.fragment_size();
    ensure!(cluster > 0, "Invalid ESP allocation unit");
    check_space(
        stats.blocks_available() * cluster,
        0,
        allocation(from, cluster)?,
    )
}

/// Replaces an owned directory's complete contents, including nested and empty
/// directories. Destination-only entries are retired after the new tree is staged.
///
/// Each top-level entry is staged as `.new`, backed up as `.old`, then published.
/// Directories move with their complete subtrees. Publication across entries is
/// not atomic: failure can leave a mix of current, staged and backup entries.
/// Retries rebuild staging and recover selected backups whose current entry is absent.
pub(crate) fn replace_boot_files(from: &Path, to: &Path) -> Result<(), Error> {
    // Reject malformed or overlapping trees before modifying destination entries.
    boot_files(from, false)?;
    if to.try_exists()? {
        boot_files(to, true)?;
    }
    ensure_disjoint(from, to)?;
    let mut entries = fs::read_dir(from)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();

    // Budget the new tree alongside current entries and backups. Only matching
    // abandoned `.new` entries are credited; other temporary entries stay in place
    // until publication succeeds.
    let stats = statvfs::statvfs(to.parent().context("Missing fallback parent")?)?;
    let cluster = stats.fragment_size();
    ensure!(cluster > 0, "Invalid ESP allocation unit");
    let mut reclaimed = 0;
    for name in &entries {
        reclaimed += allocation(&to.join(format!("{}.new", name.to_string_lossy())), cluster)?;
    }
    check_space(
        stats.blocks_available() * cluster,
        reclaimed,
        allocation(from, cluster)?,
    )?;

    // Rebuild all incoming staging while leaving current entries available.
    fs::create_dir_all(to)?;
    for name in &entries {
        let destination = to.join(name);
        let previous = to.join(format!("{}.old", name.to_string_lossy()));
        // A missing current entry may be an interrupted rename. Restore its backup
        // before risking another copy; otherwise the current entry wins.
        if previous.try_exists()? {
            if destination.try_exists()? {
                remove_entry(&previous)?;
            } else {
                fs::rename(&previous, &destination)?;
            }
        }
        // Never merge a partial staged directory: it may contain retired files.
        let staged = to.join(format!("{}.new", name.to_string_lossy()));
        remove_entry(&staged)?;
        copy_entry(&from.join(name), &staged)?;
    }

    // Snapshot before renaming so directory iteration cannot skip changed entries.
    // Back up destination-only entries too: ownership is of the complete tree.
    for entry in fs::read_dir(to)?.collect::<Result<Vec<_>, _>>()? {
        let name = entry.file_name();
        let name = name.to_str().context("Non-UTF-8 boot file name")?;
        if !is_temporary(name) {
            let previous = to.join(format!("{name}.old"));
            remove_entry(&previous)?;
            fs::rename(entry.path(), previous)?;
        }
    }

    // Publish only the entries rebuilt from this source, never unrelated stale `.new` files.
    for name in &entries {
        fs::rename(
            to.join(format!("{}.new", name.to_string_lossy())),
            to.join(name),
        )?;
    }

    // Publication is complete. Retire backups and abandoned temporary entries,
    // including entries no longer present in the source.
    for entry in fs::read_dir(to)?.collect::<Result<Vec<_>, _>>()? {
        if is_temporary(
            entry
                .file_name()
                .to_str()
                .context("Non-UTF-8 boot file name")?,
        ) {
            remove_entry(&entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{fs::File, os::unix::fs as unix_fs};

    use nix::{sys::stat::Mode, unistd};
    use tempfile::TempDir;

    use trident_api::{
        config::UefiFallbackMode,
        constants::EFI_DEFAULT_BIN_RELATIVE_PATH,
        status::{AbVolumeSelection, ServicingState, ServicingType},
    };

    use crate::{
        engine::{boot::mshv::FILES, EngineContext},
        subsystems::esp,
    };

    /// Creates all required slot boot files for tests.
    fn create_boot_files(root: &Path, contents: &[u8]) {
        for name in FILES {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
    }

    #[test]
    fn stages_and_replaces_ordinary_fallback_without_touching_other_installations() {
        for mode in [UefiFallbackMode::Conservative, UefiFallbackMode::Optimistic] {
            let temp = TempDir::new().unwrap();
            let source_esp = temp.path().join("source");
            let source = source_esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
            let esp = temp.path().join("esp");
            let fallback = esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
            let target = esp.join("EFI/AZLA");
            let retained = esp.join("EFI/AZL2A");
            create_boot_files(&source, b"mshv");
            fs::create_dir_all(&fallback).unwrap();
            fs::create_dir_all(&retained).unwrap();
            for root in [&fallback, &retained] {
                for name in [
                    "bootx64.efi",
                    "grubx64.efi",
                    "GRUB.CFG",
                    "grub.cfg.NEW",
                    "grub.cfg.old",
                    "grubx64-noprefix.efi",
                    "grubx64-noprefix.efi.new",
                    "grubx64-noprefix.efi.OLD",
                ] {
                    fs::write(root.join(name), b"ordinary").unwrap();
                }
            }
            let mut ctx = EngineContext {
                servicing_type: ServicingType::CleanInstall,
                esp_mount_path: ".".into(),
                ..Default::default()
            };
            ctx.spec.os.uefi_fallback = mode;
            copy_boot_files(&source, &target).unwrap();
            for _ in 0..2 {
                esp::set_uefi_fallback_contents(&ctx, ServicingState::CleanInstallStaged, &esp)
                    .unwrap();
            }
            filepaths(&fallback).unwrap();
            for name in FILES {
                assert_eq!(fs::read(fallback.join(name)).unwrap(), b"mshv");
            }
            for entry in fs::read_dir(&retained).unwrap() {
                let entry = entry.unwrap();
                assert_eq!(fs::read(entry.path()).unwrap(), b"ordinary");
                if !FILES.iter().any(|name| entry.file_name() == *name) {
                    assert!(!fallback.join(entry.file_name()).exists());
                }
            }
        }
    }

    #[test]
    fn copies_installed_files_without_bootloader_specific_allowlists() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(&source).unwrap();
        for name in ["grub.cfg", "grubx64-noprefix.efi", "future-loader.efi"] {
            fs::write(source.join(name), name).unwrap();
        }
        copy_boot_files(&source, &target).unwrap();
        for name in ["grub.cfg", "grubx64-noprefix.efi", "future-loader.efi"] {
            assert_eq!(
                fs::read(source.join(name)).unwrap(),
                fs::read(target.join(name)).unwrap()
            );
        }
    }

    #[test]
    fn rejects_unsafe_ordinary_fallback_entries_before_mutation() {
        for kind in ["symlink", "fifo"] {
            let temp = TempDir::new().unwrap();
            let source_esp = temp.path().join("source");
            let source = source_esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
            let esp = temp.path().join("esp");
            let fallback = esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
            let outside = temp.path().join("outside");
            create_boot_files(&source, b"mshv");
            fs::create_dir_all(&fallback).unwrap();
            fs::write(fallback.join("bootx64.efi"), b"retained").unwrap();
            fs::write(&outside, b"outside").unwrap();
            let ordinary = fallback.join("grub.cfg");
            match kind {
                "symlink" => unix_fs::symlink(&outside, ordinary).unwrap(),
                "fifo" => unistd::mkfifo(&ordinary, Mode::S_IRUSR).unwrap(),
                _ => unreachable!(),
            }
            replace_boot_files(&source, &fallback).unwrap_err();
            assert_eq!(fs::read(fallback.join("bootx64.efi")).unwrap(), b"retained");
            assert_eq!(fs::read(&outside).unwrap(), b"outside");
        }
    }

    /// Candidate copying preserves the retained slot.
    #[test]
    fn copies_slot_boot_files_without_touching_other_slots() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let active = temp.path().join("AZLA");
        let target = temp.path().join("AZLB");
        create_boot_files(&source, b"new");
        create_boot_files(&active, b"retained");
        copy_boot_files(&source, &target).unwrap();
        for name in FILES {
            assert_eq!(fs::read(target.join(name)).unwrap(), b"new");
            assert_eq!(fs::read(active.join(name)).unwrap(), b"retained");
        }
    }

    #[test]
    fn copies_complete_trees_including_nested_components() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        let secondary = source.join("Windows/System32");
        create_boot_files(&source, b"new");
        fs::create_dir_all(secondary.join("drivers")).unwrap();
        fs::write(secondary.join("image-specific.bin"), b"\0\xffpayload").unwrap();
        fs::write(secondary.join("drivers/ignored.dll"), b"ignored").unwrap();

        copy_boot_files(&source, &target).unwrap();
        assert_eq!(
            fs::read(target.join("Windows/System32/image-specific.bin")).unwrap(),
            b"\0\xffpayload"
        );
        assert_eq!(
            fs::read(target.join("Windows/System32/drivers/ignored.dll")).unwrap(),
            b"ignored"
        );
        assert!(target.join("HvLoader.efi").is_file());

        fs::remove_dir_all(source.join("Windows")).unwrap();
        let without_secondary = temp.path().join("without-secondary");
        copy_boot_files(&source, &without_secondary).unwrap();
        assert!(!without_secondary.join("Windows").exists());
        fs::create_dir_all(&secondary).unwrap();
        let empty_secondary = temp.path().join("empty-secondary");
        copy_boot_files(&source, &empty_secondary).unwrap();
        assert!(empty_secondary.join("Windows/System32").is_dir());

        for name in ["payload.dll.new", "payload.dll.old"] {
            let path = secondary.join(name);
            fs::write(&path, b"reserved temporary name").unwrap();
            assert!(filepaths(&source).is_err());
            fs::remove_file(path).unwrap();
        }
        unix_fs::symlink(temp.path().join("outside"), secondary.join("linked.dll")).unwrap();
        let rejected = temp.path().join("rejected");
        assert!(copy_boot_files(&source, &rejected).is_err());
        assert!(!rejected.exists());
    }

    #[test]
    fn replacement_handles_partial_directories_and_entry_type_changes() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("component")).unwrap();
        fs::create_dir_all(target.join("component.new")).unwrap();
        fs::create_dir_all(target.join("loader.old")).unwrap();
        fs::write(source.join("component/current"), b"new").unwrap();
        fs::write(source.join("loader"), b"loader").unwrap();
        fs::write(target.join("component"), b"old file").unwrap();
        fs::write(target.join("component.new/retired"), b"partial").unwrap();
        fs::write(target.join("loader.old/retired"), b"old directory").unwrap();
        replace_boot_files(&source, &target).unwrap();
        replace_boot_files(&source, &target).unwrap();
        assert_eq!(fs::read(target.join("component/current")).unwrap(), b"new");
        assert_eq!(fs::read(target.join("loader")).unwrap(), b"loader");
        assert!(!target.join("component/retired").exists());
        assert!(!target.join("loader.old").exists());
        assert!(replace_boot_files(&source, &source.join("child")).is_err());
        assert!(!source.join("child").exists());
    }

    #[test]
    fn replaces_changing_boot_components_and_recovers_partial_publication() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let fallback = temp.path().join("fallback");
        create_boot_files(&source, b"new");
        create_boot_files(&fallback, b"old");
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

        replace_boot_files(&source, &fallback).unwrap();
        replace_boot_files(&source, &fallback).unwrap();
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
        replace_boot_files(&source, &fallback).unwrap();
        assert!(!fallback.join(secondary).join("current.dll").exists());
        filepaths(&fallback).unwrap();
    }

    #[test]
    fn budgets_all_files_including_nested_subdirectories() {
        let temp = TempDir::new().unwrap();
        let source_esp = temp.path().join("source");
        let source = source_esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        let esp = temp.path().join("esp");
        let fallback = esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        create_boot_files(&source, b"new");
        create_boot_files(&fallback, b"retained");
        let secondary = source.join("Windows/System32");
        fs::create_dir_all(&secondary).unwrap();
        let large = secondary.join("image-specific.bin");
        let stats = statvfs::statvfs(&esp).unwrap();
        File::create(&large)
            .unwrap()
            .set_len(stats.blocks_available() * stats.fragment_size() + SPACE_RESERVE)
            .unwrap();
        assert!(preflight_bootstrap(&source, &esp).is_err());
        assert!(replace_boot_files(&source, &fallback).is_err());
        assert_eq!(
            fs::read(fallback.join("HvLoader.efi")).unwrap(),
            b"retained"
        );

        fs::create_dir(secondary.join("ignored")).unwrap();
        fs::rename(&large, secondary.join("ignored/large.bin")).unwrap();
        assert!(preflight_bootstrap(&source, &esp).is_err());
        assert!(replace_boot_files(&source, &fallback).is_err());
        fs::remove_file(secondary.join("ignored/large.bin")).unwrap();
        preflight_bootstrap(&source, &esp).unwrap();
        replace_boot_files(&source, &fallback).unwrap();
        assert_eq!(fs::read(fallback.join("HvLoader.efi")).unwrap(), b"new");
        assert!(fallback.join("Windows/System32/ignored").is_dir());
    }

    /// Temporary suffixes are reserved regardless of filename casing.
    #[test]
    fn accepts_additional_files_but_rejects_temporary_source_names() {
        let temp = TempDir::new().unwrap();
        for name in FILES {
            let path = temp.path().join(name.to_ascii_uppercase());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"payload").unwrap();
        }
        boot_files(temp.path(), false).unwrap();
        let pending = temp.path().join("SECOND.EFI.NEW");
        fs::write(&pending, b"pending").unwrap();
        assert!(boot_files(temp.path(), false).is_err());
        boot_files(temp.path(), true).unwrap();
        fs::remove_file(pending).unwrap();
        fs::write(temp.path().join("IDENTITY.JSON"), b"unexpected").unwrap();
        boot_files(temp.path(), true).unwrap();
    }

    /// Invalid sources fail before destination creation.
    #[test]
    fn rejects_special_source_entries_before_destination_creation() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("future-loader.efi"), b"loader").unwrap();
        unistd::mkfifo(&source.join("unsafe"), Mode::S_IRUSR).unwrap();
        copy_boot_files(&source, &target).unwrap_err();
        assert!(!target.exists());
    }

    /// Validation rejects source and destination symlinks.
    #[test]
    fn rejects_source_and_destination_symlinks() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        let outside = temp.path().join("outside");
        create_boot_files(&source, b"new");
        fs::create_dir(&outside).unwrap();
        unix_fs::symlink(&outside, &target).unwrap();
        assert!(replace_boot_files(&source, &target).is_err());
        fs::remove_file(&target).unwrap();
        fs::remove_file(source.join("lxhvloader.dll")).unwrap();
        unix_fs::symlink(outside.join("dll"), source.join("lxhvloader.dll")).unwrap();
        assert!(copy_boot_files(&source, &target).is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
    }

    /// Retrying publication replaces stale files and removes backups.
    #[test]
    fn retries_partial_publication_and_removes_backups() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("AZLB");
        let fallback = temp.path().join("BOOT");
        create_boot_files(&source, b"new");
        create_boot_files(&fallback, b"old");
        fs::rename(
            fallback.join("HvLoader.efi"),
            fallback.join("HvLoader.efi.old"),
        )
        .unwrap();
        fs::write(fallback.join("second.efi.new"), b"partial").unwrap();
        fs::write(fallback.join("lxhvloader.dll.old"), b"stale").unwrap();
        replace_boot_files(&source, &fallback).unwrap();
        replace_boot_files(&source, &fallback).unwrap();
        filepaths(&fallback).unwrap();
        for name in FILES {
            assert_eq!(fs::read(fallback.join(name)).unwrap(), b"new");
            assert!(!fallback.join(format!("{name}.new")).exists());
            assert!(!fallback.join(format!("{name}.old")).exists());
        }
        assert!(replace_boot_files(&source, &source).is_err());
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

    /// Enabled fallback policies copy all slot boot files and preserve the retained slot.
    #[test]
    fn fallback_modes_use_selected_boot_files_during_trial_commit_and_rollback() {
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
            create_boot_files(&active, b"retained");
            create_boot_files(&target, b"candidate");
            create_boot_files(&fallback, b"retained");
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
