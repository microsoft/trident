use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, ensure, Context, Error};
use log::{debug, trace, warn};

use osutils::{
    block_devices,
    dependencies::Dependency,
    filesystems::MountFileSystemType,
    mount::{self, MountGuard},
    veritysetup::{self, VerityDevice as VerityDeviceUtils},
};
use tempfile::NamedTempFile;
use trident_api::{
    config::{HostConfiguration, VerityDevice},
    constants::{
        internal_params::VERITY_SIGNATURE_PATHS, DEV_MAPPER_PATH, ROOT_VERITY_DEVICE_NAME,
        USR_MOUNT_POINT_PATH, USR_VERITY_DEVICE_NAME,
    },
    BlockDeviceId,
};

use crate::engine::{
    storage::common::{self, SetRelationship},
    EngineContext,
};

use super::raid;

pub(crate) fn get_updated_device_name(device_name: &str) -> String {
    format!("{device_name}_new")
}

/// Get the root-verity root hash.
fn get_root_verity_root_hash(ctx: &EngineContext) -> Result<String, Error> {
    // Extract information from the OS image.
    let Some(os_img) = ctx.image.as_ref() else {
        bail!("Image is not available");
    };

    trace!("Getting root-verity root hash from OS image");
    let root_fs = os_img
        .root_filesystem()
        .context("Failed to get root filesystem from OS image")?;

    let Some(verity) = root_fs.verity.as_ref() else {
        bail!("Root filesystem in OS image is not verity enabled");
    };

    Ok(verity.roothash.clone())
}

/// Gets the usr-verity root hash.
fn get_usr_verity_root_hash(ctx: &EngineContext) -> Result<String, Error> {
    // Extract information from the OS image.
    let Some(os_img) = ctx.image.as_ref() else {
        bail!("Image is not available");
    };

    trace!("Getting usr-verity root hash from OS image");
    let usr_fs = os_img
        .filesystems()
        .find(|fs| fs.mount_point == Path::new(USR_MOUNT_POINT_PATH))
        .context("Failed to get usr filesystem from OS image")?;

    let Some(verity) = usr_fs.verity.as_ref() else {
        bail!("usr filesystem in OS image is not verity enabled");
    };

    Ok(verity.roothash.clone())
}

/// Setup verity devices.
///
/// Assumes that images are already in place (data and hash), so that it can
/// assemble the verity devices.
#[tracing::instrument(skip_all)]
pub(super) fn setup_verity_devices(ctx: &EngineContext) -> Result<(), Error> {
    // Validated from API there is only ONE verity device
    let Some(verity_device) = ctx.spec.storage.verity.first() else {
        return Ok(());
    };

    // Get the verity data and hash device paths from the engine context
    let (data_dev, hash_dev) = get_verity_device_paths(ctx, verity_device)?;
    let update_name = get_updated_device_name(&verity_device.name);

    let root_hash = if ctx.storage_graph.root_fs_is_verity() {
        debug!(
            "Setting up verity device '{}' for root filesystem",
            verity_device.id
        );

        get_root_verity_root_hash(ctx)?
    } else if ctx.storage_graph.usr_fs_is_verity() {
        debug!(
            "Setting up verity device '{}' for usr filesystem",
            verity_device.id
        );

        get_usr_verity_root_hash(ctx)?
    } else {
        bail!(
            "Verity device '{}' is not on a supported filesystem.",
            verity_device.name
        );
    };

    // Create the internal representation of the verity device.
    let verity_dev = VerityDeviceUtils::new(update_name, data_dev, hash_dev, root_hash);

    // Prefer the declarative signature partition approach, if a signature
    // device is configured on the verity device.
    if let Some(hash_signature_device_id) = verity_device.hash_signature_device_id.as_ref() {
        return open_verity_device_with_signature_partition(
            ctx,
            &verity_device.id,
            verity_dev,
            hash_signature_device_id,
        )
        .with_context(|| {
            format!(
                "Failed to open verity device '{}' with signature partition '{}'",
                verity_device.id, hash_signature_device_id
            )
        });
    }

    // Check internal parameters for verity signatures.
    if let Some(signature_file_map) = ctx
        .spec
        .internal_params
        .get::<HashMap<BlockDeviceId, PathBuf>>(VERITY_SIGNATURE_PATHS)
        .transpose()?
    {
        if let Some(signature_file_path) = signature_file_map.get(&verity_device.id) {
            // If we have valid internal params and one signature file path matching this block device ID,
            // open the verity device with the signature file and return.
            return open_verity_device_with_signature(
                ctx,
                &verity_device.id,
                verity_dev,
                signature_file_path,
            )
            .with_context(|| {
                format!(
                    "Failed to open verity device '{}' with signature file '{}'",
                    verity_device.id,
                    signature_file_path.display()
                )
            });
        }
    }

    // Otherwise, open and return normally.
    debug!("Opening verity device '{}'", verity_device.id);
    verity_dev.open()
}

