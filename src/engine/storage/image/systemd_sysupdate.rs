//! Systemd-sysupdate is a subsystem of the Image subsystem that provides A/B
//! upgrade functionality by using sysupdate, a systemd component. This is v1,
//! which supports the most basic e2e flow:
//! 1. Trident delegates download of the image and update of partition to
//!    systemd-sysupdate. Currently, only partitions of type root and can be
//!    updated; boot can be written to. More info in README.md.
//! 2. Other advanced features are not yet implemented.

// TODO: In a future iteration, systemd-sysupdate.rs needs to be refactored, to
// implement parallel downloads/updates of images with systemd-sysupdate.
// ADO task: https://dev.azure.com/mariner-org/ECF/_workitems/edit/6115.

use std::{
    fs::{self, File},
    io::Read,
    option::Option,
    path::{self, PathBuf},
    process::Command,
};

use anyhow::{bail, Context, Error};
use configparser::ini::Ini;
use log::{debug, info};
use regex::Regex;
use reqwest::Url;
use tempfile;

use osutils::{
    exe::{OutputChecker, RunAndCheck},
    hashing_reader::HashingReader,
    udevadm,
};
use trident_api::{
    config::{Image, ImageSha256, PartitionType},
    status::{AbVolumeSelection, HostStatus},
    BlockDeviceId,
};

use crate::Path;

/// This struct describes an A/B update of a SINGLE image via systemd-sysupdate.
pub(super) struct ImageDeployment {
    /// Id of the partition to be updated.
    pub(super) partition_id_to_update: BlockDeviceId,
    /// New version of the update image; the same as the file name of the update image.
    ///
    /// This value will be set by sysupdate as the new PARTLABEL of the updated partition.
    pub(super) version: String,
    /// TransferConfig struct, whose fields will be parsed to generate a transfer config file for
    /// systemd-sysupdate.
    transfer_config_contents: TransferConfig,
    /// Local path to a temp dir with transfer config file, which sysupdate will be pointed to.
    transfer_config_dir: tempfile::TempDir,
    /// Status of the update. This is set to Pending by default, and will be updated to Succeeded
    /// or Failed after sysupdate is run.
    pub(super) status: Status,
}

/// Enum for the status of ImageDeployment instance.
#[derive(Debug, PartialEq)]
pub(super) enum Status {
    Pending,
    Succeeded,
    Failed,
}

/// This struct is constructed based on data in Image object. It is used to write a transfer
/// definition file for the image deployment instance, to communicate with systemd-sysupdate.
#[derive(Debug)]
struct TransferConfig {
    /// Corresponds to [Transfer] section inside of a transfer config file for systemd-sysupdate.
    transfer: Transfer,
    /// Corresponds to [Source] section inside of a transfer config file for systemd-sysupdate.
    source: Source,
    /// Corresponds to [Target] section inside of a transfer config file for systemd-sysupdate.
    target: Target,
}

/// Corresponds to [Transfer] section inside of a transfer config file for systemd-sysupdate.
#[derive(Debug)]
struct Transfer {
    /// Minimum version of the update image that can be applied.
    min_version: Option<String>,
    /// Version that cannot be removed, or updated.
    protect_version: Option<String>,
    /// Communicates to sysupdate whether the gpg signature of the update image needs to be
    /// verified, along with the image hash.
    verify: bool,
}

/// Defines the two source types that systemd-sysupdate supports: url-file and regular-file.
#[derive(Debug)]
enum SourceType {
    UrlFile,
    RegularFile,
}

impl SourceType {
    /// Returns a string representation of SourceType, following the format of the source type
    /// naming as defined in sysupdate.d.
    fn to_sysupdate_source_type(&self) -> &str {
        match self {
            SourceType::UrlFile => "url-file",
            SourceType::RegularFile => "regular-file",
        }
    }
}

/// Corresponds to [Source] section inside of a transfer config file for systemd-sysupdate.
#[derive(Debug)]
struct Source {
    /// Type of source, either url-file or regular-file.
    type_: SourceType,
    /// Path to the directory containing the update image; could be a remote directory at an
    /// HTTP/HTTPS endpoint (for url-file) or a local temp directory (for regular-file).
    path: PathBuf,
    /// Match pattern for the update image, which is the entire file name of the update image.
    match_pattern: String,
}

/// Corresponds to [Target] section inside of a transfer config file for systemd-sysupdate.
#[derive(Debug)]
struct Target {
    /// Type of target, which is always partition.
    type_: String,
    /// Path to the disk containing the partition to be updated.
    path: PathBuf,
    /// Match pattern for the update image, which is the entire file name of the update image.
    match_pattern: String,
    /// Partition type as a string, e.g. "root", according to the GPT partition type identifiers.
    match_partition_type: PartitionType,
    /// PARTUUID of the partition to be updated; this is set to None so that sysupdate retains the
    /// PARTUUID of updated partition.
    partition_uuid: Option<String>,
    /// Flags of the partition to be updated.
    partition_flags: Option<String>,
    /// Sets no auto flags on partition to be updated.
    partition_no_auto: Option<String>,
    /// Sets grow fs flags on partition to be updated.
    partition_grow_fs: Option<String>,
    /// Whether the partition to be updated is read-only.
    read_only: bool,
}

