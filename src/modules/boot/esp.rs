use std::{
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, Context, Error};
use log::{debug, info};
use reqwest::Url;
use tempfile::{NamedTempFile, TempDir};

use osutils::{
    filesystems::MountFileSystemType,
    hashing_reader::HashingReader,
    image_streamer,
    mount::{self, MountGuard},
};
use trident_api::{
    config::{HostConfiguration, Image, ImageFormat, ImageSha256, PartitionType},
    status::{AbVolumeSelection, BlockDeviceContents, HostStatus},
};

use crate::modules::{
    self,
    constants::{
        EFI_DEFAULT_BIN_RELATIVE_PATH, ESP_EFI_DIRECTORY, ESP_RELATIVE_MOUNT_POINT_PATH,
        GRUB2_CONFIG_FILENAME, GRUB2_CONFIG_RELATIVE_PATH,
    },
    storage::{
        self,
        image::{
            self,
            stream_image::{self, GET_MAX_RETRIES, GET_TIMEOUT_SECS},
        },
    },
    BOOT_ENTRY_A, BOOT_ENTRY_B,
};

/// Performs file-based update of stand-alone ESP volume by copying three boot files into the
/// correct dir:
/// 1. If volume A is currently active, place in /boot/efi/EFI/azlinuxB,
/// 2. If volume B is currently active, place in /boot/efi/EFI/azlinuxA,
/// 3. If no volume is active, i.e. Trident is doing CleanInstall, place in /boot/efi/EFI/azlinuxA.
///
/// The func takes the following arguments:
/// 1. image_url: &Url, which is the URL of the image to be downloaded,
/// 2. image: &Image, which is the Image object from HostConfig,
/// 3. host_status: &mut HostStatus, which is the HostStatus object,
/// 4. is_local: bool, which is a boolean indicating whether the image is a local file or not.
///
/// Local image and dir it's mounted to are temp, so they're automatically removed from the FS
/// after function returns.
fn copy_file_artifacts(
    image_url: &Url,
    image: &Image,
    host_status: &mut HostStatus,
    is_local: bool,
    mount_point: &Path,
) -> Result<(), Error> {
    // Check whether image_url is local or remote
    let stream: Box<dyn Read> = if is_local {
        // For local files, open the file at the given path
        Box::new(File::open(image_url.path()).context(format!("Failed to open {}", image_url))?)
    } else {
        // For remote files, perform a blocking GET request
        stream_image::exponential_backoff_get(
            image_url,
            GET_MAX_RETRIES,
            Duration::from_secs(GET_TIMEOUT_SECS),
        )?
    };

    // Initialize HashingReader instance on stream
    let reader = HashingReader::new(stream);
    // Create a temporary file to download ESP image
    let temp_image = NamedTempFile::new().context("Failed to create a temporary file")?;
    let temp_image_path = temp_image.path().to_path_buf();

    debug!("Extracting ESP image to {}", temp_image_path.display());

    // Stream image to the temporary file. destination_size is None since we're writing to a new
    // file and not block device
    let (computed_sha256, bytes_copied) =
        image_streamer::stream_zstd(reader, &temp_image_path, None)
            .context(format!("Failed to stream ESP image from {}", image_url))?;

    // Create a temporary directory to mount ESP image
    let temp_dir = TempDir::new().context("Failed to create a temporary mount directory")?;
    let temp_mount_dir = temp_dir.path();

    // Mount image to temp dir
    mount::mount(
        &temp_image_path,
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
    let esp_dir_path = generate_efi_bin_base_dir_path(host_status, mount_point)?;

    // Call helper func to copy files from mounted img dir to esp_dir_path
    info!("Writing boot files to directory {}", esp_dir_path.display());
    // Generate file name of EFI executable based on target architecture
    let arch_str = generate_arch_str()
        .context("Failed to generate arch string based on target architecture")?;

    // Generate list of filepaths to the boot files. Pass in the temp dir path where the image is
    // mounted to as an argument
    let boot_files = generate_boot_filepaths(temp_mount_dir, &arch_str)
        .context("Failed to generate boot filepaths")?;

    // Clear esp_dir_path if it exists
    if esp_dir_path.exists() {
        info!("Clearing directory {}", esp_dir_path.display());
        osutils::files::clean_directory(esp_dir_path.clone()).context(format!(
            "Failed to clean directory {}",
            esp_dir_path.display()
        ))?;
    } else {
        // Create esp_dir_path if it doesn't exist
        info!("Creating directory {}", esp_dir_path.display());
        fs::create_dir_all(esp_dir_path.clone()).context(format!(
            "Failed to create directory {}",
            esp_dir_path.display()
        ))?;
    }

    let is_mariner_2_0 =
        check_mariner_2_0().context("Failed to check if the system is mariner 2.0")?;
    // Call helper func to copy boot files from temp_mount_dir to esp_dir_path
    copy_boot_files(temp_mount_dir, &esp_dir_path, boot_files, is_mariner_2_0).context(format!(
        "Failed to copy boot files from directory {} to directory {}",
        temp_mount_dir.display(),
        esp_dir_path.display()
    ))?;

    // Update HostStatus.
    // TODO: Setting BlockDeviceContents to Image contents, like for any volume, while updating the
    // ESP volume is a temporary solution. In the next PR, Trident will set contents of the
    // partition to ESPImage, a new value in the BlockDeviceContents object. Related ADO task:
    // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6625.

    storage::set_host_status_block_device_contents(
        host_status,
        &image.target_id,
        BlockDeviceContents::Image {
            sha256: computed_sha256.clone(),
            length: bytes_copied,
            url: image_url.to_string(),
        },
    )?;

    // If SHA256 is ignored, log message and skip hash validation; otherwise, ensure computed
    // SHA256 matches SHA256 in HostConfig
    match image.sha256 {
        ImageSha256::Ignored => {
            info!("Ignoring SHA256 for image from '{}'", image_url);
        }
        ImageSha256::Checksum(ref expected_sha256) => {
            if computed_sha256 != *expected_sha256 {
                bail!(
                    "SHA256 mismatch for disk image {}: expected {}, got {}",
                    image_url,
                    expected_sha256,
                    computed_sha256
                );
            }
        }
    }

    Ok(())
}

//function to encapsulate the version check
fn check_mariner_2_0() -> Result<bool, Error> {
    let version = fs::read_to_string("/etc/mariner-release")
        .context("Failed to read /etc/mariner-release")?;
    Ok(version.contains("2.0"))
}

/// Copies boot files from temp_mount_dir, where image was mounted to, to given dir esp_dir.
fn copy_boot_files(
    temp_mount_dir: &Path,
    esp_dir: &Path,
    boot_files: Vec<PathBuf>,
    is_mariner_2_0: bool,
) -> Result<(), Error> {
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

        info!(
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
        if file_name == format!("grub{}-noprefix.efi", generate_arch_str()?).as_str() {
            fs::rename(
                &destination_path,
                esp_dir
                    .join(format!("grub{}.efi", generate_arch_str()?))
                    .to_str()
                    .context("Failed to convert path to string")?,
            )
            .context("Failed to rename grub-noprefix efi")?;
            no_prefix = true;
        }
    }

    // Fail if no-prefix is not used on mariner 2.0
    if !no_prefix {
        // Check if we are on mariner 2.0
        if is_mariner_2_0 {
            bail!("grub-noprefix.efi should be used on mariner 2.0 images for trident");
        } else {
            debug!("grub-noprefix.efi is not used and the system is not on mariner 2.0");
        }
    } else {
        info!("grub-noprefix.efi is used");
    }

    Ok(())
}

/// Generates a list of filepaths to the boot files that need to be copied to implement file-based
/// update of ESP, relative to the mounted directory.
///
/// The func takes in 2 arg-s:
/// 1. temp_mount_dir, which is the path to the directory where the ESP image is mounted to,
/// 2. efi_filename_ending, which is the filename ending of the EFI executable. E.g., if the target
/// architecture is x86_64, the arg needs to be "x64" since the EFI executable for x86_64 is named
/// "grubx64.efi."
fn generate_boot_filepaths(
    temp_mount_dir: &Path,
    efi_filename_ending: &str,
) -> Result<Vec<PathBuf>, Error> {
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

/// Returns the arch string based on the target architecture.
///
/// E.g., if the target architecture is x86_64, the function returns "x64" since the EFI executable
/// for x86_64 is named "grubx64.efi."
fn generate_arch_str() -> Result<String, Error> {
    let arch_string = match () {
        _ if cfg!(target_arch = "x86_64") => "x64",
        _ if cfg!(target_arch = "x86") => "ia32",
        _ if cfg!(target_arch = "arm") => "arm",
        _ if cfg!(target_arch = "aarch64") => "aa64",
        // Add more architectures as needed
        _ => bail!("Failed to generate the filename of EFI executable as the target architecture is not supported"),
    };

    Ok(arch_string.to_string())
}

/// Returns the path to the ESP directory where the boot files need to be copied to.
fn generate_efi_bin_base_dir_path(
    host_status: &HostStatus,
    mount_point: &Path,
) -> Result<PathBuf, Error> {
    // Compose the path to the ESP directory

    // Path to the EFI directory on the ESP volume mount point path, /boot/efi, where EFI executables
    // will be placed on the updated volume, as part of file-based update of ESP:
    // a. If volume A is currently active, copy boot files into /boot/efi/EFI/azlinuxB,
    // b. If volume B is currently active OR no volume is currently active, i.e., Trident is doing
    // CleanInstall, copy boot files into /boot/efi/EFI/azlinuxA.
    let esp_efi_path = mount_point
        .join(ESP_RELATIVE_MOUNT_POINT_PATH)
        .join(ESP_EFI_DIRECTORY);

    // Based on which volume is being updated, determine how to name the dir
    let esp_dir_path = match modules::get_ab_update_volume(host_status, false)
        .context("Failed to determine which A/B volume is currently inactive")?
    {
        AbVolumeSelection::VolumeA => Path::new(&esp_efi_path).join(BOOT_ENTRY_A),
        AbVolumeSelection::VolumeB => Path::new(&esp_efi_path).join(BOOT_ENTRY_B),
    };

    Ok(esp_dir_path)
}

/// Function that fetches the list of ESP images that need to be updated and performs file-based
/// update of standalone ESP partition.
pub(super) fn update_images(
    host_status: &mut HostStatus,
    host_config: &HostConfiguration,
    mount_point: &Path,
) -> Result<(), Error> {
    // Fetch the list of ESP images that need to be updated/deployed
    for image in get_undeployed_images(host_status, host_config, false) {
        debug!(
            "Updating ESP filesystem on block device id '{}'",
            &image.target_id
        );

        // Parse the URL to determine the download strategy
        let image_url = Url::parse(image.url.as_str())
            .context(format!("Failed to parse image URL '{}'", image.url))?;

        // Only need to perform file-based update of ESP if image is in format RawZstd b/c RawLzma
        // requires a block-based update of ESP
        if image.format == ImageFormat::RawZst {
            info!(
                "Performing file-based update of ESP partition with id '{}'",
                &image.target_id
            );

            info!(
                "Deploying image {} onto ESP partition with id {}",
                image.url, image.target_id
            );

            if image_url.scheme() == "file" {
                // 5th arg is true to communicate that image is a local file, i.e.,  is_local
                // will be set to true
                copy_file_artifacts(&image_url, image, host_status, true, mount_point).context(
                    format!(
                    "Failed to deploy image {} onto ESP partition with id {} via direct streaming",
                    image.url, image.target_id
                ),
                )?;
            } else if image_url.scheme() == "http" || image_url.scheme() == "https" {
                // 5th arg is false to communicate that image is a local file, i.e.,  is_local
                // will be set to false
                copy_file_artifacts(&image_url, image, host_status, false, mount_point).context(
                    format!(
                    "Failed to deploy image {} onto ESP partition with id {} via direct streaming",
                    image.url, image.target_id
                ),
                )?;
            } else if image_url.scheme() == "oci" {
                bail!("Downloading images as OCI artifacts from Azure container registry is not implemented")
            } else {
                bail!("Unsupported URL scheme")
            }
        } else {
            bail!(
                "Unsupported image format for ESP partition with id '{}': {:?}",
                &image.target_id,
                image.format
            );
        }
    }
    Ok(())
}

/// Returns a list of images that correspond to ESP partition that need to be updated/provisioned.
///
/// Uses get_undeployed_images() to fetch the list of images that need to be updated/deployed and
/// then filters the vector to find images that corresponds to ESP partition.
fn get_undeployed_images<'a>(
    host_status: &HostStatus,
    host_config: &'a HostConfiguration,
    active: bool,
) -> Vec<&'a Image> {
    // Fetch the list of images that need to be updated/deployed
    let undeployed_images = image::get_undeployed_images(host_status, host_config, active);

    // Filter the vector to find images that corresponds to ESP partition
    undeployed_images
        .into_iter()
        .filter(|image| {
            // Check if image's target_id corresponds to a PartitionType::Esp
            host_status
                .spec
                .storage
                .disks
                .iter()
                .flat_map(|disk| &disk.partitions)
                .any(|partition| {
                    partition.id == image.target_id
                        && partition.partition_type == PartitionType::Esp
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    use maplit::btreemap;

    use trident_api::{
        config::{self, AbUpdate, AbVolumePair, PartitionSize, PartitionType},
        constants::{ROOT_MOUNT_POINT_PATH, UPDATE_ROOT_PATH},
        status::{BlockDeviceInfo, ReconcileState, Storage, UpdateKind},
    };

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
            assert!(generate_arch_str().is_err(), "generate_arch_str() should return an error if target architecture is not supported");
        };

        let generated_arch = generate_arch_str().unwrap();
        assert_eq!(
            generated_arch, expected_arch,
            "Architecture string does not match expected value"
        );
    }

    /// Validates logic for setting block device contents
    #[test]
    fn test_generate_esp_dir_path() {
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![config::Disk {
                        id: "os".to_owned(),
                        partitions: vec![
                            config::Partition {
                                id: "efi".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::Fixed(1000),
                            },
                            config::Partition {
                                id: "root".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(1000),
                            },
                            config::Partition {
                                id: "rootb".to_owned(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(1000),
                            },
                        ],
                        ..Default::default()
                    }],
                    ab_update: Some(AbUpdate {
                        volume_pairs: vec![AbVolumePair {
                            id: "osab".to_string(),
                            volume_a_id: "root".to_string(),
                            volume_b_id: "rootb".to_string(),
                        }],
                    }),

                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "os".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "efi".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp1"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "root".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp2"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "rootb".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-partlabel/osp3"),
                        size: 0,
                        contents: BlockDeviceContents::Unknown,
                    },
                    "data".into() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/disk/by-bus/foobar"),
                        size: 1000,
                        contents: BlockDeviceContents::Unknown,
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case 1: If no volume is currently active, generate_esp_dir_path() should return
        // /boot/efi/EFI/azlinuxA
        assert!(
            generate_efi_bin_base_dir_path(&host_status, Path::new(UPDATE_ROOT_PATH))
                .unwrap()
                .ends_with(BOOT_ENTRY_A),
            "generate_esp_dir_path() should return /boot/efi/EFI/AZLA if no volume is currently active"
        );

        // Test case 2: If volume A is currently active, generate_esp_dir_path() should return
        // /boot/efi/EFI/azlinuxB
        // Modify host_status to set active_volume to volume A
        host_status.reconcile_state = ReconcileState::UpdateInProgress(UpdateKind::AbUpdate);
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeA);
        assert!(
            generate_efi_bin_base_dir_path(&host_status, Path::new(UPDATE_ROOT_PATH))
                .unwrap()
                .ends_with(BOOT_ENTRY_B),
            "generate_esp_dir_path() should return /boot/efi/EFI/AZLB if volume A is currently active"
        );

        // Test case 3: If volume B is currently active, generate_esp_dir_path() should return
        // /boot/efi/EFI/azlinuxA
        // Modify host_status to set active_volume to volume B
        host_status.storage.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        assert!(
            generate_efi_bin_base_dir_path(&host_status, Path::new(UPDATE_ROOT_PATH))
                .unwrap()
                .ends_with(BOOT_ENTRY_A),
            "generate_esp_dir_path() should return /boot/efi/EFI/AZLA if volume B is currently active"
        );
    }

    /// Validates that get_undeployed_esp() returns the correct list of images that need to be
    /// updated/provisioned
    #[test]
    fn test_get_undeployed_esp() {
        // Initialize a HostStatus object with ESP and root partitions
        let mut host_status = HostStatus {
            reconcile_state: ReconcileState::CleanInstall,
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![config::Disk {
                        id: "foo".to_string(),
                        partitions: vec![
                            config::Partition {
                                id: "esp".to_string(),
                                partition_type: PartitionType::Esp,
                                size: PartitionSize::Fixed(1000),
                            },
                            config::Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: PartitionSize::Fixed(1000),
                            },
                        ],
                        ..Default::default()
                    }],
                    mount_points: vec![
                        config::MountPoint {
                            path: PathBuf::from("/boot"),
                            target_id: "esp".to_string(),
                            filesystem: config::FileSystemType::Vfat,
                            options: vec![],
                        },
                        config::MountPoint {
                            path: PathBuf::from(ROOT_MOUNT_POINT_PATH),
                            target_id: "root".to_string(),
                            filesystem: config::FileSystemType::Ext4,
                            options: vec![],
                        },
                    ],
                    images: vec![
                        Image {
                            url: "http://example.com/esp_1.img".to_string(),
                            target_id: "esp".to_string(),
                            format: ImageFormat::RawZst,
                            sha256: ImageSha256::Checksum("esp_sha256_1".to_string()),
                        },
                        Image {
                            url: "http://example.com/root_1.img".to_string(),
                            target_id: "root".to_string(),
                            format: ImageFormat::RawZst,
                            sha256: ImageSha256::Checksum("root_sha256_1".to_string()),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            storage: Storage {
                block_devices: btreemap! {
                    "foo".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda"),
                        size: 10,
                        contents: BlockDeviceContents::Initialized,
                    },
                    "esp".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda1"),
                        size: 3,
                        contents: BlockDeviceContents::Image {
                            url: "http://example.com/esp_1.img".to_string(),
                            sha256: "esp_sha256_1".to_string(),
                            length: 100,
                        },
                    },
                    "root".to_string() => BlockDeviceInfo {
                        path: PathBuf::from("/dev/sda2"),
                        size: 7,
                        contents: BlockDeviceContents::Image {
                            url: "http://example.com/root_1.img".to_string(),
                            sha256: "root_sha256_1".to_string(),
                            length: 100,
                        },
                    },
                },
                ..Default::default()
            },
            ..Default::default()
        };

        // Test case 1: ESP partition does not need to be updated
        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false),
            Vec::<&Image>::new(),
            "Incorrectly identified ESP partition as needing an update"
        );

        // Test case 2: ESP partition needs to be updated
        host_status.spec.storage.images[0].sha256 =
            ImageSha256::Checksum("esp_sha256_2".to_string());
        host_status.spec.storage.images[0].url = "http://example.com/esp_2.img".to_string();
        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false),
            vec![&Image {
                url: "http://example.com/esp_2.img".to_string(),
                target_id: "esp".to_string(),
                format: ImageFormat::RawZst,
                sha256: ImageSha256::Checksum("esp_sha256_2".to_string()),
            }],
            "Incorrectly identified ESP partition as not needing an update"
        );

        // Test case 3: Change PartitionType of ESP partition to swap, so func
        // get_undeployed_esp() should return an empty vector
        host_status
            .spec
            .storage
            .disks
            .iter_mut()
            .find(|d| d.id == "foo")
            .unwrap()
            .partitions
            .get_mut(0)
            .unwrap()
            .partition_type = PartitionType::Swap;
        assert_eq!(
            get_undeployed_images(&host_status, &host_status.spec, false),
            Vec::<&Image>::new(),
            "Incorrectly identified ESP partition as needing an update"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {
    use super::*;
    use std::io::Write;

    use pytest_gen::functional_test;
    use trident_api::constants::GRUB2_RELATIVE_PATH;

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
    #[functional_test(feature = "abupdate")]
    fn test_copy_boot_files_without_noprefix_not_mariner_2_0() {
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
        copy_boot_files(
            temp_mount_dir.path(),
            esp_dir.path(),
            file_names.clone(),
            false,
        )
        .unwrap();

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

    #[functional_test(feature = "abupdate")]
    fn test_copy_boot_files_without_noprefix_mariner_2_0() {
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

        // returns an error if no-prefix is not used on mariner 2.0
        assert!(
            copy_boot_files(
                temp_mount_dir.path(),
                esp_dir.path(),
                file_names.clone(),
                true
            )
            .is_err(),
            "grub-noprefix.efi should be used on mariner 2.0 images for trident"
        );
    }

    /// Validates that copy_boot_files() correctly copies boot files with grub-noprefix from temp_mount_dir to esp_dir
    #[functional_test(feature = "abupdate")]
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
        copy_boot_files(
            temp_mount_dir.path(),
            esp_dir.path(),
            file_names.clone(),
            true,
        )
        .unwrap();

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

        copy_boot_files(
            temp_mount_dir.path(),
            esp_dir.path(),
            file_names.clone(),
            false,
        )
        .unwrap();
        for file_name in file_names {
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
    #[functional_test(feature = "abupdate")]
    fn test_generate_boot_filepaths() {
        // Test case 1: Run generate_boot_filepaths() with GRUB under EFI_DEFAULT_BIN_RELATIVE_PATH
        // Create a temp dir
        let temp_mount_dir = TempDir::new().unwrap();
        // Fetch the path of temp dir
        let temp_mount_path = temp_mount_dir.path();

        // Create a GRUB config inside of the temp dir
        let efi_boot_grub_path = Path::new(temp_mount_path)
            .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
            .join(GRUB2_CONFIG_FILENAME);
        fs::create_dir_all(efi_boot_grub_path.parent().unwrap()).unwrap();
        File::create(&efi_boot_grub_path).unwrap();

        // Create a grub EFI executable inside of the temp dir
        let grub_efi_path = Path::new(temp_mount_path)
            .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
            .join("grubx64.efi");
        File::create(&grub_efi_path).unwrap();

        // Create a boot EFI executable inside of the temp dir
        let boot_efi_path = Path::new(temp_mount_path)
            .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
            .join("bootx64.efi");
        File::create(&boot_efi_path).unwrap();

        let generated_paths_efi_boot = generate_boot_filepaths(temp_mount_path, "x64").unwrap();
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
            generate_boot_filepaths(temp_mount_path, "x64")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find grub.cfg",
            "generate_boot_filepaths() should fail if grub.cfg does not exist"
        );

        // Test case 3: Run generate_boot_filepaths() with GRUB under GRUB2_RELATIVE_PATH
        let boot_grub2_grub_path = Path::new(temp_mount_path)
            .join(GRUB2_RELATIVE_PATH)
            .join(GRUB2_CONFIG_FILENAME);
        fs::create_dir_all(boot_grub2_grub_path.parent().unwrap()).unwrap();
        File::create(&boot_grub2_grub_path).unwrap();

        let generated_paths_boot_grub2 = generate_boot_filepaths(temp_mount_path, "x64").unwrap();
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
            generate_boot_filepaths(temp_mount_path, "x64")
                .unwrap_err()
                .root_cause()
                .to_string(),
            "Failed to find GRUB EFI executable"
        );

        // Test case 5: Run generate_boot_filepaths() with a grub EFI executable with noprefix name
        // Create a grub EFI executable with the noprefix name inside of the temp dir
        let grub_efi_noprefix_path = Path::new(temp_mount_path)
            .join(EFI_DEFAULT_BIN_RELATIVE_PATH)
            .join("grubx64-noprefix.efi");
        File::create(&grub_efi_noprefix_path).unwrap();

        let generated_paths_noprefix = generate_boot_filepaths(temp_mount_path, "x64").unwrap();
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
            generate_boot_filepaths(temp_mount_path, "x64")
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