/// Open a verity device with a signature file.
///
/// ONLY MEANT FOR USE DURING OS UPDATES.
///
/// The signature is expected to be a file contained inside of the update image.
/// It may be located in any filesystem except for the ESP and the verity
/// filesystem itself (as that would be impossible). However, placing it on a
/// standalone filesystem mounted at `/boot` is recommended. (And so far the
/// only tested location.)
///
/// The signature is expected to exist in der format.
///
/// Internally, this function will figure out where the signature file is
/// located, mount that filesystem, copy the file out into a temporary file, and
/// then use `veritysetup open --root-hash-signature=<signature_file_path>` to
/// open the verity device. The function will only succeed if the device was
/// successfully opened and the signature validated.
///
/// The certificate matching the signature MUST exist in the kernel keyring,
/// otherwise the operation WILL fail.
///
/// As a small aid, information about the signature file will be printed to the
/// debug log.
fn open_verity_device_with_signature(
    ctx: &EngineContext,
    verity_device_id: &BlockDeviceId,
    verity_device: VerityDeviceUtils,
    signature_file_path: &Path,
) -> Result<(), Error> {
    debug!(
        "Preparing to open verity device '{}' with signature file '{}'",
        verity_device_id,
        signature_file_path.display()
    );

    // ESP is populated after this point, so we cannot allow signature files to
    // be on the ESP mount point.
    ensure!(
        !signature_file_path.starts_with(ctx.esp_mount_path.as_path()),
        "Signature file cannot be on the ESP mount point '{}'",
        ctx.esp_mount_path.as_path().display()
    );

    let (mpi, relative_path) = ctx
        .spec
        .storage
        .get_mount_point_info_and_relative_path(signature_file_path)
        .context("Could not find a mount point and relative path for the signature file.")?;

    let signature_block_device_id = mpi.device_id.with_context(|| {
        format!(
            "The mount point '{}' is not placed on a real block device.",
            mpi.mount_point.path.display()
        )
    })?;
    let signature_block_device_path = ctx
        .get_block_device_path(signature_block_device_id)
        .with_context(|| {
            format!("Failed to find path for block device '{signature_block_device_id}'")
        })?;

    // Create a temporary file to hold a copy of the signature file.
    let temp_signature_file_path = NamedTempFile::new()
        .context("Failed to create temporary file for verity signature")?
        .into_temp_path();

    // Create a temporary directory to mount the signature block device.
    let signature_mount_dir =
        tempfile::tempdir().context("Failed to create temporary directory for verity device")?;

    debug!(
        "Mounting signature block device '{}' [{}] at temporary directory '{}'",
        signature_block_device_id,
        signature_block_device_path.display(),
        signature_mount_dir.path().display()
    );

    // Mount the signature block device at the temporary directory.
    mount::mount(
        &signature_block_device_path,
        &signature_mount_dir,
        MountFileSystemType::Auto,
        &[],
    )
    .context(format!(
        "Failed to mount signature block device '{}' at temporary directory '{}'",
        signature_block_device_path.display(),
        signature_mount_dir.path().display()
    ))?;

    // IMMEDIATELY after mounting create a new scope with a MountGuard to ensure
    // the temporary mount is unmounted when we leave this scope.
    {
        let _guard = MountGuard {
            mount_dir: signature_mount_dir.path(),
        };

        // This will be the path we can find the signature file at after mounting.
        let effective_signature_file_path = signature_mount_dir.path().join(relative_path);

        ensure!(
            effective_signature_file_path.exists(),
            "Signature file does not exist at expected path '{}'",
            effective_signature_file_path.display(),
        );

        let copied = fs::copy(&effective_signature_file_path, &temp_signature_file_path)
            .with_context(|| {
                format!(
                    "Failed to copy signature file '{}' to temporary file '{}'",
                    effective_signature_file_path.display(),
                    temp_signature_file_path.display(),
                )
            })?;

        debug!(
            "Copied signature file '{}' to temporary file '{}' ({} bytes)",
            effective_signature_file_path.display(),
            temp_signature_file_path.display(),
            copied,
        );
    }

    // Try to print signature info
    match veritysetup::get_verity_signature_info(&temp_signature_file_path) {
        Ok(signature_info) => {
            debug!(
                "Signature file '{}' for verity device '{}' info:\n{}",
                temp_signature_file_path.display(),
                verity_device_id,
                signature_info
            );
        }
        Err(e) => {
            warn!(
                "Failed to get signature info from file '{}': {e:?}",
                temp_signature_file_path.display(),
            );
        }
    }

    debug!(
        "Opening verity device '{}' with signature file '{}' [{}]",
        verity_device_id,
        signature_file_path.display(),
        temp_signature_file_path.display(),
    );

    verity_device
        .open_with_signature(&temp_signature_file_path)
        .context(format!(
            "Failed to open verity device '{}' with signature file '{}'",
            verity_device_id,
            temp_signature_file_path.display()
        ))
}