impl ImageDeployment {
    /// Constructs an instance of ImageDeployment based on the information in Image struct, derived
    /// from HostConfiguration. Accepts TWO optional arg-s:
    /// 1. local_update_dir, which is a local directory containing the update image,
    /// 2. local_update_file, which is a String representing the name of the image file downloaded
    ///    by Trident so that sysupdate can operate on it. This is to handle the case where
    ///    ImageFormat is OciArtifact.
    ///   
    /// Returns an instance of ImageDeployment, or an Error if failed to create one.
    pub(super) fn new(
        update_image: &Image,
        device_id: &BlockDeviceId,
        host_status: &HostStatus,
        local_update_dir: Option<&Path>,
        local_update_file: Option<&str>,
    ) -> Result<Self, Error> {
        // Construct instances of Transfer, Source, and Target
        debug!("Constructing ImageDeployment instance for update of block device with id '{}' to image {}...",
            &device_id, &update_image.url);

        let transfer = Transfer {
            min_version: None,
            protect_version: None,
            // TODO: Set to true once Hermes image pipeline implements signing a .gpg signature.
            // Related ADO task: https://dev.azure.com/mariner-org/ECF/_workitems/edit/5901/.
            verify: false,
        };

        // Determine the directory, filename, and source type to use
        let (update_dir, update_file, source_type) = match (local_update_dir, local_update_file) {
            (Some(dir), Some(file_name)) => {
                // If local_update_dir and local_update_file are provided, we clone dir to get a PathBuf
                // and file_name to get a String
                (
                    PathBuf::from(dir),
                    file_name.to_string(),
                    SourceType::RegularFile,
                )
            }
            _ => {
                // Call filename_dir_from_url once and destructure its result to avoid calling it twice
                // Since filename_dir_from_url returns a PathBuf and a String, we can use them directly
                let (dir_pathbuf, file_name) =
                    filename_dir_from_url(&update_image.url).context(format!(
                        "Failed to extract directory and file name from update image URL: '{}'",
                        &update_image.url
                    ))?;

                // Use dir_pathbuf and file_name directly since they are already of correct types
                (dir_pathbuf, file_name, SourceType::UrlFile)
            }
        };

        let source = Source {
            type_: source_type,
            path: update_dir,
            // Assigns entire file name from SHA256SUMS manifest to be version. Assumes that the
            // user uses consistent formatting so that every next version will be determined by
            // sysupdate to be newer.
            // TODO: Implement down-grades with systemd-sysupdate. Related ADO task:
            // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6126.
            match_pattern: "@v".to_string(),
        };

        // Call get_update_partition_id(), to determine id of partition to update in this A/B
        // update, based on device_id, volume pairs, and active volume.
        let partition_id_to_update =
            get_update_partition_id(host_status, device_id).context(format!(
                "Failed to fetch partition id for update image with device_id '{}'",
                &device_id
            ))?;
        // Fetch block device path of the entire disk that the target partition belongs to, from
        // HostStatus and partition_id_to_update
        let disk_path =
            get_parent_disk_path(host_status, &partition_id_to_update).context(format!(
            "Failed to fetch path of parent disk of partition with id '{partition_id_to_update}'"
        ))?;

        // Fetch partition type from HostStatus based on partition_id_to_update
        let partition_type =
            get_partition_type(host_status, &partition_id_to_update).context(format!(
                "Failed to fetch partition type of partition with id '{partition_id_to_update}'"
            ))?;

        let target = Target {
            type_: "partition".to_string(),
            path: disk_path,
            match_pattern: "@v".to_string(),
            match_partition_type: partition_type,
            // Retain PARTUUID from the old partition
            partition_uuid: None,
            // TODO: Might want to make these configurable for the user.
            // Related ADO task:
            // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6127.
            partition_flags: None,
            partition_no_auto: None,
            partition_grow_fs: None,
            read_only: false,
        };

        // Construct instance of TransferConfig based on newly created Transfer, Source, and Target
        let transfer_config_contents = TransferConfig {
            transfer,
            source,
            target,
        };

        // Create temp directory for writing transfer file for the update
        let transfer_config_dir = tempfile::tempdir()
            .context("Failed to create temporary directory for transfer definitions")?;

        // Construct an instance of ImageDeployment. Status is set to Pending
        let img_deploy_instance = ImageDeployment {
            partition_id_to_update,
            version: update_file.clone(), // Version corresponds to file name
            transfer_config_contents,
            transfer_config_dir,
            status: Status::Pending,
        };
        debug!("Successfully constructed ImageDeployment instance for update of block device with id '{}' to image {}.",
            &device_id, &update_image.url);

        // Call write_transfer_config() to generate a transfer config file and get the path
        let transfer_config_path = img_deploy_instance
            .write_transfer_config()
            .context("Failed to write a transfer config for ImageDeployment instance")?;

        debug!("Successfully wrote transfer config for ImageDeployment instance");

        // Read the contents of the transfer config file
        let transfer_config_contents = std::fs::read_to_string(transfer_config_path)?;
        debug!(
            "Transfer config file contents:\n{}",
            transfer_config_contents
        );
        // Return ImageDeployment instance
        Ok(img_deploy_instance)
    }

