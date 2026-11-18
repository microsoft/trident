//! Copies complete installed boot trees without interpreting their bootloader.
//!
//! Image-specific completeness belongs to staging. Relocation
//! preserves unfamiliar files and directories while enforcing structural safety.
//! Replacement owns the destination's entire contents, not individual filenames.
//! Space exhaustion is reported by copying, not a separate capacity estimate.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{ensure, Context, Error};
use walkdir::WalkDir;

use osutils::files;

/// Returns sorted root-relative file paths, validating empty directories without listing them.
/// `temporary` permits replacement suffixes in destinations left by interrupted work.
fn boot_files(root: &Path, temporary: bool) -> Result<Vec<PathBuf>, Error> {
    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .follow_root_links(false)
    {
        let entry = entry?;
        let kind = entry.file_type();
        if entry.depth() == 0 {
            ensure!(kind.is_dir(), "Invalid boot directory: {}", root.display());
            continue;
        }
        let name = entry
            .file_name()
            .to_str()
            .context("Non-UTF-8 boot file name")?;
        ensure!(
            temporary || !is_temporary(name),
            "Reserved boot file name: {name}"
        );
        if kind.is_file() {
            files.push(entry.path().strip_prefix(root)?.to_path_buf());
        } else {
            ensure!(
                kind.is_dir(),
                "Not a regular boot file: {}",
                entry.path().display()
            );
        }
    }
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

/// Copies the fallback boot tree into temporary slot-bootstrap staging.
/// An exact copy requires an empty destination. Copy failures may leave partial
/// contents for the caller to discard, as bootstrap does with its staging directory.
pub(crate) fn copy_fallback_for_bootstrap(from: &Path, to: &Path) -> Result<(), Error> {
    boot_files(from, false)?;
    if to.try_exists()? {
        boot_files(to, true)?;
    }
    files::ensure_disjoint_paths(from, to)?;
    files::copy_recursively(from, to)
}

/// Replaces the UEFI fallback's complete boot tree, including nested and empty
/// directories. Destination-only entries are retired after the new tree is staged.
/// Copy failures propagate before current entries are renamed; retries discard partial staging.
///
/// Each top-level entry is staged as `.new`, backed up as `.old`, then published.
/// Directories move with their complete subtrees. Publication across entries is
/// not atomic: failure can leave a mix of current, staged and backup entries.
/// Retries rebuild staging and recover selected backups whose current entry is absent.
pub(crate) fn replace_fallback_boot_files(from: &Path, to: &Path) -> Result<(), Error> {
    // Reject malformed or overlapping trees before modifying destination entries.
    boot_files(from, false)?;
    if to.try_exists()? {
        boot_files(to, true)?;
    }
    files::ensure_disjoint_paths(from, to)?;
    let mut entries = fs::read_dir(from)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    entries.sort();

    // Rebuild all incoming staging while leaving current entries available.
    fs::create_dir_all(to)?;
    for name in &entries {
        let destination = to.join(name);
        let previous = to.join(format!("{}.old", name.to_string_lossy()));
        // A missing current entry may be an interrupted rename. Restore its backup
        // before risking another copy; otherwise the current entry wins.
        if previous.try_exists()? {
            if destination.try_exists()? {
                files::remove_recursively_if_exists(&previous)?;
            } else {
                fs::rename(&previous, &destination)?;
            }
        }
        // Never merge a partial staged directory: it may contain retired files.
        let staged = to.join(format!("{}.new", name.to_string_lossy()));
        files::remove_recursively_if_exists(&staged)?;
        files::copy_recursively(from.join(name), &staged)?;
    }

    // Snapshot before renaming so directory iteration cannot skip changed entries.
    // Back up destination-only entries too: ownership is of the complete tree.
    for entry in fs::read_dir(to)?.collect::<Result<Vec<_>, _>>()? {
        let name = entry.file_name();
        let name = name.to_str().context("Non-UTF-8 boot file name")?;
        if !is_temporary(name) {
            let previous = to.join(format!("{name}.old"));
            files::remove_recursively_if_exists(&previous)?;
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
            files::remove_recursively_if_exists(entry.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::os::unix::fs as unix_fs;

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
            copy_fallback_for_bootstrap(&source, &target).unwrap();
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
        copy_fallback_for_bootstrap(&source, &target).unwrap();
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
            replace_fallback_boot_files(&source, &fallback).unwrap_err();
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
        copy_fallback_for_bootstrap(&source, &target).unwrap();
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

        copy_fallback_for_bootstrap(&source, &target).unwrap();
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
        copy_fallback_for_bootstrap(&source, &without_secondary).unwrap();
        assert!(!without_secondary.join("Windows").exists());
        fs::create_dir_all(&secondary).unwrap();
        let empty_secondary = temp.path().join("empty-secondary");
        copy_fallback_for_bootstrap(&source, &empty_secondary).unwrap();
        assert!(empty_secondary.join("Windows/System32").is_dir());

        for name in ["payload.dll.new", "payload.dll.old"] {
            let path = secondary.join(name);
            fs::write(&path, b"reserved temporary name").unwrap();
            assert!(filepaths(&source).is_err());
            fs::remove_file(path).unwrap();
        }
        unix_fs::symlink(temp.path().join("outside"), secondary.join("linked.dll")).unwrap();
        let rejected = temp.path().join("rejected");
        assert!(copy_fallback_for_bootstrap(&source, &rejected).is_err());
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
        replace_fallback_boot_files(&source, &target).unwrap();
        replace_fallback_boot_files(&source, &target).unwrap();
        assert_eq!(fs::read(target.join("component/current")).unwrap(), b"new");
        assert_eq!(fs::read(target.join("loader")).unwrap(), b"loader");
        assert!(!target.join("component/retired").exists());
        assert!(!target.join("loader.old").exists());
        assert!(replace_fallback_boot_files(&source, &source.join("child")).is_err());
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

        replace_fallback_boot_files(&source, &fallback).unwrap();
        replace_fallback_boot_files(&source, &fallback).unwrap();
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
        replace_fallback_boot_files(&source, &fallback).unwrap();
        assert!(!fallback.join(secondary).join("current.dll").exists());
        filepaths(&fallback).unwrap();
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
        copy_fallback_for_bootstrap(&source, &target).unwrap_err();
        assert!(!target.exists());
    }

    /// Validation rejects source and destination symlinks.
    #[test]
    fn rejects_source_root_symlinks_before_destination_creation() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let linked_source = temp.path().join("linked-source");
        let target = temp.path().join("target");
        create_boot_files(&source, b"source");
        unix_fs::symlink(&source, &linked_source).unwrap();
        filepaths(&linked_source).unwrap_err();
        copy_fallback_for_bootstrap(&linked_source, &target).unwrap_err();
        assert!(!target.exists());
    }

    #[test]
    fn rejects_reserved_empty_directories_before_destination_creation() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        fs::create_dir_all(source.join("nested/component.OLD")).unwrap();
        filepaths(&source).unwrap_err();
        copy_fallback_for_bootstrap(&source, &target).unwrap_err();
        assert!(!target.exists());
    }

    #[test]
    fn rejects_source_and_destination_symlinks() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        let outside = temp.path().join("outside");
        create_boot_files(&source, b"new");
        fs::create_dir(&outside).unwrap();
        unix_fs::symlink(&outside, &target).unwrap();
        assert!(replace_fallback_boot_files(&source, &target).is_err());
        fs::remove_file(&target).unwrap();
        fs::remove_file(source.join("lxhvloader.dll")).unwrap();
        unix_fs::symlink(outside.join("dll"), source.join("lxhvloader.dll")).unwrap();
        assert!(copy_fallback_for_bootstrap(&source, &target).is_err());
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
        replace_fallback_boot_files(&source, &fallback).unwrap();
        replace_fallback_boot_files(&source, &fallback).unwrap();
        filepaths(&fallback).unwrap();
        for name in FILES {
            assert_eq!(fs::read(fallback.join(name)).unwrap(), b"new");
            assert!(!fallback.join(format!("{name}.new")).exists());
            assert!(!fallback.join(format!("{name}.old")).exists());
        }
        assert!(replace_fallback_boot_files(&source, &source).is_err());
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