/// Open a verity device using a signature from a dedicated partition.
///
/// cryptsetup requires a regular signature file. Extract only the bounded
/// DER object, excluding partition padding, for diagnostics and activation.
/// The matching certificate must exist in the kernel keyring.
fn open_verity_device_with_signature_partition(
    ctx: &EngineContext,
    verity_device_id: &BlockDeviceId,
    verity_device: VerityDeviceUtils,
    hash_signature_device_id: &BlockDeviceId,
) -> Result<(), Error> {
    let signature_block_device_path = ctx
        .get_block_device_path(hash_signature_device_id)
        .with_context(|| {
            format!("Failed to find path for block device '{hash_signature_device_id}'")
        })?;

    let mut source = fs::File::open(&signature_block_device_path).with_context(|| {
        format!(
            "Failed to read signature partition '{}'",
            signature_block_device_path.display()
        )
    })?;
    let signature = extract_verity_signature(&mut source).with_context(|| {
        format!("Failed to extract signature from partition '{hash_signature_device_id}'")
    })?;

    // Try to print signature info
    match veritysetup::get_verity_signature_info(signature.path()) {
        Ok(signature_info) => {
            debug!(
                "Signature partition '{}' for verity device '{}' info:\n{}",
                hash_signature_device_id, verity_device_id, signature_info
            );
        }
        Err(e) => {
            warn!(
                "Failed to get signature info from partition '{}': {e:?}",
                hash_signature_device_id,
            );
        }
    }

    debug!(
        "Opening verity device '{}' with signature from partition '{}' [{}]",
        verity_device_id,
        hash_signature_device_id,
        signature_block_device_path.display(),
    );

    verity_device
        .open_with_signature(signature.path())
        .context(format!(
            "Failed to open verity device '{}' with signature from partition '{}'",
            verity_device_id, hash_signature_device_id
        ))
}

const MAX_VERITY_SIGNATURE_SIZE: usize = 1024 * 1024;