    /// Writes Ini-formatted data into a local transfer definition file for systemd-sysupdate.
    fn write_transfer_config(&self) -> Result<PathBuf, Error> {
        // Call transfer_config_to_ini() to create an Ini file
        let ini_data = transfer_config_to_ini(&self.transfer_config_contents)
            .context("Failed to convert TransferConfig to Ini format")?;
        // Construct a full path in TempDir
        let config_file_path = self.transfer_config_dir.path().join("transfer-file.conf");
        // Write the Ini data to the file
        ini_data
            .write(config_file_path.clone())
            .context("Failed to write the Ini data to the transfer file")?;

        Ok(config_file_path)
    }

    /// Takes in an instance of ImageDeployment, runs sysupdate, and returns image_length, a u64
    /// representing the number of bytes acquired by systemd-sysupdate to download an image. This
    /// is to be used for updating HostStatus inside of Image subsystem.
    pub(super) fn run_sysupdate(&mut self, host_status: &HostStatus) -> Result<u64, Error> {
        // Fetch block device path from HostStatus and partition_id_to_update
        let partition_path = get_partition_path(host_status, &self.partition_id_to_update)
            .context(format!(
                "Failed to fetch path of partition with id '{}'",
                &self.partition_id_to_update
            ))?;
        // Fetch current part label of partition; this is its current version
        let old_partlabel = get_partlabel_from_path(&partition_path).context(format!(
            "Failed to fetch PARTLABEL for partition with path '{partition_path}'"
        ))?;

        // Call sysupdate_update() to trigger update
        debug!(
            "Triggering update of partition with id '{}' and path {} from version '{}' to version '{}' with sysupdate",
            &self.partition_id_to_update, &partition_path, &old_partlabel, &self.version
        );

        match self.sysupdate_update() {
            Err(e) => {
                // If update failed, set status to Failed
                self.status = Status::Failed;
                Err(e.context(format!("Failed to update partition with id '{}' and path '{}' from version '{}' to version '{}' with sysupdate",
                    &self.partition_id_to_update, &partition_path, &old_partlabel, &self.version
                )))
            }
            Ok(image_length) => {
                // Double check that update succeeded by verifying that
                // PARTLABEL of updated partition is now the requested version
                udevadm::settle()?;
                let actual_partlabel =
                    get_partlabel_from_path(&partition_path).context(format!(
                        "Failed to verify PARTLABEL for updated partition with id '{}'",
                        &self.partition_id_to_update
                    ))?;
                if actual_partlabel != self.version {
                    bail!(
                        "Success reported by sysupdate, but verification failed. Expected partition with id '{}' to have PARTLABEL '{}' but current PARTLABEL is set to '{}'",
                        &self.partition_id_to_update, &self.version, &actual_partlabel
                    );
                }
                debug!(
                    "PARTLABEL of updated partition with id '{}' successfully updated from '{}' to '{}'",
                    &self.partition_id_to_update, &old_partlabel, &self.version
                );

                debug!(
                    "Update of partition with id '{}' from version '{}' to version '{}' with sysupdate succeeded",
                    &self.partition_id_to_update, &old_partlabel, &self.version
                );

                // TODO: Generate a random UUID for the updated partition, so
                // that we can correctly differentiate between A and B root
                // partitions, e.g. for GRUB config.
                // Related ADO task:
                // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6169.

                // If update succeeded, set status to Succeeded
                self.status = Status::Succeeded;

                // Return the number of bytes acquired by systemd-sysupdate
                Ok(image_length)
            }
        }
    }

    // TODO: Need to write a tester for this function. Related ADO task:
    // https://dev.azure.com/mariner-org/ECF/_workitems/edit/6128.

    /// Triggers an update with systemd-sysupdate. Returns the number of bytes that sysupdate
    /// downloaded, to be used for updating HostStatus inside of Image subsystem.
    fn sysupdate_update(&self) -> Result<u64, Error> {
        // Run systemd-sysupdate update [VERSION] command, with option --definitions set to dir
        // where transfer config file is located
        info!("Running systemd-sysupdate...");

        let res = Command::new("/lib/systemd/systemd-sysupdate")
            .arg("update")
            .arg(&self.version)
            .arg("--definitions")
            .arg(self.transfer_config_dir.path())
            .raw_output_and_check()
            .context(format!(
                "Failed to run systemd-sysupdate to version {}, with config definition files in directory {}",
                self.version,
                self.transfer_config_dir.path().display()
            )
        )?;

        // Get stderr output of sysupdate
        let stderr_str = res.error_output();

        // TODO: Implement live-streaming of systemd-sysupdate logs to the orchestrator in Trident.
        // Related ADO task: https://dev.azure.com/mariner-org/ECF/_workitems/edit/6177.

        info!("Systemd-Sysupdate succeeded");

        // If type of Source is a regular-file, compute the num of bytes
        // since it is a local file; otherwise, call extract_image_length()
        // to parse the output of sysupdate
        match self.transfer_config_contents.source.type_ {
            SourceType::RegularFile => {
                // Re-construct the full path of the update image
                let update_image_path =
                    Path::new(&self.transfer_config_contents.source.path).join(&self.version);
                // Call compute_image_length() to compute num of bytes
                Ok(compute_image_length(&update_image_path).context(format!(
                    "Failed to compute length of image {}",
                    self.version
                ))?)
            }
            SourceType::UrlFile => {
                // Call extract_image_length() to parse the output of sysupdate
                Ok(extract_image_length(&stderr_str).context(format!(
                    "Failed to extract length of image {}",
                    self.version
                ))?)
            }
        }
    }
}

