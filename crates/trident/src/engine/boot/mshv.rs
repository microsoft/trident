use std::{fs, io::ErrorKind, path::Path};

use anyhow::{ensure, Context, Result};
use nix::sys::statvfs::statvfs;
use osutils::uki::uki_addon_dir;
use trident_api::{config::UefiFallbackMode, constants::EFI_DEFAULT_BIN_RELATIVE_PATH};

use crate::engine::{boot::uki, EngineContext};

// This hardcoded x64 payload list limits the current implementation to x86-64.
const FILES: &[&str] = &[
    "bootx64.efi",
    "grubx64.efi",
    "second.efi",
    "HvLoader.efi",
    "lxhvloader.dll",
    "Windows/System32/hvix64.exe",
    "Windows/System32/hvax64.exe",
    "Windows/System32/kdstub.dll",
];

const SPACE_RESERVE: u64 = 1024 * 1024;

/// Detects slot-owned MSHV in BOOT, requiring the complete bundle when markers exist.
/// No markers means mode off; incomplete bundles and inspection errors fail discovery.
pub(crate) fn has_slot_mshv_bundle(esp_path: &Path) -> Result<bool> {
    let bundle = esp_path.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
    for marker in ["HvLoader.efi", "lxhvloader.dll", "Windows"] {
        match fs::symlink_metadata(bundle.join(marker)) {
            Ok(_) => {
                validate_bundle(&bundle)?;
                return Ok(true);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("Failed to inspect MSHV BOOT payload"),
        }
    }
    Ok(false)
}

/// Checks allowed entries without requiring every payload file to be present.
/// Replacement destinations may also contain known `.new` and `.old` files.
fn validate_tree(root: &Path, directory: &Path, temporary: bool) -> Result<()> {
    ensure!(
        fs::symlink_metadata(directory)?.is_dir(),
        "Invalid payload directory"
    );
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?;
        let relative_name = relative.to_str().context("Non-UTF-8 MSHV payload path")?;
        let kind = entry.file_type()?;
        if kind.is_dir() {
            ensure!(
                relative_name.eq_ignore_ascii_case("Windows")
                    || relative_name.eq_ignore_ascii_case("Windows/System32"),
                "Unexpected payload directory: {}",
                relative.display()
            );
            validate_tree(root, &path, temporary)?;
        } else {
            ensure!(
                kind.is_file(),
                "Payload entry is not a regular file: {}",
                path.display()
            );
            ensure!(
                FILES.iter().any(|name| {
                    relative_name.eq_ignore_ascii_case(name)
                        || temporary
                            && (relative_name.eq_ignore_ascii_case(&format!("{name}.new"))
                                || relative_name.eq_ignore_ascii_case(&format!("{name}.old")))
                }),
                "Unexpected payload file: {}",
                relative.display()
            );
        }
    }
    Ok(())
}

/// Requires the complete supported payload and rejects unexpected entries.
/// Validates layout, not executable contents or the running hypervisor.
pub(crate) fn validate_bundle(root: &Path) -> Result<()> {
    validate_tree(root, root, false)?;
    for name in FILES {
        ensure!(
            fs::symlink_metadata(root.join(name))
                .with_context(|| format!("Missing MSHV payload file: {name}"))?
                .is_file(),
            "Invalid MSHV payload file: {name}"
        );
    }
    Ok(())
}