fn extract_verity_signature(reader: &mut impl Read) -> Result<NamedTempFile, Error> {
    let mut header = [0u8; 2];
    reader
        .read_exact(&mut header)
        .context("Failed to read DER signature header")?;
    ensure!(header[0] == 0x30, "Signature must be a DER sequence");

    let mut length_bytes = [0u8; 4];
    let length_count = if header[1] & 0x80 != 0 {
        usize::from(header[1] & 0x7f)
    } else {
        0
    };
    let content_length = if header[1] & 0x80 == 0 {
        usize::from(header[1])
    } else {
        ensure!(
            (1..=length_bytes.len()).contains(&length_count),
            "Invalid DER length encoding"
        );
        reader
            .read_exact(&mut length_bytes[..length_count])
            .context("Failed to read DER signature length")?;
        ensure!(length_bytes[0] != 0, "Non-minimal DER length encoding");
        let length = length_bytes[..length_count]
            .iter()
            .fold(0usize, |length, byte| (length << 8) | usize::from(*byte));
        ensure!(length >= 128, "Non-minimal DER length encoding");
        length
    };
    ensure!(content_length > 0, "Signature DER sequence is empty");
    let header_length = header.len() + length_count;
    ensure!(
        content_length <= MAX_VERITY_SIGNATURE_SIZE - header_length,
        "Signature exceeds the {MAX_VERITY_SIGNATURE_SIZE}-byte limit"
    );

    let mut signature =
        NamedTempFile::new().context("Failed to create signature temporary file")?;
    signature
        .write_all(&header)
        .context("Failed to write DER signature header")?;
    signature
        .write_all(&length_bytes[..length_count])
        .context("Failed to write DER signature length")?;
    let copied = io::copy(&mut reader.take(content_length as u64), &mut signature)
        .context("Failed to copy DER signature")?;
    ensure!(copied == content_length as u64, "Truncated DER signature");
    signature
        .flush()
        .context("Failed to flush signature temporary file")?;
    Ok(signature)
}

/// Get the verity data and hash paths.
///
/// Verity data and hash devices are fetched from the engine context.
pub fn get_verity_device_paths(
    ctx: &EngineContext,
    verity_device: &VerityDevice,
) -> Result<(PathBuf, PathBuf), Error> {
    let verity_data_path = ctx
        .get_block_device_path(&verity_device.data_device_id)
        .context(format!(
            "Failed to find path of verity data device with id '{}'",
            verity_device.data_device_id
        ))?;

    let verity_hash_path = ctx
        .get_block_device_path(&verity_device.hash_device_id)
        .context(format!(
            "Failed to find verity hash device with ID '{}'",
            verity_device.hash_device_id
        ))?;

    Ok((verity_data_path, verity_hash_path))
}

/// Looks for verity devices created by Trident during servicing and stops them.
///
/// This specifically targets root-verity devices (named `root_new`) and usr-verity devices (named
/// `usr_new`).
#[tracing::instrument(skip_all)]
pub fn stop_trident_servicing_devices(host_config: &HostConfiguration) -> Result<(), Error> {
    // If no verity module is loaded, there are no verity devices to stop
    if !Path::new("/sys/module/dm_verity").exists() {
        return Ok(());
    }

    // Close the root-verity device
    stop_verity_device(
        host_config,
        &get_updated_device_name(ROOT_VERITY_DEVICE_NAME),
    )?;
    // Close the usr-verity device
    stop_verity_device(
        host_config,
        &get_updated_device_name(USR_VERITY_DEVICE_NAME),
    )?;

    Ok(())
}