/// Extracts the number of bytes acquired by sysupdate from its output.
fn extract_image_length(stderr_str: &str) -> Result<u64, Error> {
    let re = Regex::new(r"Acquired (\d+\.?\d*)\s*([KMG]B?)").context("Failed to compile regex")?;
    let cap = re
        .captures(stderr_str)
        .context("Failed to parse sysupdate output to extract image length")?;

    let value = cap[1]
        .parse::<f64>()
        .context("Failed to parse the number from regex capture")?;
    let unit = &cap[2];

    match unit {
        "B" => Ok(value as u64),
        "K" | "KB" => Ok((value * 1024.0) as u64),
        "M" | "MB" => Ok((value * 1024.0 * 1024.0) as u64),
        "G" | "GB" => Ok((value * 1024.0 * 1024.0 * 1024.0) as u64),
        _ => bail!(
            "Extracted unrecognized unit {}. Failed to parse image length from sysupdate output",
            unit
        ),
    }
}

/// Computes the number of bytes of update image based on its path.
// TODO: Reports the length of the uncomressed local raw lzma file, while the field length in
// HostStatus is the length of the compressed image. Need to fix this in a future iteration.
// Related ADO task: https://dev.azure.com/mariner-org/ECF/_workitems/edit/6209.
fn compute_image_length(image_path: &Path) -> Result<u64, Error> {
    // Fetch num of bytes of image
    let file = File::open(image_path).context(format!(
        "Failed to open file with path '{}'",
        image_path.display()
    ))?;
    let metadata = file.metadata().context(format!(
        "Failed to fetch metadata for file with path '{}'",
        image_path.display()
    ))?;
    Ok(metadata.len())
}

/// Returns a string representation of the block device path of partition, based on partition id.
fn get_partition_path(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
) -> Result<String, Error> {
    // Fetch BlockDeviceInfo of partition based on its id
    let part_block_device_path = host_status
        .block_device_paths
        .get(block_device_id)
        .context(format!("No partition with id '{block_device_id}' found"))?;

    // Ensure block device is a partition
    let _ = host_status
        .spec
        .storage
        .get_partition(block_device_id)
        .context(format!(
            "Block device with id '{block_device_id}' is not a partition"
        ))?;

    // Fetch partition path and convert to string
    let partition_path = part_block_device_path.to_str().context(format!(
        "Failed to convert partition path '{}' to string",
        part_block_device_path.display()
    ))?;

    Ok(partition_path.to_string())
}

/// Returns a string representation of the block device path of the parent disk of the partition,
/// based on its id.
fn get_parent_disk_path(
    host_status: &HostStatus,
    block_device_id: &BlockDeviceId,
) -> Result<PathBuf, Error> {
    // Fetch block device path of the full disk, i.e. parent of partition
    let parent_disk = get_parent_disk(host_status, block_device_id).context(format!(
        "Failed to fetch parent disk for partition with id {block_device_id}"
    ))?;

    Ok(parent_disk)
}

/// Returns the path of the parent disk of partition, based on its id.
fn get_parent_disk(host_status: &HostStatus, partition_id: &BlockDeviceId) -> Option<PathBuf> {
    // Iterate over all the disks in host_status
    for disk in host_status.spec.storage.disks.iter() {
        // Iterate over the partitions of the disk
        for partition in &disk.partitions {
            // Check if the partition id matches the given BlockDeviceId
            if &partition.id == partition_id {
                return host_status.block_device_paths.get(&disk.id).cloned();
            }
        }
    }
    // If not found, return None
    None
}

/// Returns PartitionType of partition, based on its id.
fn get_partition_type(
    host_status: &HostStatus,
    partition_id: &str,
) -> Result<PartitionType, Error> {
    // Iterate through all disks and partitions
    for disk in host_status.spec.storage.disks.iter() {
        for partition in &disk.partitions {
            if partition.id == partition_id {
                // Directly return the type of partition
                return Ok(partition.partition_type);
            }
        }
    }
    bail!("Failed to find partition with id '{}'", &partition_id);
}

