use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, trace};
use reqwest::Url;
use tempfile::{NamedTempFile, TempDir};

use osutils::{
    bootloaders::{BOOT_EFI, GRUB_EFI, GRUB_NOPREFIX_EFI},
    dependencies::Dependency,
    filesystems::MountFileSystemType,
    mount::{self, MountGuard},
    path,
};
use trident_api::{
    config::UefiFallbackMode,
    constants::{
        internal_params::DISABLE_GRUB_NOPREFIX_CHECK, EFI_DEFAULT_BIN_DIRECTORY,
        EFI_DEFAULT_BIN_RELATIVE_PATH, ESP_EFI_DIRECTORY, ESP_RELATIVE_MOUNT_POINT_PATH,
        GRUB2_CONFIG_FILENAME, GRUB2_CONFIG_RELATIVE_PATH,
    },
    error::{ReportError, ServicingError, TridentError, TridentResultExt},
    status::AbVolumeSelection,
};

use crate::{
    engine::{
        boot::{self, uki, ESP_EXTRACTION_DIRECTORY},
        EngineContext, Subsystem,
    },
    io_utils::{
        hashing_reader::{HashingReader, HashingReader384},
        image_streamer,
    },
};

#[derive(Default, Debug)]
pub struct EspSubsystem;
impl Subsystem for EspSubsystem {
    fn name(&self) -> &'static str {
        "esp"
    }

    #[tracing::instrument(name = "esp_provision", skip_all)]
    fn provision(&mut self, ctx: &EngineContext, mount_path: &Path) -> Result<(), TridentError> {
        // Perform file-based deployment of ESP images, if needed, after filesystems have been
        // mounted and initialized.

        // Deploy ESP image
        deploy_esp(ctx, mount_path).structured(ServicingError::DeployESPImages)?;

        Ok(())
    }
}

/// Performs file-based deployment of ESP images from the OS image.
fn deploy_esp(ctx: &EngineContext, mount_point: &Path) -> Result<(), Error> {
    trace!("Deploying ESP from OS image");

    let os_image = ctx
        .image
        .as_ref()
        .context("OS image is required to deploy ESP from OS image")?;

    let esp_img = os_image
        .esp_filesystem()
        .context("Failed to get ESP image from OS image")?;

    let stream = esp_img
        .image_file
        .reader()
        .context("Failed to get reader for ESP image from OS image")?;

    // Extract the ESP image to a temporary file in
    // `<newroot>/ESP_EXTRACTION_DIRECTORY`. This location is generally
    // guaranteed to be writable and backed by a real block device, so we don't
    // have to store a potentially large ESP image in memory.
    let esp_extraction_dir = path::join_relative(mount_point, ESP_EXTRACTION_DIRECTORY);

    let (temp_file, computed_sha384) = load_raw_image(
        &esp_extraction_dir,
        os_image.source(),
        HashingReader384::new(stream),
    )
    .context("Failed to load raw image")?;

    if esp_img.image_file.sha384 != computed_sha384 {
        bail!(
            "SHA384 mismatch for disk image {}: expected {}, got {}",
            os_image.source(),
            esp_img.image_file.sha384,
            computed_sha384
        );
    }

    copy_file_artifacts(temp_file.path(), ctx, mount_point)
}

/// Takes in a reader to the raw zstd-compressed ESP image and decompresses it
/// into a temporary file under `/<mount_point>/<ESP_EXTRACTION_DIRECTORY>`.
/// Returns a tuple containing the temporary file and the computed hash (SHA256
/// or SHA384) of the image.
///
/// It also takes in the URL of the image to be shown in case of errors.
fn load_raw_image<R>(
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
    let computed_hash = image_streamer::stream_zstd_and_hash(reader, &temp_image_path)
        .context(format!("Failed to stream ESP image from {source}"))?;

    Ok((temp_image, computed_hash))
}

/// Performs file-based update of stand-alone ESP volume by copying boot files.
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

    // Generate list of filepaths to the boot files. Pass in the temp dir path where the image is
    // mounted to as an argument
    let boot_files =
        generate_boot_filepaths(temp_mount_dir).context("Failed to generate boot filepaths")?;

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
    let grub_noprefix = copy_boot_files(temp_mount_dir, &esp_dir_path, boot_files.clone())
        .context(format!(
            "Failed to copy boot files from directory {} to directory {}",
            temp_mount_dir.display(),
            esp_dir_path.display()
        ))?;

    if ctx.is_uki().unstructured("UKI setting unknown")? {
        // Prepare ESP directory structure for UKI boot
        uki::prepare_esp_for_uki(mount_point)?;

        // Copy the UKI from the image into the ESP directory
        uki::stage_uki_on_esp(temp_mount_dir, mount_point)?;
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

    // Handle UEFI fallback if configured
    configure_uefi_fallback(ctx, mount_point).context("Failed to configure UEFI fallback")?;

    Ok(())
}