/// Stops a specific verity device.
fn stop_verity_device(
    host_config: &HostConfiguration,
    verity_device_name: &str,
) -> Result<(), Error> {
    debug!("Attempting to stop pre-existing verity devices");

    let verity_device_path = Path::new(DEV_MAPPER_PATH).join(verity_device_name);

    // Check if the root-verity device is present
    if !verity_device_path.exists() {
        return Ok(());
    }

    // Check if the veritysetup command is available
    if !Dependency::Veritysetup.exists() {
        bail!("Veritysetup is not installed");
    }

    let root_verity_device_status = veritysetup::status(verity_device_name)
        .context("Failed to get status of root-verity device")?
        .active()
        .with_context(|| {
            format!(
                "Verity device '{}' is not active",
                verity_device_path.display()
            )
        })?;

    // Resolve disks in the HC to their /dev/... paths.
    let hc_disks = block_devices::get_resolved_disks(host_config)
        .context("Failed to resolved disks in the Host Configuration to their device paths.")?
        .iter()
        .map(|rd| rd.dev_path.to_owned())
        .collect::<HashSet<_>>();

    // Get the /dev/... paths of the disks that are used to store the verity members.
    let verity_disks = {
        let mut disks = HashSet::new();
        for verity_member in root_verity_device_status.members() {
            if let Ok(disk_path) = block_devices::get_disk_for_partition(verity_member) {
                let canonical_disk_path = disk_path
                    .canonicalize()
                    .context(format!("Failed to find the device path '{disk_path:?}'"))?;
                disks.insert(canonical_disk_path);
            } else if let Ok(disk_paths) = raid::get_raid_disks(verity_member) {
                disks.extend(disk_paths);
            } else {
                bail!(
                    "Failed to find the disk path for the device path '{:?}'",
                    verity_member
                )
            }
        }

        disks
    };

    // Get what the set of verity disks is in relation to the set of disks in the Host Configuration.
    match common::subset_check(&verity_disks, &hc_disks) {
        SetRelationship::Disjoint => {
            debug!("No overlap between the verity disks and the disks in the Host Configuration, device will not be stopped.");
            return Ok(());
        }
        SetRelationship::Overlap => {
            return Err(anyhow!(
                "A device has underlying disks that are not part of Host Configuration. Used disks: {:?}, Host Configuration disks: {:?}",
                verity_disks, hc_disks,
            )).context("Could not stop verity device.");
        }
        SetRelationship::Subset => {
            debug!("Verity disks are a subset of the disks in the Host Configuration, stopping device.");
        }
    }

    block_devices::unmount_all_mount_points(&verity_device_path).context(format!(
        "Failed to unmount all mount points for verity device '{}'",
        verity_device_path.display()
    ))?;

    debug!("Closing verity device '{}'", verity_device_path.display());
    veritysetup::close(verity_device_name).context(format!(
        "Failed to close root-verity device '{verity_device_name}'"
    ))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use sysdefs::partition_types::DiscoverablePartitionType;
    use trident_api::{
        config::{
            Disk, FileSystem, FileSystemSource, HostConfiguration, Partition, PartitionType,
            Storage, VerityDevice,
        },
        constants::ROOT_MOUNT_POINT_PATH,
    };

    use crate::osimage::{
        mock::{MockImage, MockOsImage},
        OsImage, OsImageFileSystemType,
    };

    #[test]
    fn test_extract_verity_signature_excludes_partition_padding() {
        for encoded in [
            vec![0x30, 3, 2, 1, 0],
            [vec![0x30, 0x81, 0x80], vec![0; 128]].concat(),
            [
                vec![0x30, 0x83, 0x0f, 0xff, 0xfb],
                vec![0; MAX_VERITY_SIGNATURE_SIZE - 5],
            ]
            .concat(),
        ] {
            let partition = [encoded.clone(), vec![0; 4096]].concat();
            let mut reader = io::Cursor::new(partition);
            let signature = extract_verity_signature(&mut reader).unwrap();
            assert_eq!(fs::read(signature.path()).unwrap(), encoded);
            assert_eq!(reader.position(), encoded.len() as u64);
            assert!(signature.as_file().metadata().unwrap().is_file());
        }
    }

    #[test]
    fn test_extract_verity_signature_rejects_invalid_der() {
        for (encoded, expected) in [
            (vec![], "header"),
            (vec![0x30], "header"),
            (vec![0x31, 1, 0], "DER sequence"),
            (vec![0x30, 0], "empty"),
            (vec![0x30, 0x80], "length encoding"),
            (vec![0x30, 0x85], "length encoding"),
            (vec![0x30, 0x82, 1], "signature length"),
            (vec![0x30, 0x82, 0, 0x80], "Non-minimal"),
            (vec![0x30, 0x81, 0x7f], "Non-minimal"),
            (vec![0x30, 0x83, 0x10, 0, 0], "limit"),
            (vec![0x30, 3, 0], "Truncated"),
        ] {
            let mut reader = io::Cursor::new(encoded);
            let error = extract_verity_signature(&mut reader).unwrap_err();
            assert!(error.to_string().contains(expected), "{error:#}");
        }
    }

    #[test]
    fn test_get_updated_device_name() {
        assert_eq!(get_updated_device_name("root"), "root_new");
        assert_eq!(get_updated_device_name("foo"), "foo_new");
    }

    #[test]
    fn test_get_usr_verity_root_hash() {
        let expected_root_hash = "sample-roothash";
        let mut mock = MockOsImage::new().with_image(MockImage::new(
            USR_MOUNT_POINT_PATH,
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::Root,
            Some(expected_root_hash),
        ));

        let as_ctx = |mock: &MockOsImage| EngineContext {
            image: Some(OsImage::mock(mock.clone())),
            ..Default::default()
        };

        assert_eq!(
            get_usr_verity_root_hash(&as_ctx(&mock)).unwrap(),
            expected_root_hash,
            "Root hash does not match expected"
        );

        // test failure when root filesystem is not verity enabled
        mock.images[0].verity = None;
        assert_eq!(
            get_usr_verity_root_hash(&as_ctx(&mock))
                .unwrap_err()
                .to_string(),
            "usr filesystem in OS image is not verity enabled",
            "Got unexpected error"
        );

        // test failure when root filesystem is not found
        mock.images.clear();
        assert_eq!(
            get_usr_verity_root_hash(&as_ctx(&mock))
                .unwrap_err()
                .to_string(),
            "Failed to get usr filesystem from OS image",
            "Got unexpected error"
        );
    }

    #[test]
    fn test_get_root_verity_root_hash() {
        let expected_root_hash = "sample-roothash";
        let mut mock = MockOsImage::new().with_image(MockImage::new(
            ROOT_MOUNT_POINT_PATH,
            OsImageFileSystemType::Ext4,
            DiscoverablePartitionType::Root,
            Some(expected_root_hash),
        ));

        let as_ctx = |mock: &MockOsImage| EngineContext {
            image: Some(OsImage::mock(mock.clone())),
            ..Default::default()
        };

        assert_eq!(
            get_root_verity_root_hash(&as_ctx(&mock)).unwrap(),
            expected_root_hash,
            "Root hash does not match expected"
        );

        // test failure when root filesystem is not verity enabled
        mock.images[0].verity = None;
        assert_eq!(
            get_root_verity_root_hash(&as_ctx(&mock))
                .unwrap_err()
                .to_string(),
            "Root filesystem in OS image is not verity enabled",
            "Got unexpected error"
        );

        // test failure when root filesystem is not found
        mock.images.clear();
        assert_eq!(
            get_root_verity_root_hash(&as_ctx(&mock))
                .unwrap_err()
                .to_string(),
            "Failed to get root filesystem from OS image",
            "Got unexpected error"
        );
    }

    /// Tests that [`setup_verity_devices`] prefers the declarative
    /// `hash_signature_device_id` signature partition over the legacy
    /// `veritySignaturePaths` internal parameter when both are present on the
    /// same verity device.
    #[test]
    fn test_setup_verity_devices_signature_partition_takes_precedence() {
        let mut hc = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "sdb".to_string(),
                    device: PathBuf::from("/dev/sdb"),
                    partitions: vec![
                        Partition {
                            id: "root-hash".to_string(),
                            partition_type: PartitionType::RootVerity,
                            size: 4096.into(),
                            uuid: None,
                            label: None,
                        },
                        Partition {
                            id: "root-data".to_string(),
                            partition_type: PartitionType::Root,
                            size: 4096.into(),
                            uuid: None,
                            label: None,
                        },
                        Partition {
                            id: "root-hash-sig".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 4096.into(),
                            uuid: None,
                            label: None,
                        },
                    ],
                    ..Default::default()
                }],
                verity: vec![VerityDevice {
                    id: "root".into(),
                    name: "root".into(),
                    data_device_id: "root-data".into(),
                    hash_device_id: "root-hash".into(),
                    hash_signature_device_id: Some("root-hash-sig".into()),
                    ..Default::default()
                }],
                filesystems: vec![FileSystem {
                    device_id: Some("root".to_string()),
                    mount_point: Some(ROOT_MOUNT_POINT_PATH.into()),
                    source: FileSystemSource::Image,
                    is_esp: false,
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        // Also configure the legacy signature-file mechanism for the same
        // verity device, to prove it is NOT consulted while
        // hash_signature_device_id is set.
        hc.internal_params = serde_yaml::from_str(&format!(
            "{}:\n  root: /mnt/some/signature/file",
            VERITY_SIGNATURE_PATHS
        ))
        .unwrap();

        let ctx = EngineContext::default()
            .with_spec(hc)
            .with_image(MockOsImage::new().with_image(MockImage::new(
                ROOT_MOUNT_POINT_PATH,
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::Root,
                Some("deadbeef".to_string()),
            )))
            .with_partition_paths(
                [
                    ("sdb", PathBuf::from("/dev/sdb")),
                    ("root-hash", PathBuf::from("/dev/sdb1")),
                    ("root-data", PathBuf::from("/dev/sdb2")),
                    // Deliberately do NOT register "root-hash-sig": if the
                    // declarative signature-partition path is taken (as it
                    // should be), setup fails here with a specific error
                    // naming that partition, before any device is opened.
                ]
                .into_iter(),
            );

        let err = setup_verity_devices(&ctx).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("signature partition 'root-hash-sig'"),
            "Expected the declarative signature partition to be tried first, got: {message}"
        );
    }
}