/// Returns the id of partition to be updated, based on device_id provided in HC. Assumes that
/// device_id corresponds to a valid partition inside of an A/B volume pair, b/c func
/// stream_images() in image/mod.rs already verifies that.
fn get_update_partition_id(
    host_status: &HostStatus,
    device_id: &BlockDeviceId,
) -> Result<BlockDeviceId, Error> {
    // Iterate through storage.ab-update.volume-pairs and return the correct volume-id, i.e. id of
    // partition to be updated; when ServicingType is AbUpdate, get_ab_update_volume() already returns
    // the inactive AbVolumeSelection, i.e. the one to be updated
    if let Some(ab_update) = &host_status.spec.storage.ab_update {
        // Call helper func from lib.rs, which returns AbVolumeSelection to be updated in this A/B
        // update, either VolumeA or VolumeB, depending on which volume is active now
        let volume_selection: AbVolumeSelection = host_status
            .get_ab_update_volume()
            .context("Failed to determine which A/B volume is currently inactive")?;
        // Fetch volume pair for the device_id
        if let Some(volume_pair) = ab_update.volume_pairs.iter().find(|p| &p.id == device_id) {
            return match volume_selection {
                AbVolumeSelection::VolumeA => Ok(volume_pair.volume_a_id.clone()),
                AbVolumeSelection::VolumeB => Ok(volume_pair.volume_b_id.clone()),
            };
        }
    }
    // If there is no volume pair for the device_id OR if ab-update is None, it means that we are
    // using systemd_sysupdate.rs to clean-install the runtime OS image; return device_id itself.
    Ok(device_id.clone())
}

/// Returns string representations of directory path and file name from URL of update image.
fn filename_dir_from_url(image_url: &str) -> Result<(PathBuf, String), Error> {
    // Parse URL into Url struct
    let parsed_url = Url::parse(image_url).context(format!(
        "Failed to parse image URL '{image_url}' provided for sysupdate"
    ))?;
    // Split URL into path segments and collect them into a vector
    let mut segments: Vec<String> = parsed_url
        .path_segments()
        .context(format!(
            "Failed to retrieve path segments from image URL '{}'",
            &image_url
        ))?
        .map(|s| s.to_string()) // Transform each segment into a String
        .collect(); // Collect all segments into a Vec<String>
                    // If there is a valid last segment, save it as file name
    let file_name = segments.pop().context(format!(
        "Image URL '{image_url}' does not contain any segments"
    ))?;

    // Rebuild the URL without the file name segment
    let mut url_without_file = parsed_url.clone();
    url_without_file.set_path(&segments.join(path::MAIN_SEPARATOR_STR));
    let dir_path = PathBuf::from(url_without_file.to_string());

    Ok((dir_path, file_name))
}

/// Writes TransferConfig instance to Ini file. Returns an Ini object.
fn transfer_config_to_ini(config: &TransferConfig) -> Result<Ini, Error> {
    let mut transfer_config = Ini::new_cs();
    let section_transfer = "Transfer";
    // Only add field to Ini file if value other than None
    if let Some(min_version) = &config.transfer.min_version {
        transfer_config.set(
            section_transfer,
            "MinVersion",
            Some(min_version.to_string()),
        );
    }
    if let Some(protect_version) = &config.transfer.protect_version {
        transfer_config.set(
            section_transfer,
            "ProtectVersion",
            Some(protect_version.to_string()),
        );
    }
    let verify_str = if config.transfer.verify { "yes" } else { "no" };
    transfer_config.set(section_transfer, "Verify", Some(verify_str.to_string()));

    let section_source = "Source";
    transfer_config.set(
        section_source,
        "Type",
        Some(config.source.type_.to_sysupdate_source_type().to_string()),
    );
    transfer_config.set(
        section_source,
        "Path",
        Some(config.source.path.display().to_string()),
    );
    transfer_config.set(
        section_source,
        "MatchPattern",
        Some(config.source.match_pattern.clone()),
    );
    let section_target = "Target";
    transfer_config.set(section_target, "Type", Some(config.target.type_.clone()));
    transfer_config.set(
        section_target,
        "Path",
        Some(config.target.path.display().to_string()),
    );
    transfer_config.set(
        section_target,
        "MatchPattern",
        Some(config.target.match_pattern.clone()),
    );
    transfer_config.set(
        section_target,
        "MatchPartitionType",
        Some(
            config
                .target
                .match_partition_type
                .to_sdrepart_part_type()
                .to_string(),
        ),
    );
    if let Some(partition_uuid) = &config.target.partition_uuid {
        transfer_config.set(
            section_target,
            "PartitionUUID",
            Some(partition_uuid.to_string()),
        );
    }
    if let Some(partition_flags) = &config.target.partition_flags {
        transfer_config.set(
            section_target,
            "PartitionFlags",
            Some(partition_flags.to_string()),
        );
    }
    if let Some(partition_no_auto) = &config.target.partition_no_auto {
        transfer_config.set(
            section_target,
            "PartitionNoAuto",
            Some(partition_no_auto.to_string()),
        );
    }
    if let Some(partition_grow_fs) = &config.target.partition_grow_fs {
        transfer_config.set(
            section_target,
            "PartitionGrowFileSystem",
            Some(partition_grow_fs.to_string()),
        );
    }
    let read_only_str = if config.target.read_only { "yes" } else { "no" };
    transfer_config.set(section_target, "ReadOnly", Some(read_only_str.to_string()));
    Ok(transfer_config)
}