fn configure_uefi_fallback(ctx: &EngineContext, mount_point: &Path) -> Result<(), Error> {
    match ctx.spec.os.uefi_fallback {
        Some(UefiFallbackMode::Rollback) => {
            debug!("UEFI fallback mode is set to rollback");
            match ctx.servicing_type {
                trident_api::status::ServicingType::CleanInstall => {
                    // For clean install, do nothing
                    debug!("Clean install detected. No action needed for UEFI rollback mode.");
                }
                trident_api::status::ServicingType::AbUpdate => {
                    // For update, find the servicing os boot files and copy them to EFI/BOOT/.
                    let active_boot_esp_dir_name = boot::make_esp_dir_name(
                        ctx.install_index,
                        match ctx.ab_active_volume {
                            None | Some(AbVolumeSelection::VolumeA) => AbVolumeSelection::VolumeA,
                            Some(AbVolumeSelection::VolumeB) => AbVolumeSelection::VolumeB,
                        },
                    );
                    let active_boot_esp_dir_path = mount_point
                        .join(ESP_RELATIVE_MOUNT_POINT_PATH)
                        .join(ESP_EFI_DIRECTORY)
                        .join(active_boot_esp_dir_name);
                    let uefi_fallback_path = mount_point
                        .join(ESP_RELATIVE_MOUNT_POINT_PATH)
                        .join(ESP_EFI_DIRECTORY)
                        .join(EFI_DEFAULT_BIN_DIRECTORY);
                    debug!(
                        "{:?} detected. Copying boot files from {} to {}",
                        ctx.servicing_type,
                        active_boot_esp_dir_path.display(),
                        uefi_fallback_path.display()
                    );
                    simple_copy_boot_files(&active_boot_esp_dir_path, &uefi_fallback_path)
                        .context(format!(
                            "Failed to copy boot files from directory {} to directory {}",
                            active_boot_esp_dir_path.display(),
                            uefi_fallback_path.display()
                        ))?;
                }
                _ => {
                    // Otherwise, should this be an error???
                    debug!("{:?} detected, no action needed.", ctx.servicing_type);
                }
            }
        }
        Some(UefiFallbackMode::Rollforward) => {
            debug!("UEFI fallback mode is set to rollforward");
            match ctx.servicing_type {
                trident_api::status::ServicingType::CleanInstall
                | trident_api::status::ServicingType::AbUpdate => {
                    // For install and update, copy COSI boot files to EFI/BOOT/.
                    // For update, find the servicing os boot files and copy them to EFI/BOOT/.
                    let next_boot_esp_dir_name = boot::make_esp_dir_name(
                        ctx.install_index,
                        match ctx.ab_active_volume {
                            None | Some(AbVolumeSelection::VolumeB) => AbVolumeSelection::VolumeA,
                            Some(AbVolumeSelection::VolumeA) => AbVolumeSelection::VolumeB,
                        },
                    );
                    let next_boot_esp_dir_path = mount_point
                        .join(ESP_RELATIVE_MOUNT_POINT_PATH)
                        .join(ESP_EFI_DIRECTORY)
                        .join(next_boot_esp_dir_name);
                    let uefi_fallback_path = mount_point
                        .join(ESP_RELATIVE_MOUNT_POINT_PATH)
                        .join(ESP_EFI_DIRECTORY)
                        .join(EFI_DEFAULT_BIN_DIRECTORY);
                    debug!(
                        "{:?} detected. Copying boot files from {} to {}.",
                        ctx.servicing_type,
                        next_boot_esp_dir_path.display(),
                        uefi_fallback_path.display()
                    );
                    simple_copy_boot_files(&next_boot_esp_dir_path, &uefi_fallback_path).context(
                        format!(
                            "Failed to copy boot files from directory {} to directory {}",
                            next_boot_esp_dir_path.display(),
                            uefi_fallback_path.display()
                        ),
                    )?;
                }
                _ => {
                    // Otherwise, should this be an error???
                    debug!("{:?} detected, no action needed.", ctx.servicing_type);
                }
            }
        }
        Some(UefiFallbackMode::None) | None => {
            debug!("No UEFI fallback mode is set");
        }
    }

    Ok(())
}