#[cfg(feature = "functional-test")]
#[cfg_attr(not(test), allow(unused_imports, dead_code))]
mod functional_test {

    use super::*;

    use std::path::PathBuf;

    use const_format::formatcp;
    use maplit::btreemap;

    use osutils::{
        filesystems::MountFileSystemType,
        mount::{self, MountGuard},
        mountpoint,
        testutils::{
            repart::TEST_DISK_DEVICE_PATH,
            verity::{self},
        },
        veritysetup::VerityDeviceGuard,
    };
    use pytest_gen::functional_test;
    use sysdefs::partition_types::DiscoverablePartitionType;
    use trident_api::{
        config::{
            Disk, FileSystem, FileSystemSource, NewFileSystemType, Partition, PartitionType,
            Storage, VerityDevice,
        },
        constants::{MOUNT_OPTION_READ_ONLY, ROOT_MOUNT_POINT_PATH},
    };

    use crate::osimage::{
        mock::{MockImage, MockOsImage},
        OsImageFileSystemType,
    };

    #[functional_test]
    fn test_setup_verity_devices() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // test no verity devices
        let ctx = EngineContext::default();
        setup_verity_devices(&ctx).unwrap();

        assert!(ctx.partition_paths.is_empty());

        // Test root-verity device
        let (boot_dev, verity_vol) = verity::setup_verity_volumes_with_boot();
        let verity_dev = verity_vol.verity_device("root_new");

