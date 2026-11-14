use std::{
    fs,
    io::Read,
    ops::ControlFlow,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, trace};
use reqwest::Url;
use tempfile::{NamedTempFile, TempDir};

use osutils::{
    blkid,
    bootloaders::{BOOT_EFI, GRUB_EFI, GRUB_NOPREFIX_EFI},
    fatlabel, files,
    filesystems::MountFileSystemType,
    mount::{self, MountGuard},
    path,
};
use trident_api::{
    config::UefiFallbackMode,
    constants::{
        internal_params::DISABLE_GRUB_NOPREFIX_CHECK, EFI_DEFAULT_BIN_DIRECTORY,
        EFI_DEFAULT_BIN_RELATIVE_PATH, ESP_EFI_DIRECTORY, GRUB2_CONFIG_FILENAME,
        GRUB2_CONFIG_RELATIVE_PATH,
    },
    error::{InvalidInputError, ReportError, ServicingError, TridentError, TridentResultExt},
    status::{AbVolumeSelection, ServicingState, ServicingType},
};

use crate::{
    engine::{
        boot::{
            self, mshv as boot_mshv, uki, ESP_EXTRACTION_DIRECTORY, ESP_EXTRACTION_DIRECTORY_MODE,
        },
        EngineContext, Subsystem,
    },
    io_utils::{
        hashing_reader::{HashingReader, HashingReader384},
        image_streamer,
        read_monitor::ReadMonitor,
    },
    osimage::bootloader::Bootloader,
};

mod mshv;

#[derive(Default, Debug)]
pub struct EspSubsystem;
impl Subsystem for EspSubsystem {
    fn name(&self) -> &'static str {
        "esp"
    }

    /// Applies slot-owned MSHV support restrictions before ESP servicing.
    fn validate_host_config(&self, ctx: &EngineContext) -> Result<(), TridentError> {
        mshv::validate_configuration(ctx)
    }

    #[tracing::instrument(name = "esp_provision", skip_all)]
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        // Perform file-based deployment of ESP images, if needed, after filesystems have been
        // mounted and initialized.

        if !ctx.is_stream_image {
            deploy_esp(ctx, mount_path)?;
        }

        Ok(())
    }
}

/// Configures UEFI fallback by copying boot files to the UEFI fallback folder
/// based on the UEFI fallback mode and servicing state.
///
/// UEFI fallback is handled in two stages:
/// 1. During finalize:
///    * Clean install copies the target OS boot files (always volume A).
///    * A/B update copies the candidate before boot validation in optimistic
///      mode, or the retained slot in conservative mode so fallback continues
///      to boot the previously working OS.
///    * Manual A/B rollback copies the rollback target in either enabled mode.
/// 2. During A/B update commit, after boot validation, conservative mode copies
///    the target OS boot files. Optimistic mode needs no further copy.
///
/// Disabled fallback is unchanged. MSHV copies the complete nested bundle.
pub fn set_uefi_fallback_contents(
    ctx: &EngineContext,
    servicing_state: ServicingState,
    mount_point: &Path,
) -> Result<(), Error> {
    let source_esp_dir_name = find_uefi_fallback_source_dir_name(ctx, servicing_state);
    if let Some(source_esp_dir_name) = source_esp_dir_name {
        debug!(
            "Configuring UEFI fallback for servicing state {:?} using source ESP dir '{}'",
            servicing_state, source_esp_dir_name
        );
        copy_boot_files_for_uefi_fallback(
            mount_point,
            ctx.esp_mount_path.as_path(),
            &source_esp_dir_name,
            &ctx.servicing_type,
            ctx.install_index,
        )?;
    } else {
        debug!(
            "No UEFI fallback configuration needed for servicing state {:?}",
            servicing_state
        );
    }
    Ok(())
}

/// Performs file-based deployment of ESP images from the OS image.
fn deploy_esp(ctx: &EngineContext, mount_point: &Path) -> Result<(), TridentError> {
    trace!("Deploying ESP from OS image");

    let os_image = ctx
        .image
        .as_ref()
        .structured(ServicingError::DeployESPImages)
        .message("OS image is required to deploy ESP from OS image")?;

    let esp_img = os_image
        .esp_filesystem()
        .structured(ServicingError::DeployESPImages)
        .message("Failed to get ESP image from OS image")?;

    // Extract the ESP image to a temporary file in
    // `<newroot>/ESP_EXTRACTION_DIRECTORY`. This location is generally
    // guaranteed to be writable and backed by a real block device, so we don't
    // have to store a potentially large ESP image in memory.
    let esp_extraction_dir = ensure_esp_extraction_dir(mount_point)?;

    // Get the threshold and interval for reporting slow streaming speed from
    // the context, to be used in the ReadMonitor while streaming images to the
    // block devices.
    let (threshold_reporting, reporting_interval) = ctx.read_monitor_params()?;

    let mut found_esp = false;
    os_image.read_images(|path, stream| {
        if path != esp_img.image_file.path {
            return ControlFlow::Continue(());
        }

        let monitored_reader = ReadMonitor::new(
            stream,
            "esp",
            esp_img.image_file.compressed_size,
            threshold_reporting,
            reporting_interval,
        )
        .make_reporting("image_read_complete", "esp");

        found_esp = true;
        let (temp_file, computed_sha384) = match load_raw_image(
            ctx,
            &esp_extraction_dir,
            os_image.source(),
            HashingReader384::new(monitored_reader),
        ) {
            Ok(r) => r,
            Err(e) => {
                return ControlFlow::Break(
                    Err(e)
                        .structured(ServicingError::DeployESPImages)
                        .message("Failed to load raw image"),
                )
            }
        };
        if esp_img.image_file.sha384 != computed_sha384 {
            return ControlFlow::Break(
                Err(TridentError::new(ServicingError::DeployESPImages)).message(format!(
                    "SHA384 mismatch for disk image {}: expected {}, got {}",
                    os_image.source(),
                    esp_img.image_file.sha384,
                    computed_sha384
                )),
            );
        }

        ControlFlow::Break(
            copy_file_artifacts(temp_file.path(), ctx, mount_point)
                .and_then(|()| preserve_esp_filesystem_label(temp_file.path(), ctx))
                .structured(ServicingError::DeployESPImages)
                .message("Failed to deploy the ESP image"),
        )
    })?;

    if !found_esp {
        return Err(TridentError::new(InvalidInputError::CorruptOsImage(
            "ESP filesystem listed in OS image but not present".into(),
        )));
    }

    Ok(())
}

/// Returns the directory under `mount_point` to stage the ESP image in,
/// creating it if the OS image does not ship it.
///
/// The staging directory is deliberately on the newly written root rather than
/// in memory, since an ESP image can be large. It is not guaranteed to exist
/// there, though: an immutable OS image may ship a root filesystem with no
/// `/var` at all and leave systemd-tmpfiles to populate it on first boot. In
/// that case create it with the mode `/var/tmp` is expected to have, so that we
/// neither fail the deployment nor leave the installed system with a
/// wrongly-permissioned directory.
fn ensure_esp_extraction_dir(mount_point: &Path) -> Result<PathBuf, TridentError> {
    let esp_extraction_dir = path::join_relative(mount_point, ESP_EXTRACTION_DIRECTORY);

    if !esp_extraction_dir.exists() {
        debug!(
            "Creating ESP staging directory '{}'",
            esp_extraction_dir.display()
        );

        files::create_dirs(&esp_extraction_dir)
            .structured(ServicingError::DeployESPImages)
            .message("Failed to create the ESP staging directory")?;

        fs::set_permissions(
            &esp_extraction_dir,
            fs::Permissions::from_mode(ESP_EXTRACTION_DIRECTORY_MODE),
        )
        .structured(ServicingError::DeployESPImages)
        .message("Failed to set permissions on the ESP staging directory")?;
    }

    Ok(esp_extraction_dir)
}

/// Copies the ESP image's filesystem label onto the ESP Trident created.
///
/// Every other filesystem from the OS image is written to its partition
/// verbatim, so its label arrives with the rest of the bits. The ESP is the
/// exception: it is deployed file by file onto a filesystem Trident makes
/// itself, and `mkfs` gives that filesystem no label. Copying the label across
/// leaves the ESP in the state it would have been in had it been written
/// wholesale, which matters because ACL's initrd looks the ESP up as
/// `/dev/disk/by-label/EFI-SYSTEM` and otherwise waits for a device node that
/// never appears.
///
/// Scoped to ACL rather than applied generally: images that do not refer to
/// their ESP by label have installed correctly without this, and relabelling
/// their ESP would be a behavioural change they have not asked for.
///
/// An image whose ESP has no label leaves the new filesystem unlabelled too,
/// which is still faithful to the image.
fn preserve_esp_filesystem_label(esp_image_path: &Path, ctx: &EngineContext) -> Result<(), Error> {
    if !ctx.image_distro().is_acl() {
        return Ok(());
    }

    // blkid exits successfully with no output when the filesystem simply has no
    // label, and fails only when it cannot read the image at all. Propagate the
    // latter: silently treating an unreadable image as "no label" would skip
    // the labelling and reintroduce the boot failure this exists to prevent.
    let label = blkid::get_filesystem_label(esp_image_path)
        .context("Failed to read the filesystem label of the ESP image")?;

    if label.is_empty() {
        debug!("ESP image has no filesystem label to preserve");
        return Ok(());
    }

    let (esp_device_id, _) = ctx
        .spec
        .storage
        .esp_filesystem()
        .context("Failed to find the ESP filesystem in the Host Configuration")?;

    let esp_device_path = ctx
        .get_block_device_path(esp_device_id)
        .with_context(|| format!("Failed to find the path of ESP device '{esp_device_id}'"))?;

    debug!(
        "Applying ESP image's filesystem label '{label}' to '{}'",
        esp_device_path.display()
    );
    fatlabel::set_label(&esp_device_path, &label).with_context(|| {
        format!(
            "Failed to apply filesystem label '{label}', read from ESP image '{}', to ESP device \
            '{}'",
            esp_image_path.display(),
            esp_device_path.display()
        )
    })
}