/// Estimates allocated bytes using the destination cluster size, including directories.
/// Missing paths contribute zero.
fn allocation(path: &Path, cluster: u64) -> Result<u64> {
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

/// Requires room for the incoming allocation plus a reserve, crediting planned reclamation.
fn check_space(available: u64, reclaimed: u64, required: u64) -> Result<()> {
    ensure!(
        available.saturating_add(reclaimed) >= required.saturating_add(SPACE_RESERVE),
        "Insufficient ESP capacity: need {required} bytes plus {SPACE_RESERVE} bytes reserve, \
         have {available} bytes free and {reclaimed} bytes reclaimable from the inactive slot"
    );
    Ok(())
}

/// Estimates staging space for the slot bundle, UKIs, addons, and an enabled fallback copy.
/// Credits inactive-slot contents and UKIs/addons scheduled for removal before staging.
pub(crate) fn preflight_staging(
    ctx: &EngineContext,
    source_esp: &Path,
    destination_esp: &Path,
    target_bundle: &Path,
) -> Result<()> {
    let stats = statvfs(destination_esp)?;
    let cluster = stats.fragment_size();
    ensure!(cluster > 0, "Invalid ESP allocation unit");
    let bundle = allocation(&source_esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH), cluster)?;
    let mut required = bundle + allocation(&source_esp.join(uki::UKI_DIRECTORY), cluster)?;
    // Account for slot-addon templates whose selected file may replace a smaller live addon.
    required += allocation(&source_esp.join("acl/uki-addons"), cluster)?;
    if ctx.spec.os.uefi_fallback != UefiFallbackMode::Disabled {
        let existing_fallback = destination_esp.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        required += bundle.max(allocation(&existing_fallback, cluster)?);
    }
    let mut reclaimed = allocation(target_bundle, cluster)?;
    for path in
        uki::find_ukis_to_remove_before_staging(ctx, &destination_esp.join(uki::UKI_DIRECTORY))?
    {
        reclaimed += allocation(&path, cluster)? + allocation(&uki_addon_dir(&path), cluster)?;
    }
    check_space(stats.blocks_available() * cluster, reclaimed, required)
}

/// Checks room for a new bundle copy without reclaiming existing ESP contents.
pub(crate) fn preflight_bootstrap(from: &Path, esp: &Path) -> Result<()> {
    let stats = statvfs(esp)?;
    let cluster = stats.fragment_size();
    ensure!(cluster > 0, "Invalid ESP allocation unit");
    check_space(
        stats.blocks_available() * cluster,
        0,
        allocation(from, cluster)?,
    )
}

/// Copies a complete validated bundle into an allowlisted destination.
/// Callers are responsible for temporary-directory staging and atomic publication.
pub(crate) fn copy_bundle(from: &Path, to: &Path) -> Result<()> {
    validate_bundle(from)?;
    fs::create_dir_all(to)?;
    validate_tree(to, to, false)?;
    for name in FILES {
        let destination = to.join(name);
        fs::create_dir_all(destination.parent().context("Missing payload parent")?)?;
        fs::copy(from.join(name), destination)?;
    }
    Ok(())
}