// TODO: Need to write a tester for this function, post this PR! Related ADO task:
// https://dev.azure.com/mariner-org/ECF/_workitems/edit/6130.
//
/// Returns PARTLABEL of partition as a string based on path to the partition, in the canonicalized
/// format.
fn get_partlabel_from_path(partition_path: &str) -> Result<String, Error> {
    // Canonicalize the path
    let canonical_path = fs::canonicalize(partition_path)
        .with_context(|| format!("Failed to canonicalize the path '{partition_path}'"))?;
    // Run the blkid command to fetch block devices
    let output = Command::new("blkid")
        .arg("-o")
        .arg("value") // Entire file name is [VERSION] option
        .arg("-s")
        .arg("PARTLABEL")
        .arg(&canonical_path)
        .output_and_check()
        .context(format!(
            "Failed to fetch PARTLABEL for partition with path '{partition_path}'"
        ))?;

    // Trim the output and return
    if !output.trim().is_empty() {
        return Ok(output.trim().to_string());
    }

    // Return an Error otherwise
    bail!("No PARTLABEL found on block device '{}'", &partition_path)
}

/// Open & process a local image file
///
/// Takes in a URL to a local image, and an Image struct. Attempts to open the
/// image, checks its sha256 checksum.
///
/// Returns a tuple of (directory, filename, computed sha256), where directory
/// is a PathBuf, filename is a String, and sha256 is a String.
pub(super) fn get_local_image(
    image_url: &Url,
    image: &Image,
) -> Result<(PathBuf, String, String), Error> {
    // Open local image file and read it into a stream of bytes
    let stream: Box<dyn Read> =
        Box::new(File::open(image_url.path()).context(format!("Failed to open {}", image.url))?);

    // Use HashingReader to compute sha256 hash of stream
    let stream = HashingReader::new(stream);
    let computed_sha256 = stream.hash();

    // If SHA256 is ignored, log message and skip hash validation; otherwise,
    // ensure it is the same as hash in HostConfig
    match image.sha256 {
        ImageSha256::Ignored => {
            info!("Ignoring SHA256 for image from '{}'", image.url);
        }
        ImageSha256::Checksum(ref expected_sha256) => {
            if computed_sha256 != *expected_sha256 {
                bail!(
                    "SHA256 mismatch for disk image {}: expected {}, got {}",
                    image.url,
                    image.sha256,
                    stream.hash()
                );
            }
        }
    }

    // Convert image URL to file path
    let path = image_url
        .to_file_path()
        .map_err(|_| anyhow::anyhow!("Failed to convert URL to file path"))?;

    // Extract directory from URL path
    let directory = path.parent().context(format!(
        "Failed to extract local dir from URL path {}",
        path.display()
    ))?;

    // Extract filename as String from URL path
    let filename = path
        .file_name()
        .map(|f| f.to_string_lossy())
        .context(format!(
            "Failed to extract filename from URL path {}",
            path.display()
        ))?;

    Ok((directory.to_owned(), filename.to_string(), computed_sha256))
}