/// Takes in a reader to the raw zstd-compressed ESP image and decompresses it
/// into a temporary file under `/<mount_point>/<ESP_EXTRACTION_DIRECTORY>`.
/// Returns a tuple containing the temporary file and the computed hash (SHA256
/// or SHA384) of the image.
///
/// It also takes in the URL of the image to be shown in case of errors.
fn load_raw_image<R>(
    ctx: &EngineContext,
    esp_extraction_dir: &Path,
    source: &Url,
    reader: R,
) -> Result<(NamedTempFile, String), Error>
where
    R: Read + HashingReader,
{
    // Create a temporary file to download ESP image
    trace!(
        "Creating temporary file for ESP image extraction at {}",
        esp_extraction_dir.display()
    );

    let temp_image =
        NamedTempFile::new_in(esp_extraction_dir).context("Failed to create a temporary file")?;
    let temp_image_path = temp_image.path().to_path_buf();

    debug!("Extracting ESP image to {}", temp_image_path.display());

    // Stream image to the temporary file.
    let computed_hash = image_streamer::stream_zstd_and_hash(
        reader,
        &temp_image_path,
        ctx.image_zstd_max_window_log(),
    )
    .context(format!("Failed to stream ESP image from {source}"))?;

    Ok((temp_image, computed_hash))
}

/// Performs file-based update of a stand-alone ESP volume by copying boot files.
///
/// Mounts the source ESP image, copies boot files into the selected target-slot
/// directory, and stages the UKI when applicable. For MSHV, validates the bundle
/// and preflights capacity before reclaiming target-slot boot files and UKIs.
/// Retained-slot payloads are preserved.
fn copy_file_artifacts(
    temp_image_path: &Path,
    ctx: &EngineContext,
    mount_point: &Path,
) -> Result<(), Error> {
    // Create a temporary directory to mount ESP image
    let temp_dir = TempDir::new().context("Failed to create a temporary mount directory")?;
    let temp_mount_dir = temp_dir.path();

    // Mount image to temp dir
    mount::mount(
        temp_image_path,
        temp_mount_dir,
        MountFileSystemType::Vfat,
        &["umask=0077".into()],
    )
    .context(format!(
        "Failed to mount image at path {} to directory {}",
        temp_image_path.display(),
        temp_mount_dir.display()
    ))?;

    // Create a mount guard that will automatically unmount when it goes out of scope
    let _mount_guard = MountGuard {
        mount_dir: temp_mount_dir,
    };

    // Determine which ESP dir to copy boot files into
    let esp_dir_path = generate_efi_bin_base_dir_path(ctx, mount_point)?;

    // Call helper func to copy files from mounted img dir to esp_dir_path
    debug!("Writing boot files to directory {}", esp_dir_path.display());

    // MSHV uses a complete bundle, not the ordinary boot-file list.
    let slot_mshv = ctx.bootloader().is_some_and(Bootloader::uses_mshv);
    let source_bundle = temp_mount_dir.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
    // Compare source metadata with the retained slot before any ESP writes.
    mshv::validate_transition(ctx, &path::join_relative(mount_point, &ctx.esp_mount_path))?;
    let boot_files = if slot_mshv {
        // Validate and budget space before deleting target-slot files.
        boot_mshv::validate_bundle(&source_bundle)?;
        boot_mshv::preflight_staging(
            ctx,
            temp_mount_dir,
            &path::join_relative(mount_point, &ctx.esp_mount_path),
            &esp_dir_path,
        )?;
        uki::cleanup_ukis_before_staging(ctx, mount_point, &ctx.esp_mount_path)?;
        Vec::new()
    } else {
        // Ordinary copying would omit required MSHV payload files.
        ensure!(
            !source_bundle.join("HvLoader.efi").try_exists()?,
            "An MSHV image must declare COSI bootloader type 'efitreeloader-mshv-systemd-boot'"
        );
        // Generate the ordinary boot-file paths from the temporary directory
        // where the source ESP image is mounted.
        generate_boot_filepaths(
            temp_mount_dir,
            ctx.is_uki().unstructured("UKI setting unknown")?,
        )
        .context("Failed to generate boot filepaths")?
    };

    // Clear esp_dir_path if it exists
    if esp_dir_path.exists() {
        debug!("Clearing directory {}", esp_dir_path.display());
        osutils::files::clean_directory(esp_dir_path.clone()).context(format!(
            "Failed to clean directory {}",
            esp_dir_path.display()
        ))?;
    } else {
        // Create esp_dir_path if it doesn't exist
        debug!("Creating directory {}", esp_dir_path.display());
        fs::create_dir_all(esp_dir_path.clone()).context(format!(
            "Failed to create directory {}",
            esp_dir_path.display()
        ))?;
    }

    // Call helper func to copy boot files from temp_mount_dir to esp_dir_path
    let grub_noprefix = if slot_mshv {
        boot_mshv::copy_bundle(&source_bundle, &esp_dir_path)?;
        false
    } else {
        copy_boot_files(temp_mount_dir, &esp_dir_path, boot_files).context(format!(
            "Failed to copy boot files from directory {} to directory {}",
            temp_mount_dir.display(),
            esp_dir_path.display()
        ))?
    };

    if ctx.is_uki().unstructured("UKI setting unknown")? {
        // Prepare ESP directory structure for UKI boot
        uki::prepare_esp_for_uki(mount_point, &ctx.esp_mount_path)?;

        // Clean up old UKIs for the target slot before staging the new one.
        // MSHV already did this after its capacity preflight; other UKI images
        // reclaim space here because the ESP may not fit old and new UKIs
        // simultaneously (e.g. on a 128 MB ESP). A crash between cleanup and
        // staging can remove the target slot's old UKI without a replacement,
        // but the active slot's UKI is preserved, retaining a bootable rollback.
        if !slot_mshv {
            uki::cleanup_ukis_before_staging(ctx, mount_point, &ctx.esp_mount_path)?;
        }

        // Copy the UKI from the image into the ESP directory
        uki::stage_uki_on_esp(temp_mount_dir, mount_point, &ctx.esp_mount_path)?;

        // For ACL A/B images, activate the verity addon matching the target slot.
        // The image ships with slot A's addon active; this swaps it when updating
        // to slot B (or confirms slot A for clean installs). Non-ACL images have
        // no template directory and this is a no-op.
        if ctx.image_distro().is_acl() {
            if let Some(target_volume) = ctx.get_ab_update_volume() {
                uki::activate_verity_addon_for_target_volume(
                    temp_mount_dir,
                    mount_point,
                    &ctx.esp_mount_path,
                    target_volume,
                )?;

                // ACL images ship the first-boot addon pre-populated in the
                // image's live .extra.d/ (already staged verbatim above by
                // stage_uki_on_esp) so a genuine first boot works out of the
                // box. But every ACL AB-update COSI is built from a
                // never-booted image, so that addon is still present even
                // when this is not actually a first boot. Enforce that it's
                // only ever staged for a clean install; strip it otherwise
                // so Ignition doesn't re-trigger and hang on the post-update
                // reboot.
                uki::enforce_firstboot_addon_policy(
                    mount_point,
                    &ctx.esp_mount_path,
                    ctx.servicing_type,
                )?;
            }
        }
    } else {
        // In non-UKI mode, bail if grub_noprefix.efi is not found in the image.
        ensure!(
            grub_noprefix
                || ctx
                    .spec
                    .internal_params
                    .get_flag(DISABLE_GRUB_NOPREFIX_CHECK),
            "Cannot locate {GRUB_NOPREFIX_EFI} in the boot image. \
                Verify if the grub2-efi-binary-noprefix package was installed on the booted image.",
        );
    }

    Ok(())
}