/// Publishes the fallback bundle using per-file `.new` and `.old` names.
/// Reconciles leftover `.old` files, but whole-bundle replacement is not atomic.
pub(crate) fn replace_bundle(from: &Path, to: &Path) -> Result<()> {
    validate_bundle(from)?;
    match fs::symlink_metadata(to) {
        Ok(_) => {
            validate_tree(to, to, true)?;
            ensure!(
                fs::canonicalize(from)? != fs::canonicalize(to)?,
                "Source and fallback directories must differ"
            );
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let stats = statvfs(to.parent().context("Missing fallback parent")?)?;
    let cluster = stats.fragment_size();
    ensure!(cluster > 0, "Invalid ESP allocation unit");
    let required = allocation(from, cluster)?;
    let mut reclaimed = 0;
    for name in FILES {
        reclaimed += allocation(&to.join(format!("{name}.new")), cluster)?;
    }
    check_space(stats.blocks_available() * cluster, reclaimed, required)?;
    for name in FILES {
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
    for name in FILES {
        let destination = to.join(name);
        if destination.exists() {
            fs::rename(&destination, to.join(format!("{name}.old")))?;
        }
    }
    for name in FILES {
        fs::rename(to.join(format!("{name}.new")), to.join(name))?;
    }
    for name in FILES {
        let previous = to.join(format!("{name}.old"));
        if previous.exists() {
            fs::remove_file(previous)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use tempfile::TempDir;

    fn bundle(root: &Path, contents: &[u8]) {
        for name in FILES {
            let path = root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
    }

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

    #[test]
    fn rejects_source_and_destination_symlinks() {
        let temp = TempDir::new().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        let outside = temp.path().join("outside");
        bundle(&source, b"new");
        fs::create_dir(&outside).unwrap();
        symlink(&outside, &target).unwrap();
        assert!(replace_bundle(&source, &target).is_err());
        fs::remove_file(&target).unwrap();
        fs::remove_file(source.join("lxhvloader.dll")).unwrap();
        symlink(outside.join("dll"), source.join("lxhvloader.dll")).unwrap();
        assert!(copy_bundle(&source, &target).is_err());
        assert!(fs::read_dir(&outside).unwrap().next().is_none());
    }

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

    #[test]
    fn capacity_accounts_for_clusters_and_preserves_a_reserve() {
        let temp = TempDir::new().unwrap();
        fs::write(temp.path().join("file"), b"x").unwrap();
        assert_eq!(allocation(&temp.path().join("file"), 4096).unwrap(), 4096);
        assert!(check_space(SPACE_RESERVE, 0, 1).is_err());
        assert!(check_space(SPACE_RESERVE, 4096, 4096).is_ok());
    }

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
        fs::create_dir_all(source.join(uki::UKI_DIRECTORY)).unwrap();
        let stats = statvfs(&destination).unwrap();
        fs::File::create(source.join(uki::UKI_DIRECTORY).join("new.efi"))
            .unwrap()
            .set_len(stats.blocks_available() * stats.fragment_size() + SPACE_RESERVE)
            .unwrap();
        let ctx = EngineContext {
            servicing_type: trident_api::status::ServicingType::AbUpdate,
            ab_active_volume: Some(trident_api::status::AbVolumeSelection::VolumeA),
            ..Default::default()
        };
        assert!(preflight_staging(&ctx, &source, &destination, &target).is_err());
        for name in FILES {
            assert_eq!(fs::read(target.join(name)).unwrap(), b"inactive");
            assert_eq!(fs::read(retained.join(name)).unwrap(), b"committed");
        }
    }

    #[test]
    fn fallback_modes_use_the_full_selected_bundle_during_trial_commit_and_rollback() {
        use trident_api::{
            constants::internal_params::MSHV_SLOT_ESP_PAYLOAD,
            status::{AbVolumeSelection, ServicingState, ServicingType},
        };
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
            let mut ctx = EngineContext {
                servicing_type: ServicingType::AbUpdate,
                ab_active_volume: Some(AbVolumeSelection::VolumeA),
                install_index: 1,
                esp_mount_path: ".".into(),
                ..Default::default()
            };
            ctx.spec.internal_params.set_flag(MSHV_SLOT_ESP_PAYLOAD);
            ctx.spec.os.uefi_fallback = mode;
            crate::subsystems::esp::set_uefi_fallback_contents(
                &ctx,
                ServicingState::AbUpdateStaged,
                temp.path(),
            )
            .unwrap();
            for name in FILES {
                let expected = if mode == UefiFallbackMode::Optimistic {
                    b"candidate".as_slice()
                } else {
                    b"retained".as_slice()
                };
                assert_eq!(fs::read(fallback.join(name)).unwrap(), expected);
            }
            crate::subsystems::esp::set_uefi_fallback_contents(
                &ctx,
                ServicingState::AbUpdateFinalized,
                temp.path(),
            )
            .unwrap();
            for name in FILES {
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
            crate::subsystems::esp::set_uefi_fallback_contents(
                &ctx,
                ServicingState::ManualRollbackAbStaged,
                temp.path(),
            )
            .unwrap();
            for name in FILES {
                assert_eq!(fs::read(fallback.join(name)).unwrap(), b"retained");
            }
        }
    }
}