        // Close the verity device if it exists
        verity_dev.close().unwrap();

        let hc = HostConfiguration {
            storage: Storage {
                disks: vec![Disk {
                    id: "sdb".to_string(),
                    device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                    partitions: vec![
                        Partition {
                            id: "boot".to_string(),
                            partition_type: PartitionType::Xbootldr,
                            size: 4096.into(),
                            uuid: None,
                            label: None,
                        },
                        Partition {
                            id: "root-hash".to_string(),
                            partition_type: PartitionType::RootVerity,
                            size: 4096.into(),
                            uuid: None,
                            label: None,
                        },
                        Partition {
                            id: "root-data".to_string(),
                            partition_type: PartitionType::Root,
                            size: 4096.into(),
                            uuid: None,
                            label: None,
                        },
                        Partition {
                            id: "overlay".to_string(),
                            partition_type: PartitionType::LinuxGeneric,
                            size: 4096.into(),
                            uuid: None,
                            label: None,
                        },
                    ],
                    ..Default::default()
                }],
                verity: vec![VerityDevice {
                    id: "root".into(),
                    name: "root".into(),
                    data_device_id: "root-data".into(),
                    hash_device_id: "root-hash".into(),
                    hash_signature_device_id: None,
                    ..Default::default()
                }],
                filesystems: vec![
                    FileSystem {
                        device_id: Some("root".to_string()),
                        mount_point: Some(ROOT_MOUNT_POINT_PATH.into()),
                        source: FileSystemSource::Image,
                        is_esp: false,
                    },
                    FileSystem {
                        device_id: Some("boot".to_string()),
                        mount_point: Some("/boot".into()),
                        source: FileSystemSource::Image,
                        is_esp: false,
                    },
                    FileSystem {
                        device_id: Some("overlay".to_string()),
                        mount_point: Some("/var/lib/trident-overlay".into()),
                        source: FileSystemSource::New(NewFileSystemType::Ext4),
                        is_esp: false,
                    },
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let ctx_golden = EngineContext::default()
            .with_spec(hc)
            .with_image(MockOsImage::new().with_image(MockImage::new(
                ROOT_MOUNT_POINT_PATH,
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::Root,
                Some(verity_vol.root_hash.clone()),
            )))
            .with_partition_paths(
                [
                    ("sdb", PathBuf::from(TEST_DISK_DEVICE_PATH)),
                    ("boot", boot_dev),
                    ("root-hash", verity_vol.hash_volume.clone()),
                    ("root-data", verity_vol.data_volume.clone()),
                    (
                        "overlay",
                        PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
                    ),
                ]
                .into_iter(),
            );

        {
            let ctx = ctx_golden.clone();
            setup_verity_devices(&ctx).unwrap();
            let _verityguard = VerityDeviceGuard::new("root_new");
            assert!(verity_dev.is_active().unwrap());
        }

        // test failure when root hash is not matching
        let bad_hash = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        assert_ne!(bad_hash, verity_vol.root_hash, "Root hash should not match");

        let ctx = ctx_golden
            .clone()
            .with_image(MockOsImage::new().with_image(MockImage::new(
                ROOT_MOUNT_POINT_PATH,
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::Root,
                Some(bad_hash.to_string()),
            )));

        assert_eq!(
            setup_verity_devices(&ctx).unwrap_err().to_string(),
            "Failed to activate verity device 'root_new', status: 'corrupted', expected: 'verified'"
        );

        // Failure should close the device!
        assert!(!verity_dev.is_active().unwrap());
    }

    #[functional_test]
    fn test_stop_pre_existing_verity_devices() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let verity_vol = verity::setup_verity_volumes();

        let ctx_golden = EngineContext {
            spec: HostConfiguration {
                storage: Storage {
                    disks: vec![Disk {
                        id: "sdb".to_string(),
                        device: PathBuf::from(TEST_DISK_DEVICE_PATH),
                        partitions: vec![
                            Partition {
                                id: "boot".to_string(),
                                partition_type: PartitionType::Xbootldr,
                                size: 100.into(),
                                uuid: None,
                                label: None,
                            },
                            Partition {
                                id: "root-hash".to_string(),
                                partition_type: PartitionType::RootVerity,
                                size: 100.into(),
                                uuid: None,
                                label: None,
                            },
                            Partition {
                                id: "root".to_string(),
                                partition_type: PartitionType::Root,
                                size: 100.into(),
                                uuid: None,
                                label: None,
                            },
                            Partition {
                                id: "overlay".to_string(),
                                partition_type: PartitionType::LinuxGeneric,
                                size: 100.into(),
                                uuid: None,
                                label: None,
                            },
                        ],
                        ..Default::default()
                    }],
                    verity: vec![VerityDevice {
                        id: "root-verity".into(),
                        name: "root".into(),
                        data_device_id: "root".into(),
                        hash_device_id: "root-hash".into(),
                        hash_signature_device_id: None,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            partition_paths: btreemap! {
                "foo".to_owned() => PathBuf::from(TEST_DISK_DEVICE_PATH),
                "boot".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}1")),
                "root-hash".to_owned() => verity_vol.hash_volume.clone(),
                "root".to_owned() => verity_vol.data_volume.clone(),
                "overlay".to_owned() => PathBuf::from(formatcp!("{TEST_DISK_DEVICE_PATH}4")),
            },
            ..Default::default()
        };

        // nothing mounted
        let verity_device = verity_vol.verity_device("root_new");
        assert!(!verity_device.is_active().unwrap());
        stop_trident_servicing_devices(&ctx_golden.spec).unwrap();

        // root-verity opened
        {
            let _guard = verity_device.open_with_guard().unwrap();
            assert!(verity_device.is_active().unwrap());
            stop_trident_servicing_devices(&ctx_golden.spec).unwrap();
            assert!(!verity_device.is_active().unwrap());
        }

        // root-verity opened & mounted
        {
            let _guard = verity_device.open_with_guard().unwrap();

            assert!(verity_device.is_active().unwrap());
            let mount_dir = tempfile::tempdir().unwrap();
            mount::mount(
                verity_device.device_path(),
                mount_dir.path(),
                MountFileSystemType::Ext4,
                &["defaults".into(), MOUNT_OPTION_READ_ONLY.into()],
            )
            .unwrap();
            // Create a mount guard that will automatically unmount when it goes
            // out of scope
            let _mount_guard = MountGuard {
                mount_dir: mount_dir.path(),
            };

            stop_trident_servicing_devices(&ctx_golden.spec).unwrap();
            assert!(!mountpoint::check_is_mountpoint(mount_dir.path()).unwrap());
            assert!(!verity_device.is_active().unwrap());
        }

        // TODO add across disks test
    }
}