/// Determines the source directory to copy UEFI fallback boot files from.
/// This varies based on the servicing state and UEFI fallback mode.
///
/// 1. During finalize:
///    * Clean install uses volume A, the target OS volume.
///    * A/B update:
///      - Optimistic uses the opposite of the recorded active volume, which
///        contains the newly staged candidate's boot files.
///      - Conservative uses the recorded active volume, retaining the
///        previously working OS as fallback (this may be a redundant copy).
///    * Manual A/B rollback uses the opposite of the recorded active volume,
///      the rollback target, in either enabled mode.
/// 2. During commit, after the target OS boot has been verified:
///    * Clean install needs no copy because finalize already populated fallback.
///    * A/B update:
///      - Optimistic needs no copy because finalize already copied the candidate.
///      - Conservative uses the validated target volume from
///        `ctx.get_ab_update_volume()`. The recorded active volume still refers
///        to the pre-update OS until commit updates Host Status.
///
/// Returns `None` to leave fallback unchanged when disabled or for unlisted
/// states. A missing active volume defaults to A for `AbUpdateStaged` and
/// `ManualRollbackAbStaged`.
fn find_uefi_fallback_source_dir_name(
    ctx: &EngineContext,
    servicing_state: ServicingState,
) -> Option<String> {
    trace!(
        "Finding UEFI fallback source dir name for servicing state {:?} and UEFI fallback mode {:?}",
        servicing_state,
        ctx.spec.os.uefi_fallback
    );
    match servicing_state {
        ServicingState::CleanInstallStaged => match ctx.spec.os.uefi_fallback {
            UefiFallbackMode::Optimistic | UefiFallbackMode::Conservative => Some(
                boot::make_esp_dir_name(ctx.install_index, AbVolumeSelection::VolumeA),
            ),
            _ => None,
        },
        ServicingState::AbUpdateStaged => match ctx.spec.os.uefi_fallback {
            UefiFallbackMode::Optimistic => Some(boot::make_esp_dir_name(
                ctx.install_index,
                match ctx.ab_active_volume {
                    None | Some(AbVolumeSelection::VolumeB) => AbVolumeSelection::VolumeA,
                    Some(AbVolumeSelection::VolumeA) => AbVolumeSelection::VolumeB,
                },
            )),
            UefiFallbackMode::Conservative => Some(boot::make_esp_dir_name(
                ctx.install_index,
                match ctx.ab_active_volume {
                    None | Some(AbVolumeSelection::VolumeA) => AbVolumeSelection::VolumeA,
                    Some(AbVolumeSelection::VolumeB) => AbVolumeSelection::VolumeB,
                },
            )),
            _ => None,
        },
        ServicingState::AbUpdateFinalized => match ctx.spec.os.uefi_fallback {
            UefiFallbackMode::Conservative => {
                // The recorded active slot is still pre-update; copy the validated target.
                ctx.get_ab_update_volume()
                    .map(|volume| boot::make_esp_dir_name(ctx.install_index, volume))
            }
            _ => None,
        },
        ServicingState::ManualRollbackAbStaged => match ctx.spec.os.uefi_fallback {
            UefiFallbackMode::Conservative | UefiFallbackMode::Optimistic => {
                Some(boot::make_esp_dir_name(
                    ctx.install_index,
                    match ctx.ab_active_volume {
                        None | Some(AbVolumeSelection::VolumeB) => AbVolumeSelection::VolumeA,
                        Some(AbVolumeSelection::VolumeA) => AbVolumeSelection::VolumeB,
                    },
                ))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Skip UEFI fallback copy when AZLA and AZLB boot paths do not exist. This
/// is an expected VM scenario when offline-initialize is used to prepare a
/// VHD-based image for A/B update.
fn skip_uefi_fallback_copy(
    esp_dir_path: &Path,
    source_esp_name: &str,
    servicing_type: &ServicingType,
    install_index: usize,
) -> bool {
    let azla_dir_name = boot::make_esp_dir_name(install_index, AbVolumeSelection::VolumeA);
    if azla_dir_name == source_esp_name && servicing_type == &ServicingType::AbUpdate {
        // For A/B update, check for the AZLA **not** existing and
        // AZLB existing. In this case, skip the UEFI fallback copy
        if !esp_dir_path.join(azla_dir_name).exists()
            && esp_dir_path
                .join(boot::make_esp_dir_name(
                    install_index,
                    AbVolumeSelection::VolumeB,
                ))
                .exists()
        {
            return true;
        }
    }
    false
}

/// Copies boot files from the `source_esp_name` folder to the UEFI fallback
/// folder, `EFI/BOOT`.
///
/// Copies the complete nested bundle for MSHV, or top-level files for ordinary
/// bootloaders, replacing existing fallback files.
/// Skips copying from an unseeded AZLA during A/B updates.
fn copy_boot_files_for_uefi_fallback(
    mount_point: &Path,
    esp_mount_path: &Path,
    source_esp_name: &str,
    servicing_type: &ServicingType,
    install_index: usize,
) -> Result<(), Error> {
    let esp_dir_path = path::join_relative(mount_point, esp_mount_path).join(ESP_EFI_DIRECTORY);
    if skip_uefi_fallback_copy(
        &esp_dir_path,
        source_esp_name,
        servicing_type,
        install_index,
    ) {
        debug!(
            "Skipping UEFI fallback copy for servicing type {:?}",
            servicing_type
        );
        return Ok(());
    }

    let source_esp_dir_path = esp_dir_path.join(source_esp_name);
    let uefi_fallback_path = esp_dir_path.join(EFI_DEFAULT_BIN_DIRECTORY);
    debug!(
        "Copying files during {:?} from '{}' to '{}'",
        servicing_type,
        source_esp_dir_path.display(),
        uefi_fallback_path.display()
    );
    // Fallback also runs without an image, so detect the selected slot's bundle.
    let result = if boot_mshv::has_slot_mshv_bundle(&source_esp_dir_path)? {
        boot_mshv::replace_bundle(&source_esp_dir_path, &uefi_fallback_path)
    } else {
        replace_boot_files(&source_esp_dir_path, &uefi_fallback_path)
    };
    result.context(format!(
        "Failed to copy boot files from directory '{}' to directory '{}'",
        source_esp_dir_path.display(),
        uefi_fallback_path.display()
    ))?;
    Ok(())
}

/// Copies boot files from one folder to another.
pub fn replace_boot_files(from_dir: &Path, to_dir: &Path) -> Result<(), Error> {
    trace!(
        "Copying boot files from '{}' to '{}'",
        from_dir.display(),
        to_dir.display()
    );
    // Ensure to_dir exists
    fs::create_dir_all(to_dir)
        .context(format!("Failed to create directory '{}'", to_dir.display()))?;

    // Copy all files from from_dir to to_dir as <existing_filename>.new
    fs::read_dir(from_dir)?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .try_for_each(|from_path| {
            let to_file_name = format!("{}.new", from_path.file_name().to_string_lossy());
            let to_path = to_dir.join(to_file_name);
            fs::copy(from_path.path(), &to_path).context(format!(
                "Failed to copy file '{}' to '{}'",
                from_path.path().display(),
                to_path.display(),
            ))?;
            trace!(
                "Copied file '{}' to '{}'",
                from_path.path().display(),
                to_path.display()
            );
            Ok::<(), Error>(())
        })
        .context("Failed to copy files")?;

    // Rename pre-existing entries from to_dir/<filename> to to_dir/<filename>.old.
    fs::read_dir(to_dir)?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .try_for_each(|orig_path| {
            let orig_file_name = orig_path.file_name();
            let orig_file_name_string = orig_file_name.to_string_lossy();
            // Leave staged files and pending cleanup entries in place.
            if !orig_file_name_string.ends_with(".new") && !orig_file_name_string.ends_with(".old")
            {
                let new_file_name = format!("{orig_file_name_string}.old");
                let to_path = to_dir.join(new_file_name);
                fs::rename(orig_path.path(), &to_path).context(format!(
                    "Failed to rename pre-existing file '{}' to '{}'",
                    orig_path.path().display(),
                    to_path.display()
                ))?;
                trace!(
                    "Renamed pre-existing file '{}' to '{}'",
                    orig_path.path().display(),
                    to_path.display()
                );
            }
            Ok::<(), Error>(())
        })
        .context("Failed to rename pre-existing files")?;

    // Rename all copied files from to_dir/<filename>.new to to_dir/<filename>
    fs::read_dir(to_dir)?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .try_for_each(|orig_path| {
            let orig_file_name = orig_path.file_name();
            let orig_file_name_string = orig_file_name.to_string_lossy();
            // Skip files that do not end with .new
            if orig_file_name_string.ends_with(".new") {
                let new_file_name = orig_file_name_string.trim_end_matches(".new");
                let to_path = to_dir.join(new_file_name);
                fs::rename(orig_path.path(), &to_path).context(format!(
                    "Failed to rename copied file '{}' to '{}'",
                    orig_path.path().display(),
                    to_path.display()
                ))?;
                trace!(
                    "Renamed copied file '{}' to '{}'",
                    orig_path.path().display(),
                    to_path.display()
                );
            }
            Ok::<(), Error>(())
        })
        .context("Failed to rename copied files")?;

    // Remove retired files and the known MSHV payload directory.
    fs::read_dir(to_dir)?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .try_for_each(|orig_path| {
            let orig_file_name = orig_path.file_name();
            // Skip files that do not end with .old
            if orig_file_name.to_string_lossy().ends_with(".old") {
                let result = if orig_path.file_type()?.is_dir()
                    && orig_file_name
                        .to_string_lossy()
                        .eq_ignore_ascii_case("Windows.old")
                {
                    fs::remove_dir_all(orig_path.path())
                } else {
                    fs::remove_file(orig_path.path())
                };
                result.context(format!(
                    "Failed to remove pre-existing entry '{}'",
                    orig_path.path().display()
                ))?;
                trace!(
                    "Removed pre-existing entry '{}'",
                    orig_path.path().display()
                );
            }
            Ok::<(), Error>(())
        })
        .context("Failed to remove pre-existing files")?;
    Ok(())
}

/// Ensures `EFI/AZLA` exists on the ESP, seeding it from the `EFI/BOOT` fallback
/// loader if it is missing. Used by offline-initialize and ACL lazy adoption so
/// a freshly derived HostStatus is backed by the expected on-disk ESP layout
/// before the first A/B update or manual rollback assumes AZLA already exists.
///
/// Detects MSHV from BOOT, preflights capacity for its bundle, and publishes a
/// complete copy via a staged directory rename. Existing AZLA is never refreshed:
/// it is validated when BOOT contains MSHV, or left untouched for ordinary BOOT.
pub fn bootstrap_azla_from_fallback(esp_path: &Path) -> Result<(), Error> {
    let azla_esp_path = esp_path.join(ESP_EFI_DIRECTORY).join("AZLA");
    let boot_esp_path = esp_path.join(EFI_DEFAULT_BIN_RELATIVE_PATH);
    let slot_mshv = boot_mshv::has_slot_mshv_bundle(&boot_esp_path)?;
    if slot_mshv && azla_esp_path.exists() {
        return boot_mshv::validate_bundle(&azla_esp_path);
    }
    trace!("Checking for AZLA volume ESP path at {:?}", &azla_esp_path);
    if !azla_esp_path.exists() {
        trace!(
            "AZLA volume ESP path {:?} does not exist, attempting to copy from fallback location",
            azla_esp_path
        );
        // Stage the copy in a temporary sibling directory and publish it via
        // an atomic rename only once the copy has fully succeeded. Creating
        // AZLA directly before the copy completes could leave a partially
        // populated directory on failure. Ordinary-mode retries would skip
        // that directory because it exists, while MSHV retries would reject
        // its incomplete bundle rather than repair it.
        let azla_tmp_path = esp_path.join(ESP_EFI_DIRECTORY).join("AZLA.bootstrap-tmp");
        if azla_tmp_path.exists() {
            fs::remove_dir_all(&azla_tmp_path).context(format!(
                "Failed to remove stale temporary AZLA bootstrap directory '{}'",
                azla_tmp_path.display()
            ))?;
        }
        let copy_result = if slot_mshv {
            // Check capacity after reclaiming stale staging contents.
            boot_mshv::preflight_bootstrap(&boot_esp_path, esp_path)?;
            boot_mshv::copy_bundle(&boot_esp_path, &azla_tmp_path)
        } else {
            replace_boot_files(&boot_esp_path, &azla_tmp_path)
        };
        copy_result.context(format!(
            "Failed to copy boot files from '{}' to temporary AZLA bootstrap path '{}'",
            boot_esp_path.display(),
            azla_tmp_path.display()
        ))?;
        fs::rename(&azla_tmp_path, &azla_esp_path).context(format!(
            "Failed to atomically publish AZLA bootstrap directory '{}' to '{}'",
            azla_tmp_path.display(),
            azla_esp_path.display()
        ))?;
    }
    Ok(())
}

/// Copies boot files from temp_mount_dir, where image was mounted to, to given dir esp_dir.
///
/// Returns a boolean indicating whether grub-noprefix.efi is used.
fn copy_boot_files(
    temp_mount_dir: &Path,
    esp_dir: &Path,
    boot_files: Vec<PathBuf>,
) -> Result<bool, Error> {
    // Track whether grub-noprefix.efi is used
    let mut no_prefix = false;
    // Copy the specified files from temp_mount_path to esp_dir_path
    for boot_file in boot_files.iter() {
        let source_path = temp_mount_dir.join(boot_file);
        // Extract filename from path
        let file_name = Path::new(boot_file).file_name().context(format!(
            "Failed to extract filename from path '{}'",
            boot_file.display()
        ))?;

        let destination_path = esp_dir.join(file_name);

        // Create directories if they don't exist
        if let Some(parent) = destination_path.parent() {
            fs::create_dir_all(parent)
                .context(format!("Failed to create directory '{}'", parent.display()))?;
        }

        debug!(
            "Copying file '{}' to '{}'",
            source_path.display(),
            destination_path.display()
        );
        fs::copy(&source_path, &destination_path).context(format!(
            "Failed to copy file '{}' to '{}'",
            source_path.display(),
            destination_path.display()
        ))?;

        // Rename grub-noprefix efi to grub efi
        if file_name == GRUB_NOPREFIX_EFI {
            fs::rename(
                &destination_path,
                esp_dir
                    .join(GRUB_EFI)
                    .to_str()
                    .context("Failed to convert path to string")?,
            )
            .context("Failed to rename grub-noprefix efi")?;
            no_prefix = true;
        }
    }

    Ok(no_prefix)
}

/// Generates a list of filepaths to the boot files that need to be copied to implement file-based
/// update of ESP, relative to the mounted directory.
///
/// The func takes the arg temp_mount_dir, which is the path to the directory where the ESP image is mounted to.
/// The is_uki parameter indicates whether the image being processed is a UKI (Unified Kernel Image), in which
/// case, grub.cfg is not needed.
fn generate_boot_filepaths(temp_mount_dir: &Path, is_uki: bool) -> Result<Vec<PathBuf>, Error> {
    let mut paths = Vec::new();

    // Check if grub.cfg exists in EFI_DEFAULT_BIN_RELATIVE_PATH, otherwise use GRUB2_RELATIVE_PATH
    let efi_boot_grub_path = Path::new(temp_mount_dir)
        .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
        .join(GRUB2_CONFIG_FILENAME);

    // Directory in the source ESP image where the GRUB config is located.
    // TODO: In long term, in the source ESP image, the GRUB config will be placed in the same dir as
    // the EFI executables, i.e., /EFI/BOOT/grub.cfg. Related ADO task:
    // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6452.
    let boot_grub2_grub_path = Path::new(temp_mount_dir).join(GRUB2_CONFIG_RELATIVE_PATH);

    if !is_uki {
        let selected_grub_config_path =
            if efi_boot_grub_path.exists() && efi_boot_grub_path.is_file() {
                efi_boot_grub_path
            } else if boot_grub2_grub_path.exists() && boot_grub2_grub_path.is_file() {
                boot_grub2_grub_path
            } else {
                bail!("Failed to find {GRUB2_CONFIG_FILENAME}");
            };

        debug!(
            "Using GRUB configuration file '{GRUB2_CONFIG_FILENAME}' from '{}'",
            selected_grub_config_path.display()
        );
        paths.push(selected_grub_config_path);
    }

    // Check if the grub-noprefix EFI executable exists; otherwise, use the standard
    // grub EFI executable (e.g., grubx64.efi). For example, on AMD64 systems, with
    // the package update to use the grub2-efi-binary-noprefix RPM, the EFI executable
    // would be installed as grubx64-noprefix.efi.
    let grub_efi_noprefix_path = Path::new(temp_mount_dir)
        .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
        .join(GRUB_NOPREFIX_EFI);
    let grub_efi_path = Path::new(temp_mount_dir)
        .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
        .join(GRUB_EFI);

    let selected_grub_binary_path =
        if grub_efi_noprefix_path.exists() && grub_efi_noprefix_path.is_file() {
            grub_efi_noprefix_path
        } else if grub_efi_path.exists() && grub_efi_path.is_file() {
            grub_efi_path
        } else {
            bail!("Failed to find GRUB EFI executable");
        };
    debug!(
        "Using GRUB EFI executable from '{}'",
        selected_grub_binary_path.display()
    );
    paths.push(selected_grub_binary_path);

    // Construct file names of EFI executables
    let boot_efi_path = Path::new(temp_mount_dir)
        .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
        .join(BOOT_EFI);
    if !boot_efi_path.exists() {
        bail!(
            "Failed to find shim EFI executable at path '{}'",
            boot_efi_path.display()
        );
    }
    debug!(
        "Using shim EFI executable from '{}'",
        boot_efi_path.display()
    );
    paths.push(boot_efi_path);

    // Add efitreeloader second.efi to the list when present.
    let efitreeloader_second = Path::new(temp_mount_dir)
        .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
        .join(mshv::ETL_SECOND_STAGE_EFI_FILENAME);
    if efitreeloader_second.exists() {
        if efitreeloader_second.is_file() {
            debug!(
                "Using efitreeloader second-stage EFI executable from '{}'",
                efitreeloader_second.display()
            );
            paths.push(efitreeloader_second);
        } else {
            bail!(
                "Found efitreeloader second-stage EFI path '{}' but it is not a file",
                efitreeloader_second.display()
            );
        }
    }

    Ok(paths)
}

/// Returns the path to the ESP directory where the boot files need to be copied to.
///
/// Path will be in the form of `/boot/efi/EFI/<ID>`, where `<ID>` is the install ID as determined
/// by ctx.
///
/// The function will find the next available install ID for this install and update the install
/// index in the engine context.
fn generate_efi_bin_base_dir_path(
    ctx: &EngineContext,
    mount_point: &Path,
) -> Result<PathBuf, Error> {
    // Compose the path to the ESP directory
    let esp_efi_path =
        path::join_relative(mount_point, ctx.esp_mount_path.as_path()).join(ESP_EFI_DIRECTORY);

    // Return the path to the ESP directory with the ESP dir name
    Ok(
        esp_efi_path.join(boot::get_update_esp_dir_name(ctx).context(
            "Failed to get ESP directory name for the new OS. Engine context is in an invalid state.",
        )?),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs::{File, OpenOptions},
        io::Write,
    };

    use nix::sys::statvfs;

    use trident_api::{
        config::{HostConfiguration, Os},
        constants::{DEFAULT_ESP_RELATIVE_MOUNT_POINT_PATH, GRUB2_RELATIVE_PATH},
        status::{AbVolumeSelection, ServicingType},
    };

    use crate::engine::{
        boot::{get_update_esp_dir_name, make_esp_dir_name_candidates, mshv::FILES},
        install_index,
    };

    #[test]
    fn test_generate_efi_bin_base_dir_path_clean_install() {
        // Clean install EngineContext
        let mut ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            esp_mount_path: PathBuf::from("/boot/efi"),
            ..Default::default()
        };

        let test_dir = TempDir::new().unwrap();
        let test_esp_dir = test_dir
            .path()
            .join(DEFAULT_ESP_RELATIVE_MOUNT_POINT_PATH)
            .join(ESP_EFI_DIRECTORY);

        // Check over several install ESP directory names. The idea is to ensure
        // that the function can return the expected ESP directory name. Then,
        // we create it, and then call the function again to make sure it will
        // return the next one. Do that a few times.
        for (idx, dir_names) in make_esp_dir_name_candidates().take(50) {
            println!(
                "Checking install index '{}' in folder {}",
                idx,
                test_dir.path().display()
            );
            ctx.install_index =
                install_index::next_install_index(test_dir.path(), ctx.esp_mount_path.as_path())
                    .unwrap();
            assert_eq!(idx, ctx.install_index);

            let esp_dir_path = generate_efi_bin_base_dir_path(&ctx, test_dir.path()).unwrap();
            println!("Returned ESP directory path: {esp_dir_path:?}");
            assert!(
                !esp_dir_path.exists(),
                "ESP directory returned should not exist yet"
            );
            assert_eq!(
                idx, ctx.install_index,
                "Expected install index does not match the one in EngineContext",
            );
            assert_eq!(
                esp_dir_path,
                test_esp_dir.join(&dir_names[0]),
                "ESP directory path does not match expected value"
            );

            // Create the directory so the next iteration finds it, jumps to
            // the next index, and creates the next one when it gets here
            // again.
            fs::create_dir_all(&esp_dir_path).unwrap();
        }
    }

    #[test]
    fn test_generate_efi_bin_base_dir_path_ab_update() {
        fn test_generate_efi_bin_base_dir_path(ctx: &mut EngineContext) {
            println!(
                "Checking AB update to {}",
                match ctx.ab_active_volume {
                    Some(AbVolumeSelection::VolumeA) => "A",
                    Some(AbVolumeSelection::VolumeB) => "B",
                    None => "unknown",
                }
            );

            // Record the expected install index
            let expected = ctx.install_index;
            // Expected ESP dir name
            let expected_dir_name =
                get_update_esp_dir_name(ctx).expect("Failed to get ESP dir name");

            // Set up temp dirs.
            let test_dir = TempDir::new().unwrap();
            let test_esp_dir = test_dir
                .path()
                .join(DEFAULT_ESP_RELATIVE_MOUNT_POINT_PATH)
                .join(ESP_EFI_DIRECTORY);

            // On a clean state, generate the ESP directory path.
            let esp_dir_path = generate_efi_bin_base_dir_path(ctx, test_dir.path()).unwrap();
            assert_eq!(
                esp_dir_path,
                test_esp_dir.join(&expected_dir_name),
                "ESP directory path does not match expected value"
            );
            assert_eq!(
                ctx.install_index, expected,
                "Install index in EngineContext does not match expected value"
            );

            // Create all directories for the expected index + 50 to ensure they are ignored
            // and we still get the same install index.
            make_esp_dir_name_candidates()
                .take(expected + 50)
                .for_each(|(_, dir_names)| {
                    fs::create_dir_all(test_esp_dir.join(&dir_names[0])).unwrap();
                });

            // Generate the ESP directory path again.
            let esp_dir_path = generate_efi_bin_base_dir_path(ctx, test_dir.path()).unwrap();
            assert_eq!(
                esp_dir_path,
                test_esp_dir.join(&expected_dir_name),
                "ESP directory path does not match expected value"
            );
            assert_eq!(
                ctx.install_index, expected,
                "Install index in EngineContext does not match expected value"
            );
        }

        // Test AB update to B
        println!("Checking AB update to B");
        test_generate_efi_bin_base_dir_path(&mut EngineContext {
            servicing_type: ServicingType::AbUpdate,
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            ..Default::default()
        });

        // Test AB update to A
        println!("Checking AB update to A");
        test_generate_efi_bin_base_dir_path(&mut EngineContext {
            servicing_type: ServicingType::AbUpdate,
            ab_active_volume: Some(AbVolumeSelection::VolumeB),
            ..Default::default()
        });

        // Test AB update with no active volume
        println!("Checking AB update with no active volume");
        test_generate_efi_bin_base_dir_path(&mut EngineContext {
            servicing_type: ServicingType::AbUpdate,
            // Set to None to trigger default behavior
            ab_active_volume: None,
            ..Default::default()
        });
    }

    /// Creates mock boot files in temp_mount_dir
    fn create_boot_files(temp_mount_dir: &Path, boot_files: &[PathBuf], content: &str) {
        for path in boot_files {
            let full_path = temp_mount_dir.join(path);

            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut file = File::create(&full_path).unwrap();
            writeln!(file, "Mock content for {}: {}", path.display(), content).unwrap();
        }
    }

    /// Compares two files byte by byte and returns true if they are identical
    fn files_are_identical(file1: &Path, file2: &Path) -> bool {
        let mut buf1 = Vec::new();
        let mut buf2 = Vec::new();
        File::open(file1).unwrap().read_to_end(&mut buf1).unwrap();
        File::open(file2).unwrap().read_to_end(&mut buf2).unwrap();
        buf1 == buf2
    }

    fn create_and_fill_esp(
        mount_point: &TempDir,
        esp_name: &str,
        file_names: &[PathBuf],
        content: &str,
    ) -> PathBuf {
        let fallback_esp_dir = mount_point
            .path()
            .join(DEFAULT_ESP_RELATIVE_MOUNT_POINT_PATH)
            .join(ESP_EFI_DIRECTORY)
            .join(esp_name);
        fs::create_dir_all(&fallback_esp_dir).unwrap();
        create_boot_files(&fallback_esp_dir, file_names, content);
        fallback_esp_dir
    }

    fn validate_fallback(
        ctx: &EngineContext,
        servicing_state: ServicingState,
        file_names: &[PathBuf],
        azl_boot_name: &str,
    ) {
        const ORIGINAL_FALLBACK_CONTENT: &str = "original-fallback";
        const NEW_BOOT_CONTENT: &str = "new-boot";

        let mount_point = TempDir::new().unwrap();
        let fallback_esp_dir = create_and_fill_esp(
            &mount_point,
            EFI_DEFAULT_BIN_DIRECTORY,
            file_names,
            ORIGINAL_FALLBACK_CONTENT,
        );
        create_and_fill_esp(&mount_point, azl_boot_name, file_names, NEW_BOOT_CONTENT);

        set_uefi_fallback_contents(ctx, servicing_state, mount_point.path()).unwrap();
        for file in file_names {
            let fallback_file = fallback_esp_dir.join(file);
            let mut fallback_file_contents = String::new();
            File::open(&fallback_file)
                .unwrap()
                .read_to_string(&mut fallback_file_contents)
                .unwrap();
            match ctx.spec.os.uefi_fallback {
                UefiFallbackMode::Disabled =>
                // Fallback should not have been updated
                {
                    assert!(
                        fallback_file_contents.contains(ORIGINAL_FALLBACK_CONTENT),
                        "{} was updated, but should not have been",
                        fallback_file.display()
                    )
                }
                UefiFallbackMode::Conservative | UefiFallbackMode::Optimistic =>
                // Fallback should have been updated
                {
                    assert!(
                        fallback_file_contents.contains(NEW_BOOT_CONTENT),
                        "{} was not updated, but should have been",
                        fallback_file.display()
                    )
                }
            };
        }
    }

    #[test]
    fn test_find_uefi_fallback_source_dir_name() {
        let mut ctx = EngineContext {
            servicing_type: ServicingType::AbUpdate,
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            install_index: 0,
            spec: HostConfiguration {
                os: Os {
                    uefi_fallback: UefiFallbackMode::Conservative,
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        let test_cases = vec![
            (
                ServicingState::CleanInstallStaged,
                UefiFallbackMode::Disabled,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::CleanInstall,
                None::<String>, // with Disabled mode, we do not copy anything
                "Validate CleanInstallStaged + Disabled + active volume A ==> None",
            ),
            (
                ServicingState::CleanInstallStaged,
                UefiFallbackMode::Conservative,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::CleanInstall,
                Some("AZLA".to_string()), // in finalize, with 'conservative', copy from volume A (volume we just put COSI files on)
                "Validate CleanInstallStaged + Conservative + active volume A ==> AZLA",
            ),
            (
                ServicingState::CleanInstallStaged,
                UefiFallbackMode::Optimistic,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::CleanInstall,
                Some("AZLA".to_string()), // in finalize, with 'optimistic', copy from volume A (volume we just put COSI files on)
                "Validate CleanInstallStaged + Optimistic + active volume A ==> AZLA",
            ),
            (
                ServicingState::AbUpdateStaged,
                UefiFallbackMode::Conservative,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::AbUpdate,
                Some("AZLA".to_string()), // in finalize, with 'conservative', copy from active volume (the COSI bits have not been validated yet)
                "Validate AbUpdateStaged + Conservative + active volume A ==> AZLA",
            ),
            (
                ServicingState::AbUpdateStaged,
                UefiFallbackMode::Optimistic,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::AbUpdate,
                Some("AZLB".to_string()), // in finalize, with 'optimistic', copy from inactive volume (that we just put COSI files on)
                "Validate AbUpdateStaged + Optimistic + active volume A ==> AZLB",
            ),
            (
                ServicingState::AbUpdateStaged,
                UefiFallbackMode::Disabled,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::AbUpdate,
                None::<String>, // with Disabled, we do not copy anything
                "Validate AbUpdateStaged + Disabled + active volume A ==> None",
            ),
            (
                ServicingState::AbUpdateFinalized,
                UefiFallbackMode::Conservative,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::AbUpdate,
                // In commit, with 'conservative', copy the validated update target;
                // the recorded active volume still refers to the pre-update OS.
                Some("AZLB".to_string()),
                "Validate AbUpdateFinalized + Conservative + active volume A ==> AZLB",
            ),
            (
                ServicingState::AbUpdateFinalized,
                UefiFallbackMode::Conservative,
                Some(AbVolumeSelection::VolumeB),
                ServicingType::AbUpdate,
                Some("AZLA".to_string()),
                "Validate AbUpdateFinalized + Conservative + active volume B ==> AZLA",
            ),
            (
                ServicingState::AbUpdateFinalized,
                UefiFallbackMode::Optimistic,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::AbUpdate,
                None, // in commit, with 'optimistic', no copy is needed because it was done in finalize
                "Validate AbUpdateFinalized + Optimistic + active volume A ==> None",
            ),
            (
                ServicingState::AbUpdateFinalized,
                UefiFallbackMode::Disabled,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::AbUpdate,
                None::<String>, // with Disabled, we do not copy anything
                "Validate AbUpdateFinalized + Disabled + active volume A ==> None",
            ),
            (
                ServicingState::ManualRollbackAbStaged,
                UefiFallbackMode::Conservative,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::ManualRollbackAb,
                Some("AZLB".to_string()), // in ManualRollbackAbStaged, with 'conservative', copy from inactive volume
                "Validate ManualRollbackAbStaged + Conservative + active volume A ==> AZLB",
            ),
            (
                ServicingState::ManualRollbackAbStaged,
                UefiFallbackMode::Optimistic,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::ManualRollbackAb,
                Some("AZLB".to_string()), // in ManualRollbackAbStaged, with 'optimistic', copy from inactive volume
                "Validate ManualRollbackAbStaged + Optimistic + active volume A ==> AZLB",
            ),
            (
                ServicingState::ManualRollbackAbStaged,
                UefiFallbackMode::Disabled,
                Some(AbVolumeSelection::VolumeA),
                ServicingType::ManualRollbackAb,
                None::<String>, // with Disabled, we do not copy anything
                "Validate ManualRollbackAbStaged + Disabled + active volume A ==> None",
            ),
        ];
        for test_case in test_cases {
            ctx.spec.os.uefi_fallback = test_case.1;
            ctx.ab_active_volume = test_case.2;
            ctx.servicing_type = test_case.3;
            assert_eq!(
                find_uefi_fallback_source_dir_name(&ctx, test_case.0),
                test_case.4,
                "{}",
                test_case.5
            );
        }

        //
        // No-op for all unsupported stages
        //
        for stage in [ServicingState::NotProvisioned, ServicingState::Provisioned] {
            // Validate <unsupported stages> ==> None
            for mode in [
                UefiFallbackMode::Disabled,
                UefiFallbackMode::Optimistic,
                UefiFallbackMode::Conservative,
            ] {
                // Validate <all uefi fallback modes>
                for ab_volume in [AbVolumeSelection::VolumeA, AbVolumeSelection::VolumeB] {
                    // Validate <both volumes>
                    for servicing_type in [ServicingType::AbUpdate, ServicingType::CleanInstall] {
                        // all should be None
                        ctx.spec.os.uefi_fallback = mode;
                        ctx.ab_active_volume = Some(ab_volume);
                        ctx.servicing_type = servicing_type;
                        assert!(
                            find_uefi_fallback_source_dir_name(&ctx, stage).is_none(),
                            "Unsupported combo should always result in None: {stage:?} + {mode:?} + {ab_volume:?} + {servicing_type:?}"
                        );
                    }
                }
            }
        }

        //
        // No-op combinations: these are combinations that copying would be redundant
        //
        // Validate CleanInstallFinalized + Conservative + active volume B ==> None
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Conservative;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        ctx.servicing_type = ServicingType::CleanInstall;
        assert!(
            find_uefi_fallback_source_dir_name(&ctx, ServicingState::CleanInstallFinalized)
                .is_none()
        );
        // Validate AbUpdateFinalized + Optimistic + active volume B ==> None
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Optimistic;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        ctx.servicing_type = ServicingType::AbUpdate;
        assert!(
            find_uefi_fallback_source_dir_name(&ctx, ServicingState::AbUpdateFinalized).is_none()
        );
    }

    /// Conservative commit copies the validated target, not the recorded active slot.
    #[test]
    fn test_conservative_commit_copies_validated_slot() {
        let file_names = vec![PathBuf::from(BOOT_EFI), PathBuf::from(GRUB_EFI)];
        for (active_volume, active_name, target_name) in [
            (AbVolumeSelection::VolumeA, "AZLA", "AZLB"),
            (AbVolumeSelection::VolumeB, "AZLB", "AZLA"),
        ] {
            let mut ctx = EngineContext {
                servicing_type: ServicingType::AbUpdate,
                ab_active_volume: Some(active_volume),
                ..Default::default()
            };
            ctx.spec.os.uefi_fallback = UefiFallbackMode::Conservative;
            let mount_point = TempDir::new().unwrap();
            let retained = create_and_fill_esp(&mount_point, active_name, &file_names, "retained");
            let candidate =
                create_and_fill_esp(&mount_point, target_name, &file_names, "candidate");
            let fallback = create_and_fill_esp(
                &mount_point,
                EFI_DEFAULT_BIN_DIRECTORY,
                &file_names,
                "initial-fallback",
            );

            set_uefi_fallback_contents(&ctx, ServicingState::AbUpdateStaged, mount_point.path())
                .unwrap();
            for file in &file_names {
                assert!(files_are_identical(
                    &fallback.join(file),
                    &retained.join(file)
                ));
            }

            set_uefi_fallback_contents(&ctx, ServicingState::AbUpdateFinalized, mount_point.path())
                .unwrap();
            for file in &file_names {
                assert!(
                    files_are_identical(&fallback.join(file), &candidate.join(file)),
                    "Commit with recorded active slot {active_volume:?} must copy {target_name}"
                );
            }
        }
    }

    #[test]
    fn test_set_uefi_fallback_contents() {
        // Create a list of boot files
        let file_names = vec![
            PathBuf::from(GRUB2_CONFIG_FILENAME),
            PathBuf::from(GRUB_EFI),
            PathBuf::from(BOOT_EFI),
        ];

        let mut ctx = EngineContext {
            servicing_type: ServicingType::AbUpdate,
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            install_index: 0,
            spec: HostConfiguration {
                os: Os {
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Validate ABUpdate with default Conservative mode and active volume A ==> copy /EFI/AZLA to /EFI/BOOT
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, ServicingState::AbUpdateStaged, &file_names, "AZLA");

        // Validate ABUpdate Conservative with active volume A ==> copy /EFI/AZLA to /EFI/BOOT
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Conservative;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, ServicingState::AbUpdateStaged, &file_names, "AZLA");

        // Validate ABUpdate Conservative with active volume B ==> copy /EFI/AZLB to /EFI/BOOT
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Conservative;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, ServicingState::AbUpdateStaged, &file_names, "AZLB");

        // Validate ABUpdate Optimistic with active volume A ==> copy /EFI/AZLB to /EFI/BOOT
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Optimistic;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, ServicingState::AbUpdateStaged, &file_names, "AZLB");

        // Validate ABUpdate Optimistic with active volume B ==> copy /EFI/AZLA to /EFI/BOOT
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Optimistic;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, ServicingState::AbUpdateStaged, &file_names, "AZLA");

        // Validate CleanInstall Conservative with active volume None ==> copy /EFI/AZLA to /EFI/BOOT
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Conservative;
        ctx.ab_active_volume = None;
        ctx.servicing_type = ServicingType::CleanInstall;
        validate_fallback(
            &ctx,
            ServicingState::CleanInstallStaged,
            &file_names,
            "AZLA",
        );

        // Validate CleanInstall Optimistic with active volume None ==> copy /EFI/AZLA to /EFI/BOOT
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Optimistic;
        ctx.ab_active_volume = None;
        ctx.servicing_type = ServicingType::CleanInstall;
        validate_fallback(
            &ctx,
            ServicingState::CleanInstallStaged,
            &file_names,
            "AZLA",
        );

        // Validate ABUpdate Disabled with active volume A ==> /EFI/BOOT unchanged
        ctx.spec.os.uefi_fallback = UefiFallbackMode::Disabled;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, ServicingState::AbUpdateStaged, &file_names, "AZLA");
    }

    #[test]
    fn test_replace_boot_files() {
        let from_dir = TempDir::new().unwrap();
        let to_dir = TempDir::new().unwrap();

        let file_infos = vec![
            ("file1.txt", "New content of file 1"),
            ("file2.txt", "New content of file 2"),
        ];

        let existing_file_infos = vec![
            ("file1.txt", "Content of file 1"),
            ("file2.txt", "Content of file 2"),
            ("file3.txt", "Content of file 3"),
        ];

        // Create files in from_dir
        for (file_name, content) in &file_infos {
            let file_path = from_dir.path().join(file_name);
            let mut file = File::create(&file_path).unwrap();
            writeln!(file, "{content}").unwrap();
        }

        // Create existing files in esp_dir
        for (file_name, content) in &existing_file_infos {
            let file_path = to_dir.path().join(file_name);
            let mut file = File::create(&file_path).unwrap();
            writeln!(file, "{content}").unwrap();
        }

        // Call the function to copy files
        replace_boot_files(from_dir.path(), to_dir.path()).unwrap();

        // Verify that files have been copied and renamed correctly
        for (file_name, _content) in &file_infos {
            assert!(
                files_are_identical(
                    &from_dir.path().join(file_name),
                    &to_dir.path().join(file_name),
                ),
                "Files are not identical: {} and {}",
                from_dir.path().join(file_name).display(),
                to_dir.path().join(file_name).display()
            );
        }

        // Verify that existing files that were not in from_dir are removed
        for (file_name, _) in &existing_file_infos {
            if !file_infos.iter().any(|(f, _)| f == file_name) {
                assert!(!to_dir.path().join(file_name).exists());
            }
        }
    }

    #[test]
    fn test_replace_boot_files_retires_mshv_directory() {
        for directory in ["Windows", "Windows.old", "wInDoWs.old"] {
            let source = TempDir::new().unwrap();
            let destination = TempDir::new().unwrap();
            fs::write(source.path().join(BOOT_EFI), b"ordinary loader").unwrap();
            for name in FILES {
                fs::write(destination.path().join(name), b"previous loader").unwrap();
            }
            let secondary = destination.path().join(directory).join("System32");
            fs::create_dir_all(&secondary).unwrap();
            fs::write(secondary.join("image-specific.dll"), b"previous payload").unwrap();

            replace_boot_files(source.path(), destination.path()).unwrap();

            assert_eq!(
                fs::read(destination.path().join(BOOT_EFI)).unwrap(),
                b"ordinary loader"
            );
            assert_eq!(fs::read_dir(destination.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn test_replace_boot_files_preserves_unrelated_directories() {
        let source = TempDir::new().unwrap();
        let destination = TempDir::new().unwrap();
        fs::write(source.path().join(BOOT_EFI), b"ordinary loader").unwrap();
        fs::create_dir_all(destination.path().join("unrelated")).unwrap();
        fs::write(destination.path().join("unrelated/keep"), b"unrelated data").unwrap();

        assert!(replace_boot_files(source.path(), destination.path()).is_err());
        assert_eq!(
            fs::read(destination.path().join("unrelated.old/keep")).unwrap(),
            b"unrelated data"
        );
    }

    /// Ordinary bootstrap copies top-level boot files.
    #[test]
    fn test_bootstrap_azla_from_fallback() {
        let esp_dir = TempDir::new().unwrap();
        let boot_dir = esp_dir.path().join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        let azla_dir = esp_dir.path().join(ESP_EFI_DIRECTORY).join("AZLA");

        let file_infos = vec![
            (BOOT_EFI, "Fallback bootloader"),
            (GRUB_EFI, "Fallback grub"),
        ];

        fs::create_dir_all(&boot_dir).unwrap();
        for (file_name, content) in &file_infos {
            let file_path = boot_dir.join(file_name);
            let mut file = File::create(&file_path).unwrap();
            writeln!(file, "{content}").unwrap();
        }

        bootstrap_azla_from_fallback(esp_dir.path()).unwrap();

        for (file_name, _content) in &file_infos {
            assert!(
                files_are_identical(&boot_dir.join(file_name), &azla_dir.join(file_name)),
                "Files are not identical: {} and {}",
                boot_dir.join(file_name).display(),
                azla_dir.join(file_name).display()
            );
        }
    }

    /// Ordinary bootstrap preserves even an incomplete existing AZLA.
    #[test]
    fn test_bootstrap_azla_from_fallback_is_idempotent_when_azla_already_exists() {
        let esp_dir = TempDir::new().unwrap();
        let boot_dir = esp_dir.path().join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        let azla_dir = esp_dir.path().join(ESP_EFI_DIRECTORY).join("AZLA");

        let fallback_file_infos = vec![
            (BOOT_EFI, "Fallback bootloader"),
            (GRUB_EFI, "Fallback grub"),
        ];
        fs::create_dir_all(&boot_dir).unwrap();
        for (file_name, content) in &fallback_file_infos {
            let file_path = boot_dir.join(file_name);
            let mut file = File::create(&file_path).unwrap();
            writeln!(file, "{content}").unwrap();
        }

        // Pre-populate AZLA with content that does not match the fallback
        // location, so any attempt to re-copy would be detectable.
        fs::create_dir_all(&azla_dir).unwrap();
        let sentinel_path = azla_dir.join(BOOT_EFI);
        let mut sentinel_file = File::create(&sentinel_path).unwrap();
        writeln!(sentinel_file, "Pre-existing AZLA content").unwrap();

        bootstrap_azla_from_fallback(esp_dir.path()).unwrap();

        // AZLA must be left untouched: ordinary-mode bootstrap only runs when
        // AZLA is absent, so a second call neither validates nor refreshes it.
        let contents = fs::read_to_string(&sentinel_path).unwrap();
        assert_eq!(
            contents.trim(),
            "Pre-existing AZLA content",
            "bootstrap_azla_from_fallback must not modify an already-existing AZLA directory"
        );
        assert!(
            !azla_dir.join(GRUB_EFI).exists(),
            "bootstrap_azla_from_fallback must not add files to an already-existing AZLA directory"
        );
    }

    /// Retrying ordinary bootstrap discards stale staging contents.
    #[test]
    fn test_bootstrap_azla_from_fallback_cleans_up_stale_tmp_dir() {
        let esp_dir = TempDir::new().unwrap();
        let boot_dir = esp_dir.path().join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        let azla_dir = esp_dir.path().join(ESP_EFI_DIRECTORY).join("AZLA");
        let azla_tmp_dir = esp_dir
            .path()
            .join(ESP_EFI_DIRECTORY)
            .join("AZLA.bootstrap-tmp");

        let file_infos = vec![
            (BOOT_EFI, "Fallback bootloader"),
            (GRUB_EFI, "Fallback grub"),
        ];
        fs::create_dir_all(&boot_dir).unwrap();
        for (file_name, content) in &file_infos {
            let file_path = boot_dir.join(file_name);
            let mut file = File::create(&file_path).unwrap();
            writeln!(file, "{content}").unwrap();
        }

        // Simulate a prior crashed bootstrap attempt: staging succeeded but
        // the rename into place never happened, leaving a stale temp dir
        // with unrelated/incomplete content.
        fs::create_dir_all(&azla_tmp_dir).unwrap();
        let mut stale_file = File::create(azla_tmp_dir.join("stale-leftover")).unwrap();
        writeln!(stale_file, "Leftover from a crashed bootstrap").unwrap();

        bootstrap_azla_from_fallback(esp_dir.path()).unwrap();

        // The stale temp dir must be cleaned up, and bootstrapping must
        // still succeed despite it being present beforehand.
        assert!(
            !azla_tmp_dir.exists(),
            "stale temporary bootstrap directory must be removed"
        );
        for (file_name, _content) in &file_infos {
            assert!(
                files_are_identical(&boot_dir.join(file_name), &azla_dir.join(file_name)),
                "Files are not identical: {} and {}",
                boot_dir.join(file_name).display(),
                azla_dir.join(file_name).display()
            );
        }
    }

    /// MSHV discovers secondary files and validates, but never refreshes, existing AZLA.
    #[test]
    fn test_bootstrap_azla_from_mshv_fallback() {
        let esp = TempDir::new().unwrap();
        let boot = esp.path().join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        let azla = esp.path().join(ESP_EFI_DIRECTORY).join("AZLA");
        let temporary = esp
            .path()
            .join(ESP_EFI_DIRECTORY)
            .join("AZLA.bootstrap-tmp");
        for name in FILES {
            let path = boot.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, name).unwrap();
        }
        let secondary = "Windows/System32/image-specific.dll";
        fs::create_dir_all(boot.join("Windows/System32")).unwrap();
        fs::write(boot.join(secondary), b"secondary").unwrap();
        fs::create_dir_all(&temporary).unwrap();
        fs::write(temporary.join("stale"), "incomplete").unwrap();

        bootstrap_azla_from_fallback(esp.path()).unwrap();
        for name in FILES {
            assert!(files_are_identical(&boot.join(name), &azla.join(name)));
        }
        assert!(files_are_identical(
            &boot.join(secondary),
            &azla.join(secondary)
        ));
        assert!(!temporary.exists());

        fs::write(boot.join("HvLoader.efi"), "new fallback").unwrap();
        bootstrap_azla_from_fallback(esp.path()).unwrap();
        assert_eq!(
            fs::read_to_string(azla.join("HvLoader.efi")).unwrap(),
            "HvLoader.efi"
        );

        fs::remove_file(azla.join(secondary)).unwrap();
        bootstrap_azla_from_fallback(esp.path()).unwrap();
        assert!(!azla.join(secondary).exists());
        fs::remove_file(azla.join("lxhvloader.dll")).unwrap();
        assert!(bootstrap_azla_from_fallback(esp.path()).is_err());
        assert!(!azla.join("lxhvloader.dll").exists());
    }

    /// Failed preflight follows stale-copy cleanup and leaves BOOT untouched.
    #[test]
    fn test_bootstrap_azla_cleans_stale_mshv_copy_before_capacity_check() {
        let esp = TempDir::new().unwrap();
        let boot = esp.path().join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        let azla = esp.path().join(ESP_EFI_DIRECTORY).join("AZLA");
        let temporary = esp
            .path()
            .join(ESP_EFI_DIRECTORY)
            .join("AZLA.bootstrap-tmp");
        for name in FILES {
            let path = boot.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"retained").unwrap();
        }
        fs::create_dir_all(&temporary).unwrap();
        fs::write(temporary.join("stale"), b"abandoned copy").unwrap();

        // Force preflight failure with a sparse file, without filling the filesystem.
        let stats = statvfs::statvfs(esp.path()).unwrap();
        let length = stats.blocks_available() * stats.fragment_size() + 1;
        OpenOptions::new()
            .write(true)
            .open(boot.join(BOOT_EFI))
            .unwrap()
            .set_len(length)
            .unwrap();

        let error = bootstrap_azla_from_fallback(esp.path()).unwrap_err();
        assert!(error.to_string().contains("Insufficient ESP capacity"));
        assert!(!temporary.exists());
        assert!(!azla.exists());
        assert_eq!(fs::metadata(boot.join(BOOT_EFI)).unwrap().len(), length);
        assert_eq!(fs::read(boot.join("HvLoader.efi")).unwrap(), b"retained");
    }

    /// Incomplete marked fallback cannot publish AZLA.
    #[test]
    fn test_bootstrap_azla_rejects_incomplete_mshv_fallback() {
        let esp = TempDir::new().unwrap();
        let boot = esp.path().join(EFI_DEFAULT_BIN_RELATIVE_PATH);
        let azla = esp.path().join(ESP_EFI_DIRECTORY).join("AZLA");
        fs::create_dir_all(&boot).unwrap();
        fs::write(boot.join(BOOT_EFI), "shim").unwrap();
        fs::write(boot.join("HvLoader.efi"), "loader").unwrap();
        assert!(bootstrap_azla_from_fallback(esp.path()).is_err());
        assert!(!azla.exists());
        assert_eq!(fs::read_to_string(boot.join(BOOT_EFI)).unwrap(), "shim");
    }

    /// Validates that copy_boot_files() correctly copies boot files from temp_mount_dir to esp_dir
    #[test]
    fn test_copy_boot_files_without_noprefix() {
        let temp_mount_dir = TempDir::new().unwrap();
        let esp_dir = TempDir::new().unwrap();

        // Create a list of boot files
        let file_names = vec![
            PathBuf::from(GRUB2_CONFIG_FILENAME),
            PathBuf::from(GRUB_EFI),
            PathBuf::from(BOOT_EFI),
            PathBuf::from(mshv::ETL_SECOND_STAGE_EFI_FILENAME),
        ];

        // Call helper func to create mock boot files in temp_mount_dir
        create_boot_files(temp_mount_dir.path(), &file_names, "test-content");
        // Call helper func to copy boot files from temp_mount_dir to esp_dir
        let noprefix =
            copy_boot_files(temp_mount_dir.path(), esp_dir.path(), file_names.clone()).unwrap();
        assert!(
            !noprefix,
            "grub-noprefix.efi is not in the list of files, so it should not be detected"
        );

        for file_name in file_names {
            // Create full path of source_path
            let source_path = temp_mount_dir.path().join(file_name.clone());
            // Create full path of destination_path
            let destination_path = esp_dir.path().join(file_name);

            assert!(
                files_are_identical(&source_path, &destination_path),
                "Files are not identical: {} and {}",
                source_path.display(),
                destination_path.display()
            );
        }
    }

    /// Validates that copy_boot_files() correctly copies boot files with
    /// grub-noprefix from temp_mount_dir to esp_dir
    #[test]
    fn test_copy_boot_files_grub_noprefix() {
        let temp_mount_dir = TempDir::new().unwrap();
        let esp_dir = TempDir::new().unwrap();

        // Create a list of boot files
        let file_names = vec![
            PathBuf::from(GRUB2_CONFIG_FILENAME),
            PathBuf::from(GRUB_NOPREFIX_EFI),
            PathBuf::from(BOOT_EFI),
            PathBuf::from(mshv::ETL_SECOND_STAGE_EFI_FILENAME),
        ];

        // Call helper func to create mock boot files in temp_mount_dir
        create_boot_files(temp_mount_dir.path(), &file_names, "test-content");
        // Call helper func to copy boot files from temp_mount_dir to esp_dir
        let noprefix =
            copy_boot_files(temp_mount_dir.path(), esp_dir.path(), file_names.clone()).unwrap();

        assert!(
            noprefix,
            "grub-noprefix.efi is in the list of files, so it should be detected"
        );

        for file_name in file_names.clone() {
            // Create full path of source_path
            let source_path = temp_mount_dir.path().join(file_name.clone());
            // Create full path of destination_path
            let mut destination_path = esp_dir.path().join(file_name.clone());

            if file_name == Path::new(GRUB_NOPREFIX_EFI) {
                destination_path = esp_dir.path().join(GRUB_EFI);
            }

            assert!(
                files_are_identical(&source_path, &destination_path),
                "Files are not identical: {} and {}",
                source_path.display(),
                destination_path.display()
            );
        }
    }

    /// Validates that generate_boot_filepaths() returns the correct filepaths based on target
    #[test]
    fn test_generate_boot_filepaths() {
        // Test case 1: Run generate_boot_filepaths() with GRUB under EFI_DEFAULT_BIN_RELATIVE_PATH
        // Create a temp dir
        let temp_mount_dir = TempDir::new().unwrap();

        // Create a GRUB config inside of the temp dir
        let efi_boot_grub_path = temp_mount_dir
            .path()
            .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
            .join(GRUB2_CONFIG_FILENAME);
        fs::create_dir_all(efi_boot_grub_path.parent().unwrap()).unwrap();
        File::create(&efi_boot_grub_path).unwrap();

        // Create a grub EFI executable inside of the temp dir
        let grub_efi_path = temp_mount_dir
            .path()
            .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
            .join(GRUB_EFI);
        File::create(&grub_efi_path).unwrap();

        // Create a boot EFI executable inside of the temp dir
        let boot_efi_path = temp_mount_dir
            .path()
            .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
            .join(BOOT_EFI);
        File::create(&boot_efi_path).unwrap();

        let generated_paths_efi_boot =
            generate_boot_filepaths(temp_mount_dir.path(), false).unwrap();
        // Define your expected paths here when file exists
        let expected_paths_efi_boot = vec![
            efi_boot_grub_path.clone(),
            grub_efi_path.clone(),
            boot_efi_path.clone(),
        ];
        assert_eq!(
            generated_paths_efi_boot, expected_paths_efi_boot,
            "Generated file paths do not match expected paths when file exists"
        );

        // Test case 2: Run generate_boot_filepaths() without GRUB
        // Remove the GRUB config from the temp dir and create a new one, under GRUB2_RELATIVE_PATH
        fs::remove_file(&efi_boot_grub_path).unwrap();
        assert_eq!(
            generate_boot_filepaths(temp_mount_dir.path(), false)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find grub.cfg",
            "generate_boot_filepaths() should fail if grub.cfg does not exist"
        );
        generate_boot_filepaths(temp_mount_dir.path(), true).unwrap();

        // Test case 3: Run generate_boot_filepaths() with GRUB under GRUB2_RELATIVE_PATH
        let boot_grub2_grub_path = temp_mount_dir
            .path()
            .join(GRUB2_RELATIVE_PATH)
            .join(GRUB2_CONFIG_FILENAME);
        fs::create_dir_all(boot_grub2_grub_path.parent().unwrap()).unwrap();
        File::create(&boot_grub2_grub_path).unwrap();

        let generated_paths_boot_grub2 =
            generate_boot_filepaths(temp_mount_dir.path(), false).unwrap();
        // Define expected paths here when EFI/BOOT/grub.cfg does not exist and boot/grub2/grub.cfg
        // is used instead
        let expected_paths_boot_grub2 = vec![
            boot_grub2_grub_path.clone(),
            grub_efi_path.clone(),
            boot_efi_path.clone(),
        ];
        assert_eq!(
            generated_paths_boot_grub2, expected_paths_boot_grub2,
            "Generated file paths do not match expected paths when file does not exist"
        );

        // Test case 4: Run generate_boot_filepaths() without grub EFI executable
        // Remove old grub EFI executable
        fs::remove_file(&grub_efi_path).unwrap();
        assert_eq!(
            generate_boot_filepaths(temp_mount_dir.path(), false)
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find GRUB EFI executable"
        );

        // Test case 5: Run generate_boot_filepaths() with a grub EFI executable with noprefix name
        // Create a grub EFI executable with the noprefix name inside of the temp dir
        let grub_efi_noprefix_path = temp_mount_dir
            .path()
            .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
            .join(GRUB_NOPREFIX_EFI);
        File::create(&grub_efi_noprefix_path).unwrap();

        let generated_paths_noprefix =
            generate_boot_filepaths(temp_mount_dir.path(), false).unwrap();
        // Define expected paths here when EFI/BOOT/grub.cfg does not exist and boot/grub2/grub.cfg
        // is used instead
        let expected_paths_noprefix = vec![
            boot_grub2_grub_path,
            grub_efi_noprefix_path,
            boot_efi_path.clone(),
        ];
        assert_eq!(
            generated_paths_noprefix, expected_paths_noprefix,
            "Generated file paths do not match expected paths when file does not exist"
        );

        // Test case 6: Run generate_boot_filepaths() without boot EFI executable
        // Remove old boot EFI executable
        fs::remove_file(&boot_efi_path).unwrap();
        assert_eq!(
            generate_boot_filepaths(temp_mount_dir.path(), false)
                .unwrap_err()
                .root_cause()
                .to_string(),
            format!(
                "Failed to find shim EFI executable at path '{}'",
                boot_efi_path.display()
            )
        );
    }

    /// Verifies optional ETL second-stage discovery and copying for UKI and
    /// non-UKI images, with either standard or noprefix GRUB.
    #[test]
    fn test_generate_boot_filepaths_second_stage() {
        for is_uki in [false, true] {
            for grub_filename in [GRUB_EFI, GRUB_NOPREFIX_EFI] {
                let source = TempDir::new().unwrap();
                let boot_dir = Path::new(EFI_DEFAULT_BIN_RELATIVE_PATH);
                let mut relative_paths = Vec::new();
                if !is_uki {
                    relative_paths.push(boot_dir.join(GRUB2_CONFIG_FILENAME));
                }
                relative_paths.push(boot_dir.join(grub_filename));
                relative_paths.push(boot_dir.join(BOOT_EFI));
                create_boot_files(source.path(), &relative_paths, "test-content");
                let mut expected_paths: Vec<PathBuf> = relative_paths
                    .iter()
                    .map(|path| source.path().join(path))
                    .collect();
                let second_stage = source
                    .path()
                    .join(boot_dir)
                    .join(mshv::ETL_SECOND_STAGE_EFI_FILENAME);

                // Images without ETL must keep their existing boot-file list.
                assert_eq!(
                    generate_boot_filepaths(source.path(), is_uki).unwrap(),
                    expected_paths,
                    "missing optional second stage: is_uki={is_uki}, grub={grub_filename}"
                );

                // ETL's second stage must be appended once, regardless of boot mode.
                fs::write(&second_stage, [0, 0xff, 1, 0x80]).unwrap();
                expected_paths.push(second_stage.clone());
                let generated_paths = generate_boot_filepaths(source.path(), is_uki).unwrap();
                assert_eq!(
                    generated_paths, expected_paths,
                    "second stage must be appended exactly once: is_uki={is_uki}, grub={grub_filename}"
                );

                // GRUB's noprefix rename must not affect the second-stage filename
                // or binary contents, and copying must leave the source intact.
                let destination = TempDir::new().unwrap();
                let noprefix =
                    copy_boot_files(source.path(), destination.path(), generated_paths).unwrap();
                assert_eq!(noprefix, grub_filename == GRUB_NOPREFIX_EFI);
                assert_eq!(
                    fs::read(destination.path().join(mshv::ETL_SECOND_STAGE_EFI_FILENAME)).unwrap(),
                    [0, 0xff, 1, 0x80]
                );
                assert_eq!(fs::read(&second_stage).unwrap(), [0, 0xff, 1, 0x80]);

                // An invalid second-stage path must fail rather than be silently
                // omitted from the generated list.
                fs::remove_file(&second_stage).unwrap();
                fs::create_dir(&second_stage).unwrap();
                let error = generate_boot_filepaths(source.path(), is_uki).unwrap_err();
                assert_eq!(
                    error.to_string(),
                    format!(
                        "Found efitreeloader second-stage EFI path '{}' but it is not a file",
                        second_stage.display()
                    ),
                    "second-stage directory must be rejected: is_uki={is_uki}, grub={grub_filename}"
                );
            }
        }
    }

    /// Tests that [`ensure_esp_extraction_dir`] creates the staging directory
    /// when the OS image does not ship one.
    ///
    /// Immutable images such as Azure Container Linux ship a root filesystem
    /// with no `/var` at all, leaving systemd-tmpfiles to populate it on first
    /// boot. Staging the ESP image used to fail against such an image, after
    /// the disk had already been repartitioned.
    #[test]
    fn test_ensure_esp_extraction_dir_creates_missing_dir() {
        let newroot = TempDir::new().unwrap();

        // A root filesystem with no /var whatsoever, as ACL ships.
        assert!(!newroot.path().join("var").exists());

        let dir = ensure_esp_extraction_dir(newroot.path()).unwrap();

        assert!(dir.is_dir(), "staging directory should have been created");
        assert_eq!(dir, newroot.path().join("var/tmp"));
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o7777,
            ESP_EXTRACTION_DIRECTORY_MODE,
            "staging directory should be created as sticky and world-writable, \
            like a conventional /var/tmp"
        );

        // A temporary file can actually be created there, which is what the
        // deployment goes on to do.
        NamedTempFile::new_in(&dir).unwrap();
    }

    /// Tests that [`ensure_esp_extraction_dir`] leaves an existing staging
    /// directory alone, rather than changing the permissions an image chose.
    #[test]
    fn test_ensure_esp_extraction_dir_preserves_existing_dir() {
        let newroot = TempDir::new().unwrap();
        let existing = newroot.path().join("var/tmp");
        fs::create_dir_all(&existing).unwrap();
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).unwrap();

        let dir = ensure_esp_extraction_dir(newroot.path()).unwrap();

        assert_eq!(dir, existing);
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o7777,
            0o755,
            "an existing staging directory should be left untouched"
        );
    }
}