/// Copies boot files from one folder to another.
fn simple_copy_boot_files(from_dir: &Path, to_dir: &Path) -> Result<(), Error> {
    trace!(
        "Copying boot files from {} to {}",
        from_dir.display(),
        to_dir.display()
    );
    // Create to_dir if it doesn't exist
    if !Path::new(to_dir).exists() {
        fs::create_dir_all(to_dir)
            .context(format!("Failed to create directory {}", to_dir.display()))?;
    }

    // Copy all files from from_dir to to_dir as <existing_filename>.new
    fs::read_dir(from_dir)?.flatten().for_each(|from_path| {
        let to_file_name = format!("{}.new", from_path.file_name().to_string_lossy());
        let to_path = to_dir.join(to_file_name);
        debug!(
            "Copying file {} to {}",
            from_path.path().display(),
            to_path.display()
        );
        match fs::copy(from_path.path(), &to_path) {
            Ok(_) => debug!(
                "Copied file {} to {}",
                from_path.path().display(),
                to_path.display()
            ),
            Err(e) => debug!(
                "Failed to copy file {} to {}: {}",
                from_path.path().display(),
                to_path.display(),
                e
            ),
        }
    });

    // Rename all copied files from to_dir/<filename>.new to to_dir/<filename>
    fs::read_dir(to_dir)?.flatten().for_each(|orig_path| {
        let orig_file_name = orig_path.file_name();
        if !orig_file_name.to_string_lossy().ends_with(".new") {
            // Skip files that do not end with .new
            return;
        }
        let orig_file_name_string = orig_file_name.to_string_lossy();
        let new_file_name = orig_file_name_string.trim_end_matches(".new");
        let to_path = to_dir.join(new_file_name);
        trace!(
            "Renaming file {} to {}",
            orig_path.path().display(),
            to_path.display()
        );
        match fs::rename(orig_path.path(), &to_path) {
            Ok(_) => debug!(
                "Renamed file {} to {}",
                orig_path.path().display(),
                to_path.display()
            ),
            Err(e) => debug!(
                "Failed to rename file {} to {}: {}",
                orig_path.path().display(),
                to_path.display(),
                e
            ),
        }
    });
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
            "Failed to extract filename from path {}",
            boot_file.display()
        ))?;

        let destination_path = esp_dir.join(file_name);

        // Create directories if they don't exist
        if let Some(parent) = destination_path.parent() {
            fs::create_dir_all(parent)
                .context(format!("Failed to create directory {}", parent.display()))?;
        }

        debug!(
            "Copying file {} to {}",
            source_path.display(),
            destination_path.display()
        );
        fs::copy(&source_path, &destination_path).context(format!(
            "Failed to copy file {} to {}",
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
fn generate_boot_filepaths(temp_mount_dir: &Path) -> Result<Vec<PathBuf>, Error> {
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

    let selected_grub_config_path = if efi_boot_grub_path.exists() && efi_boot_grub_path.is_file() {
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
            "Failed to find shim EFI executable at path {}",
            boot_efi_path.display()
        );
    }
    debug!(
        "Using shim EFI executable from '{}'",
        boot_efi_path.display()
    );
    paths.push(boot_efi_path);

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
    let esp_efi_path = mount_point
        .join(ESP_RELATIVE_MOUNT_POINT_PATH)
        .join(ESP_EFI_DIRECTORY);

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

    use std::io::Write;

    use fs::File;

    use crate::engine::{
        boot::{get_update_esp_dir_name, make_esp_dir_name_candidates},
        install_index,
    };
    use trident_api::config::{HostConfiguration, Os};
    use trident_api::{
        constants::GRUB2_RELATIVE_PATH,
        status::{AbVolumeSelection, ServicingType},
    };

    #[test]
    fn test_generate_efi_bin_base_dir_path_clean_install() {
        // Clean install EngineContext
        let mut ctx = EngineContext {
            servicing_type: ServicingType::CleanInstall,
            ..Default::default()
        };

        let test_dir = TempDir::new().unwrap();
        let test_esp_dir = test_dir
            .path()
            .join(ESP_RELATIVE_MOUNT_POINT_PATH)
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
            ctx.install_index = install_index::next_install_index(test_dir.path()).unwrap();
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
                .join(ESP_RELATIVE_MOUNT_POINT_PATH)
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
            trace!("Created mock boot file {}", full_path.display());
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
            .join(ESP_RELATIVE_MOUNT_POINT_PATH)
            .join(ESP_EFI_DIRECTORY)
            .join(esp_name);
        fs::create_dir_all(&fallback_esp_dir).unwrap();
        create_boot_files(&fallback_esp_dir, file_names, content);
        fallback_esp_dir
    }

    fn validate_fallback(ctx: &EngineContext, file_names: &[PathBuf], azl_boot_name: &str) {
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

        configure_uefi_fallback(ctx, mount_point.path()).unwrap();
        for file in file_names {
            let fallback_file = fallback_esp_dir.join(file);
            let mut fallback_file_contents = String::new();
            File::open(&fallback_file)
                .unwrap()
                .read_to_string(&mut fallback_file_contents)
                .unwrap();
            match ctx.spec.os.uefi_fallback {
                None | Some(UefiFallbackMode::None) =>
                // Fallback should not have been updated
                {
                    assert!(
                        fallback_file_contents.contains(ORIGINAL_FALLBACK_CONTENT),
                        "{} was updated, but should not have been",
                        fallback_file.display()
                    )
                }
                Some(UefiFallbackMode::Rollback) | Some(UefiFallbackMode::Rollforward) =>
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
    fn test_configure_uefi_fallback() {
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
                    uefi_fallback: Some(UefiFallbackMode::Rollback),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Validate ABUpdate Rollback with active volume A ==> copy /EFI/AZLA to /EFI/BOOT
        ctx.spec.os.uefi_fallback = Some(UefiFallbackMode::Rollback);
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, &file_names, "AZLA");

        // Validate ABUpdate Rollback with active volume B ==> copy /EFI/AZLB to /EFI/BOOT
        ctx.spec.os.uefi_fallback = Some(UefiFallbackMode::Rollback);
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, &file_names, "AZLB");

        // Validate ABUpdate Rollforward with active volume A ==> copy /EFI/AZLA to /EFI/BOOT
        ctx.spec.os.uefi_fallback = Some(UefiFallbackMode::Rollforward);
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, &file_names, "AZLB");

        // Validate ABUpdate Rollforward with active volume B ==> copy /EFI/AZLB to /EFI/BOOT
        ctx.spec.os.uefi_fallback = Some(UefiFallbackMode::Rollforward);
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, &file_names, "AZLA");

        // Validate CleanInstall Rollforward with active volume None ==> copy /EFI/AZLA to /EFI/BOOT
        ctx.spec.os.uefi_fallback = Some(UefiFallbackMode::Rollforward);
        ctx.ab_active_volume = None;
        ctx.servicing_type = ServicingType::CleanInstall;
        validate_fallback(&ctx, &file_names, "AZLA");

        // Validate ABUpdate None with active volume A ==> /EFI/BOOT unchanged
        ctx.spec.os.uefi_fallback = Some(UefiFallbackMode::None);
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, &file_names, "AZLA");

        // Validate ABUpdate None with active volume A ==> /EFI/BOOT unchanged
        ctx.spec.os.uefi_fallback = None;
        ctx.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        ctx.servicing_type = ServicingType::AbUpdate;
        validate_fallback(&ctx, &file_names, "AZLA");
    }

    #[test]
    fn test_simple_copy_boot_files() {
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
        simple_copy_boot_files(from_dir.path(), to_dir.path()).unwrap();

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

        // Verify that existing files that were not in from_dir are unchanged
        for (file_name, content) in &existing_file_infos {
            if !file_infos.iter().any(|(f, _)| f == file_name) {
                let mut file_content = String::new();
                File::open(to_dir.path().join(file_name))
                    .unwrap()
                    .read_to_string(&mut file_content)
                    .unwrap();
                assert_eq!(
                    file_content.trim(),
                    *content,
                    "Content of existing file {file_name} does not match"
                );
            }
        }
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

            if file_name == PathBuf::from(GRUB_NOPREFIX_EFI) {
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

        let generated_paths_efi_boot = generate_boot_filepaths(temp_mount_dir.path()).unwrap();
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
            generate_boot_filepaths(temp_mount_dir.path())
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find grub.cfg",
            "generate_boot_filepaths() should fail if grub.cfg does not exist"
        );

        // Test case 3: Run generate_boot_filepaths() with GRUB under GRUB2_RELATIVE_PATH
        let boot_grub2_grub_path = temp_mount_dir
            .path()
            .join(GRUB2_RELATIVE_PATH)
            .join(GRUB2_CONFIG_FILENAME);
        fs::create_dir_all(boot_grub2_grub_path.parent().unwrap()).unwrap();
        File::create(&boot_grub2_grub_path).unwrap();

        let generated_paths_boot_grub2 = generate_boot_filepaths(temp_mount_dir.path()).unwrap();
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
            generate_boot_filepaths(temp_mount_dir.path())
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

        let generated_paths_noprefix = generate_boot_filepaths(temp_mount_dir.path()).unwrap();
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
            generate_boot_filepaths(temp_mount_dir.path())
                .unwrap_err()
                .root_cause()
                .to_string(),
            format!(
                "Failed to find shim EFI executable at path {}",
                boot_efi_path.display()
            )
        );
    }
}
