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
    filesystems::MountFileSystemType,
    hashing_reader::{HashingReader, HashingReader384},
    image_streamer,
    mount::{self, MountGuard},
    path::join_relative,
};
use sysdefs::arch::SystemArchitecture;
use trident_api::{
    constants::{
        internal_params::{DISABLE_GRUB_NOPREFIX_CHECK, ENABLE_UKI_SUPPORT},
        ESP_MOUNT_POINT_PATH,
    },
    error::{ReportError, TridentError, TridentResultExt, UnsupportedConfigurationError},
    status::AbVolumeSelection,
};

use crate::engine::{
    boot::ESP_EXTRACTION_DIRECTORY,
    constants::{
        EFI_DEFAULT_BIN_RELATIVE_PATH, ESP_EFI_DIRECTORY, ESP_RELATIVE_MOUNT_POINT_PATH,
        GRUB2_CONFIG_FILENAME, GRUB2_CONFIG_RELATIVE_PATH,
    },
    EngineContext,
};

/// Takes in a reader to the raw zstd-compressed ESP image and decompresses it
/// into a temporary file. Returns a tuple containing the temporary file and the
/// computed hash (SHA256 or SHA384) of the image.
///
/// It also takes in the URL of the image to be shown in case of errors.
fn load_raw_image<R>(source: &Url, reader: R) -> Result<(NamedTempFile, String), Error>
where
    R: Read + HashingReader,
{
    // Create a temporary file to download ESP image
    let temp_image = NamedTempFile::new_in(ESP_EXTRACTION_DIRECTORY)
        .context("Failed to create a temporary file")?;
    let temp_image_path = temp_image.path().to_path_buf();

    debug!("Extracting ESP image to {}", temp_image_path.display());

    // Stream image to the temporary file.
    let computed_hash = image_streamer::stream_zstd(reader, &temp_image_path)
        .context(format!("Failed to stream ESP image from {}", source))?;

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
    let boot_files = generate_boot_filepaths(temp_mount_dir, SystemArchitecture::current())
        .context("Failed to generate boot filepaths")?;

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
    let grub_noprefix =
        copy_boot_files(temp_mount_dir, &esp_dir_path, boot_files).context(format!(
            "Failed to copy boot files from directory {} to directory {}",
            temp_mount_dir.display(),
            esp_dir_path.display()
        ))?;

    if ctx.spec.internal_params.get_flag(ENABLE_UKI_SUPPORT) {
        let esp_root_path = join_relative(mount_point, ESP_MOUNT_POINT_PATH);

        // The directory where systemd-boot looks for UKIs.
        let esp_uki_directory = esp_root_path.join("EFI/Linux");

        // Create the EFI/Linux directory on the ESP if it doesn't exist.
        fs::create_dir_all(&esp_uki_directory)
            .context("Failed to create 'EFI/Linux' on the ESP")?;

        // Find all UKIs within the image. There should only be one.
        let ukis: Vec<_> = temp_mount_dir
            .join("EFI/Linux")
            .read_dir()
            .context("Could not read UKI directory")?
            .collect::<Result<Vec<_>, _>>()
            .context("Failed while reading UKI directory")?
            .into_iter()
            .map(|entry| entry.path())
            .collect();
        ensure!(!ukis.is_empty(), "No UKI files found within the image");
        ensure!(ukis.len() == 1, "Multiple UKI files found within the image");

        // Copy the UKI from the image into the AZLA/ALZB directory. Eventually the UKI will be
        // placed into /EFI/Linux on the ESP. But in order to do that atomically, the file needs to
        // already be located somewhere on the ESP.
        const TMP_UKI_NAME: &str = "vmlinuz.efi";
        fs::copy(&ukis[0], esp_dir_path.join(TMP_UKI_NAME))
            .context("Failed to copy UKI to the ESP")?;

        // Create 'loader/entries.srel' on the ESP as required by the Boot Loader Specification.
        fs::create_dir_all(esp_root_path.join("loader"))
            .context("Failed to create directory loader")?;
        fs::write(esp_root_path.join("loader/entries.srel"), "type1\n")
            .context("Failed to write entries.srel")?;

        // Update the boot order used by systemd-boot.
        //
        // Every UKI placed by Trident will have a name of the form
        // 'vmlinuz-<N>-azl<volume><index>.efi'. Due to the way systemd-boot works, the UKI with the
        // highest N will be first in the boot order. The volume and install index portions of the
        // name are used to map UKIs to the particular install index and A/B volume that created
        // them.
        //
        // In the loop below, we delete any existing UKIs with the current install index and A/B
        // update volume, and record the highest N of any UKI that remains. Once the loop is
        // finished, this enables us to place the new UKI at the next highest N so that it will be
        // first in the boot order.
        //
        // TODO: The rename should really happen during 'finalize' rather than when the ESP image is
        // being written.
        let uki_suffix = match ctx.ab_active_volume {
            Some(AbVolumeSelection::VolumeA) => format!("azlb{}.efi", ctx.install_index),
            None | Some(AbVolumeSelection::VolumeB) => {
                format!("azla{}.efi", ctx.install_index)
            }
        };
        let mut max_index = 99;
        let entries = fs::read_dir(&esp_uki_directory).context(format!(
            "Failed to read directory '{}'",
            esp_uki_directory.display()
        ))?;
        for entry in entries {
            let entry = entry.context("Failed to read entry")?;
            let filename = entry.file_name();

            // Parse the filename according to Trident's naming scheme. Any UKIs that don't match
            // the naming scheme are for some other unknown install and will be left in place. This
            // means they'll either be prioritized before or after the UKI Trident is placing, but
            // they won't be deleted.
            let Some((index, suffix)) = filename
                .to_str()
                .and_then(|filename| filename.strip_prefix("vmlinuz-"))
                .and_then(|f| f.split_once('-'))
                .and_then(|(index, suffix)| Some((index.parse::<usize>().ok()?, suffix)))
            else {
                trace!(
                    "Ignoring existing UKI file '{}' that does not match Trident naming scheme",
                    entry.path().display()
                );
                continue;
            };

            if suffix == uki_suffix {
                fs::remove_file(entry.path()).context(format!(
                    "Failed to remove file '{}'",
                    entry.path().display()
                ))?;
            } else {
                max_index = max_index.max(index);
            }
        }
        fs::rename(
            esp_dir_path.join(TMP_UKI_NAME),
            esp_uki_directory.join(format!("vmlinuz-{}-{uki_suffix}", max_index + 1)),
        )
        .context("Failed to rename UKI file")?;
    }

    // Bail if grub_noprefix.efi is not found on Azure Linux images.
    if !grub_noprefix
        && !ctx
            .spec
            .internal_params
            .get_flag(DISABLE_GRUB_NOPREFIX_CHECK)
    {
        let arch =
            current_arch_efi_str().context("Failed to get the target architecture string")?;
        bail!("Cannot locate grub{}-noprefix.efi in the boot image. Verify if the grub2-efi-binary-noprefix package was installed on the boot image.", arch);
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
        if file_name == format!("grub{}-noprefix.efi", current_arch_efi_str()?).as_str() {
            fs::rename(
                &destination_path,
                esp_dir
                    .join(format!("grub{}.efi", current_arch_efi_str()?))
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
/// The func takes in 2 arg-s:
/// 1. temp_mount_dir, which is the path to the directory where the ESP image is mounted to,
/// 2. efi_filename_ending, which is the filename ending of the EFI executable. E.g., if the target
///    architecture is x86_64, the arg needs to be "x64" since the EFI executable for x86_64 is
///    named "grubx64.efi."
fn generate_boot_filepaths(
    temp_mount_dir: &Path,
    arch: SystemArchitecture,
) -> Result<Vec<PathBuf>, Error> {
    let mut paths = Vec::new();

    let efi_filename_ending = get_arch_efi_str(arch)?;

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

    // Check if grubx64-noprefix.efi exists; otherwise, use grubx64.efi. With the package update
    // to use grub2-efi-binary-noprefix RPM, the EFI executable is installed as
    // grubx64-noprefix.efi.
    let grub_efi_noprefix_path = Path::new(temp_mount_dir)
        .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
        .join(format!("grub{}-noprefix.efi", efi_filename_ending));
    let grub_efi_path = Path::new(temp_mount_dir)
        .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
        .join(format!("grub{}.efi", efi_filename_ending));

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
        .join(format!("boot{}.efi", efi_filename_ending));
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

fn current_arch_efi_str() -> Result<&'static str, Error> {
    get_arch_efi_str(SystemArchitecture::current())
}

/// Returns the name of the given architecture for use in EFI.
fn get_arch_efi_str(arch: SystemArchitecture) -> Result<&'static str, Error> {
    Ok(match arch {
        SystemArchitecture::X86 => "ia32",
        SystemArchitecture::Amd64 => "x64",
        SystemArchitecture::Arm => "arm",
        SystemArchitecture::Aarch64 => "aa64",
        SystemArchitecture::Other => bail!("Failed to generate the filename of EFI executable as the target architecture is not supported"),
    })
}

pub fn next_install_index(mount_point: &Path) -> Result<usize, TridentError> {
    let esp_efi_path = mount_point
        .join(ESP_RELATIVE_MOUNT_POINT_PATH)
        .join(ESP_EFI_DIRECTORY);

    debug!(
        "Looking for next available install index in '{}'",
        esp_efi_path.display()
    );
    let first_available_install_index = find_first_available_install_index(&esp_efi_path)
        .message("Failed to find the first available install index")?;

    debug!("Selected first available install index: '{first_available_install_index}'",);
    Ok(first_available_install_index)
}

/// Returns the path to the ESP directory where the boot files need to be copied
/// to.
///
/// Path will be in the form of /boot/efi/EFI/<ID>, where <ID> is the install ID
/// as determined by ctx.
///
/// The function will find the next available install ID for this install and
/// update the install index in the engine context.
pub fn generate_efi_bin_base_dir_path(
    ctx: &EngineContext,
    mount_point: &Path,
) -> Result<PathBuf, Error> {
    // Compose the path to the ESP directory
    let esp_efi_path = mount_point
        .join(ESP_RELATIVE_MOUNT_POINT_PATH)
        .join(ESP_EFI_DIRECTORY);

    // Return the path to the ESP directory with the ESP dir name
    Ok(
        esp_efi_path.join(super::get_update_esp_dir_name(ctx).context(
            "Failed to get ESP directory name for the new OS. Engine context is in an invalid state.",
        )?),
    )
}

/// Tries to find the next available AzL install index by looking at the
/// ESP directory names present in the specified ESP EFI path.
fn find_first_available_install_index(esp_efi_path: &Path) -> Result<usize, TridentError> {
    Ok(super::make_esp_dir_name_candidates()
        // Take a limited number of candidates to avoid an infinite loop.
        .take(1000)
        // Go over all the candidates and find the first one that doesn't exist.
        .find(|(idx, dir_names)| {
            trace!("Checking if an install with index '{}' exists", idx);
            // Returns true if all possible ESP directory names for this index
            // do NOT exist.
            dir_names.iter().all(|dir_names| {
                let path = esp_efi_path.join(dir_names);
                trace!("Checking if path '{}' exists", path.display());
                !path.exists()
            })
        })
        .structured(UnsupportedConfigurationError::NoAvailableInstallIndex)?
        .0)
}

/// Performs file-based deployment of ESP images from the OS image.
pub(super) fn deploy_esp(ctx: &EngineContext, mount_point: &Path) -> Result<(), Error> {
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

    let (temp_file, computed_sha384) =
        load_raw_image(os_image.source(), HashingReader384::new(stream))
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

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write;

    use fs::File;

    use trident_api::{
        constants::GRUB2_RELATIVE_PATH,
        status::{AbVolumeSelection, ServicingType},
    };

    use crate::engine::boot::{get_update_esp_dir_name, make_esp_dir_name_candidates};

    /// Validates that generate_arch_str() returns the correct string based on target architecture
    #[test]
    fn test_generate_arch_str() {
        let mut expected_arch = "";
        if cfg!(target_arch = "x86_64") {
            expected_arch = "x64";
        } else if cfg!(target_arch = "x86") {
            expected_arch = "ia32";
        } else if cfg!(target_arch = "arm") {
            expected_arch = "arm";
        } else if cfg!(target_arch = "aarch64") {
            expected_arch = "aa64";
        } else {
            assert!(current_arch_efi_str().is_err(), "generate_arch_str() should return an error if target architecture is not supported");
        };

        let generated_arch = current_arch_efi_str().unwrap();
        assert_eq!(
            generated_arch, expected_arch,
            "Architecture string does not match expected value"
        );
    }

    /// Simple case for find_first_available_install_index
    #[test]
    fn test_find_first_available_install_index_simple() {
        let test_dir = TempDir::new().unwrap();
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 0, "First available index should be 0");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices
    #[test]
    fn test_find_first_available_install_index_existing_all() {
        let test_dir = TempDir::new().unwrap();

        // Create all ESP directories for indices 0-9
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                for dir_name in dir_names {
                    fs::create_dir(test_dir.path().join(dir_name)).unwrap();
                }
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices, even when only the A volume IDs are present
    #[test]
    fn test_find_first_available_install_index_existing_a() {
        let test_dir = TempDir::new().unwrap();

        // Create Volume A ESP directories for indices 0-9
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(test_dir.path().join(&dir_names[0])).unwrap();
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices, even when only the B volume IDs are present
    #[test]
    fn test_find_first_available_install_index_existing_b() {
        let test_dir = TempDir::new().unwrap();

        // Create Volume B ESP directories for indices 0-9
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(test_dir.path().join(&dir_names[1])).unwrap();
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices, even when only ONE ID is present per install.
    #[test]
    fn test_find_first_available_install_index_existing_mixed_1() {
        let test_dir = TempDir::new().unwrap();

        // Iterator to cycle between 0 and 1
        let mut volume_selector = (0..=1).cycle();

        // Create alternating A/B Volume ESP directories for indices 0-9, starting with A
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(
                    test_dir
                        .path()
                        .join(&dir_names[volume_selector.next().unwrap()]),
                )
                .unwrap();
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }

    /// Test that find_first_available_install_index will skip unavailable
    /// indices, even when only ONE ID is present per install.
    #[test]
    fn test_find_first_available_install_index_existing_mixed_2() {
        let test_dir = TempDir::new().unwrap();

        // Iterator to cycle between 0 and 1
        let mut volume_selector = (0..=1).cycle();

        // Advance the volume selector to start with B
        volume_selector.next();

        // Create alternating A/B Volume ESP directories for indices 0-9, starting with B
        make_esp_dir_name_candidates()
            .take(10)
            .for_each(|(_, dir_names)| {
                fs::create_dir(
                    test_dir
                        .path()
                        .join(&dir_names[volume_selector.next().unwrap()]),
                )
                .unwrap();
            });

        // The first available index should be 10
        let index = find_first_available_install_index(test_dir.path()).unwrap();
        assert_eq!(index, 10, "First available index should be 10");
    }

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
            ctx.install_index = next_install_index(test_dir.path()).unwrap();
            assert_eq!(idx, ctx.install_index);

            let esp_dir_path = generate_efi_bin_base_dir_path(&ctx, test_dir.path()).unwrap();
            println!("Returned ESP directory path: {:?}", esp_dir_path);
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
    fn create_boot_files(temp_mount_dir: &Path, boot_files: &[PathBuf]) {
        for path in boot_files {
            let full_path = temp_mount_dir.join(path);

            if let Some(parent) = full_path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let mut file = File::create(full_path).unwrap();
            writeln!(file, "Mock content for {}", path.display()).unwrap();
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

    /// Validates that copy_boot_files() correctly copies boot files from temp_mount_dir to esp_dir
    #[test]
    fn test_copy_boot_files_without_noprefix() {
        let temp_mount_dir = TempDir::new().unwrap();
        let esp_dir = TempDir::new().unwrap();

        // Create a list of boot files
        let file_names = vec![
            PathBuf::from(GRUB2_CONFIG_FILENAME),
            PathBuf::from("grubx64.efi"),
            PathBuf::from("bootx64.efi"),
        ];

        // Call helper func to create mock boot files in temp_mount_dir
        create_boot_files(temp_mount_dir.path(), &file_names);
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
            PathBuf::from("grubx64-noprefix.efi"),
            PathBuf::from("bootx64.efi"),
        ];

        // Call helper func to create mock boot files in temp_mount_dir
        create_boot_files(temp_mount_dir.path(), &file_names);
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

            if file_name == PathBuf::from("grubx64-noprefix.efi") {
                destination_path = esp_dir.path().join("grubx64.efi");
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
            .join("grubx64.efi");
        File::create(&grub_efi_path).unwrap();

        // Create a boot EFI executable inside of the temp dir
        let boot_efi_path = temp_mount_dir
            .path()
            .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
            .join("bootx64.efi");
        File::create(&boot_efi_path).unwrap();

        let generated_paths_efi_boot =
            generate_boot_filepaths(temp_mount_dir.path(), SystemArchitecture::Amd64).unwrap();
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
            generate_boot_filepaths(temp_mount_dir.path(), SystemArchitecture::Amd64)
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

        let generated_paths_boot_grub2 =
            generate_boot_filepaths(temp_mount_dir.path(), SystemArchitecture::Amd64).unwrap();
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
            generate_boot_filepaths(temp_mount_dir.path(), SystemArchitecture::Amd64)
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
            .join("grubx64-noprefix.efi");
        File::create(&grub_efi_noprefix_path).unwrap();

        let generated_paths_noprefix =
            generate_boot_filepaths(temp_mount_dir.path(), SystemArchitecture::Amd64).unwrap();
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
            generate_boot_filepaths(temp_mount_dir.path(), SystemArchitecture::Amd64)
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