/// Call into systemd-sysupdate to update the partition with the given image.
pub(super) fn deploy(
    image: &Image,
    device_id: &BlockDeviceId,
    host_status: &HostStatus,
    directory: Option<&Path>,
    filename: Option<&str>,
) -> Result<(), Error> {
    debug!("Calling Systemd-Sysupdate subsystem to execute A/B update");
    // Create ImageDeployment instance
    let mut img_deploy_instance =
        ImageDeployment::new(image, device_id, host_status, directory, filename).context(
            format!(
                "Failed to create ImageDeployment instance for block device with id '{}'",
                &device_id
            ),
        )?;
    // Call run_sysupdate(); save return value as number of bytes written
    let image_length = img_deploy_instance
        .run_sysupdate(host_status)
        .context(format!(
        "Failed to run systemd-sysupdate: Failed to update partition with id '{}' to version '{}'.",
        &img_deploy_instance.partition_id_to_update, &img_deploy_instance.version
    ))?;
    // If A/B update succeeded, update HostStatus
    if image_length > 0 && img_deploy_instance.status == Status::Succeeded {
        info!(
            "Systemd-Sysupdate subsystem successfully updated partition with id '{}' to version '{}'",
            &img_deploy_instance.partition_id_to_update, &img_deploy_instance.version
        );
    } else {
        // If image_length is not 0 or status is Failed, A/B update failed
        bail!("Update of partition with id '{}' to version '{}' failed. Returned image_length: {}; returned status: {:?}.",
            &img_deploy_instance.partition_id_to_update, &img_deploy_instance.version, image_length, &img_deploy_instance.status
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    // Import everything from the parent module
    use super::*;

    use maplit::btreemap;

    use trident_api::{
        config::{self, AbUpdate, AbVolumePair, HostConfiguration},
        status::{ServicingState, ServicingType},
    };

    /// Validates that filename_dir_from_url() parses image URL correctly.
    #[test]
    fn test_filename_dir_from_url() {
        let url =
            "https://hermesstorageacc.blob.core.windows.net/hermes-container/test_v2/root_v2.raw.xz";
        let result = filename_dir_from_url(url).unwrap();
        assert_eq!(
            result,
            (
                PathBuf::from(
                    "https://hermesstorageacc.blob.core.windows.net/hermes-container/test_v2"
                ),
                "root_v2.raw.xz".to_string()
            )
        );
    }

    #[test]
    /// Validates that write_transfer_config() correctly writes a transfer config file to an Ini
    /// file for sysupdate.
    fn test_write_transfer_config() {
        // Create an instance of TransferConfig with hard-coded values
        let transfer_config_contents = TransferConfig {
            transfer: Transfer {
                verify: false,
                min_version: None,
                protect_version: None,
            },
            source: Source {
                type_: SourceType::UrlFile,
                path: PathBuf::from(
                    "https://hermesstorageacc.blob.core.windows.net/hermes-container/test_v2",
                ),
                match_pattern: "@v".to_string(),
            },
            target: Target {
                type_: "partition".to_string(),
                path: PathBuf::from("/dev/sdc"),
                match_pattern: "@v".to_string(),
                match_partition_type: PartitionType::Root,
                partition_uuid: Some("3bc72925-f3c8-4473-a803-624415e08c00".to_string()),
                partition_flags: None,
                partition_no_auto: None,
                partition_grow_fs: None,
                read_only: false,
            },
        };
        // Create a temporary directory for the test
        let transfer_config_dir = tempfile::tempdir().unwrap();
        // Construct an instance of ImageDeployment. Status first set to Pending
        let img_deploy_instance = ImageDeployment {
            partition_id_to_update: "root".to_string(),
            version: "root_v2.raw.xz".to_string(),
            transfer_config_contents,
            transfer_config_dir,
            status: Status::Pending,
        };
        // Call write_transfer_config() to generate a transfer config file
        img_deploy_instance.write_transfer_config().unwrap();
        // Compare the contents of the written file with expected string
        let written_contents = fs::read_to_string(
            img_deploy_instance
                .transfer_config_dir
                .path()
                .join("transfer-file.conf"),
        )
        .unwrap();
        let expected_contents = "[Transfer]\nVerify=no\n[Source]\nType=url-file\nPath=https://hermesstorageacc.blob.core.windows.net/hermes-container/test_v2\nMatchPattern=@v\n[Target]\nType=partition\nPath=/dev/sdc\nMatchPattern=@v\nMatchPartitionType=root\nPartitionUUID=3bc72925-f3c8-4473-a803-624415e08c00\nReadOnly=no\n";

        assert_eq!(written_contents, expected_contents);
    }

    #[test]
    /// Validates that transfer_config_to_ini() correctly converts a TransferConfig into an Ini
    /// object.
    fn test_transfer_config_to_ini() {
        // Setup a mock TransferConfig
        let transfer_config = TransferConfig {
            transfer: Transfer {
                verify: false,
                min_version: None,
                protect_version: None,
            },
            source: Source {
                type_: SourceType::UrlFile,
                path: PathBuf::from(
                    "https://hermesstorageacc.blob.core.windows.net/hermes-container/test_v2",
                ),
                match_pattern: "@v".to_string(),
            },
            target: Target {
                type_: "partition".to_string(),
                path: PathBuf::from("/dev/sdc"),
                match_pattern: "@v".to_string(),
                match_partition_type: PartitionType::Root,
                partition_uuid: Some("3bc72925-f3c8-4473-a803-624415e08c00".to_string()),
                partition_flags: None,
                partition_no_auto: None,
                partition_grow_fs: None,
                read_only: false,
            },
        };

        let result = transfer_config_to_ini(&transfer_config).unwrap();
        let generated_content = result.writes();
        let expected_contents = "[Transfer]\nVerify=no\n[Source]\nType=url-file\nPath=https://hermesstorageacc.blob.core.windows.net/hermes-container/test_v2\nMatchPattern=@v\n[Target]\nType=partition\nPath=/dev/sdc\nMatchPattern=@v\nMatchPartitionType=root\nPartitionUUID=3bc72925-f3c8-4473-a803-624415e08c00\nReadOnly=no\n";

        assert_eq!(expected_contents, generated_content);
    }

    #[test]
    /// Validates that get_update_partition_id() correctly returns the id of the partition that is
    /// inactive, or to be updated, based on device_id.
    fn test_get_update_partition_id() {
        let mut host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![config::Disk {
                        id: "os".to_owned(),
                        partitions: vec![
                            config::Partition {
                                id: "efi".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: 1000.into(),
                            },
                            config::Partition {
                                id: "root".to_owned(),
                                partition_type: PartitionType::Root,
                                size: 1000.into(),
                            },
                            config::Partition {
                                id: "rootb".to_owned(),
                                partition_type: PartitionType::Root,
                                size: 1000.into(),
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
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "rootb".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "data".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
            },
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,
            ..Default::default()
        };

        host_status.servicing_type = ServicingType::AbUpdate;
        host_status.ab_active_volume = Some(AbVolumeSelection::VolumeA);

        // Scenario 1: Target ID matches with an entry and active volume is VolumeA
        match get_update_partition_id(&host_status, &"osab".to_string()) {
            Ok(volume_id) => assert_eq!(volume_id, "rootb"),
            Err(e) => panic!("Unexpected error: {}", e),
        }

        // Scenario 2: Target ID is a partition that is not part of any volume pair but has been
        // verified to exist in image/mod.rs and needs to be written to by systemd-sysupdate
        match get_update_partition_id(&host_status, &"efi".to_string()) {
            Ok(volume_id) => assert_eq!(volume_id, "efi"),
            Err(e) => panic!("Unexpected error: {}", e),
        }

        // Scenario 3: Switch active-volume to VolumeB and verify
        host_status.ab_active_volume = Some(AbVolumeSelection::VolumeB);
        match get_update_partition_id(&host_status, &"osab".to_string()) {
            Ok(volume_id) => assert_eq!(volume_id, "root"),
            Err(e) => panic!("Unexpected error: {}", e),
        }

        let host_status2 = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![config::Disk {
                        id: "os".to_owned(),
                        partitions: vec![
                            config::Partition {
                                id: "efi".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: 1000.into(),
                            },
                            config::Partition {
                                id: "root".to_owned(),
                                partition_type: PartitionType::Root,
                                size: 1000.into(),
                            },
                        ],
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "data".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
            },
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,
            ..Default::default()
        };

        // Scenario 4: Set ab-update to None; this means that we are using systemd-sysupdate to
        // write to a partition in clean-install, so should return target-id itself
        match get_update_partition_id(&host_status2, &"root".to_string()) {
            Ok(volume_id) => assert_eq!(volume_id, "root"),
            Err(e) => panic!("Unexpected error: {}", e),
        }
    }

    /// Validates that get_partition_type() correctly fetches the type of the partition from
    /// HostStatus, based on its id.
    #[test]
    fn test_get_partition_type() {
        let host_status = HostStatus {
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![config::Disk {
                        id: "os".to_owned(),
                        partitions: vec![
                            config::Partition {
                                id: "efi".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: 1000.into(),
                            },
                            config::Partition {
                                id: "root".to_owned(),
                                partition_type: PartitionType::Root,
                                size: 1000.into(),
                            },
                            config::Partition {
                                id: "rootb".to_owned(),
                                partition_type: PartitionType::Root,
                                size: 1000.into(),
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
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "rootb".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "data".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
            },
            ab_active_volume: Some(AbVolumeSelection::VolumeA),
            ..Default::default()
        };

        // Scenario 1: Successfully get partition type for a given id
        match get_partition_type(&host_status, "efi") {
            Ok(type_str) => assert_eq!(type_str, PartitionType::Esp),
            Err(e) => panic!("Unexpected error: {}", e),
        }

        match get_partition_type(&host_status, "rootb") {
            Ok(type_str) => assert_eq!(type_str, PartitionType::Root),
            Err(e) => panic!("Unexpected error: {}", e),
        }

        // Scenario 2: Failed to get type for non-existent partition id
        assert!(get_partition_type(&host_status, "invalid_id").is_err());

        // Scenario 3: No partitions available
        let host_status2 = HostStatus {
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![
                        config::Disk {
                            id: "os".to_owned(),
                            partitions: vec![],
                            ..Default::default()
                        },
                        config::Disk {
                            id: "data".to_owned(),
                            partitions: vec![],
                            ..Default::default()
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "data".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
            },
            ..Default::default()
        };
        assert!(get_partition_type(&host_status2, "root").is_err());
    }

    /// Validates that get_parent_disk() correctly identifies parent disk of partition, given
    /// that it is valid.
    #[test]
    fn test_get_parent_disk() {
        let host_status = HostStatus {
            servicing_type: ServicingType::CleanInstall,
            servicing_state: ServicingState::Staging,
            spec: HostConfiguration {
                storage: config::Storage {
                    disks: vec![config::Disk {
                        id: "os".to_owned(),
                        partitions: vec![
                            config::Partition {
                                id: "efi".to_owned(),
                                partition_type: PartitionType::Esp,
                                size: 1000.into(),
                            },
                            config::Partition {
                                id: "root".to_owned(),
                                partition_type: PartitionType::Root,
                                size: 1000.into(),
                            },
                            config::Partition {
                                id: "rootb".to_owned(),
                                partition_type: PartitionType::Root,
                                size: 1000.into(),
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
            block_device_paths: btreemap! {
                "os".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
                "efi".into() => PathBuf::from("/dev/disk/by-partlabel/osp1"),
                "root".into() => PathBuf::from("/dev/disk/by-partlabel/osp2"),
                "rootb".into() => PathBuf::from("/dev/disk/by-partlabel/osp3"),
                "data".into() => PathBuf::from("/dev/disk/by-bus/foobar"),
            },
            ..Default::default()
        };

        // Case 1: Partition ID is valid
        assert_eq!(
            &get_parent_disk(&host_status, &"root".to_string()).unwrap(),
            host_status.block_device_paths.get("os").unwrap()
        );
        // Case 2: Partition ID is invalid
        assert_eq!(get_parent_disk(&host_status, &"invalid".to_string()), None);
    }
}
