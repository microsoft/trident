use std::{
    collections::BTreeMap,
    io::{self, BufReader, Cursor, Read, Seek, SeekFrom, Write},
    ops::ControlFlow,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Error};
use gpt::{disk::LogicalBlockSize, GptConfig, GptDisk};
use log::{debug, trace};
use tar::Archive;
use url::Url;
use zstd::Decoder;

use sysdefs::partition_types::DiscoverablePartitionType;
use trident_api::{
    config::{HostConfiguration, ImageSha384, OsImage},
    error::{InternalError, ReportError, TridentError},
    primitives::hash::Sha384Hash,
};

use crate::io_utils::{
    file_reader::FileReader,
    hashing_reader::{HashingReader, HashingReader384},
};

use super::{
    GptPartitionInfo, OsImageFile, OsImageFileSystem, OsImagePartition, OsImageVerityHash,
};

mod derived_hc;
mod entries;
mod error;
mod metadata;
mod validation;

use entries::{CosiEntries, CosiEntry};
use metadata::{
    CosiMetadata, CosiMetadataVersion, DiskInfo, GptRegionType, ImageFile, KnownMetadataVersion,
    MetadataVersion,
};

/// Path to the COSI metadata file. Part of the COSI specification.
const COSI_METADATA_PATH: &str = "metadata.json";

/// Top-level COSI file representation.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct Cosi {
    /// The source URL of the COSI file.
    pub source: Url,

    /// The parsed COSI metadata. This is guaranteed to be valid according to
    /// the COSI specification. Because of lazy loading of file entries, the
    /// paths in the metadata may not have been verified to exist in the COSI
    /// file yet, but all other validation has been performed.
    pub metadata: CosiMetadata,

    /// SHA384 hash of the raw metadata content. This is used for validating the
    /// integrity of the metadata when the COSI file is read.
    pub metadata_sha384: Sha384Hash,

    /// GPT data for the disk, if the COSI version supports it and it is present
    /// in the metadata. This is populated on demand when the COSI instance is
    /// created, and can be used by images that require it (eg. for verity
    /// metadata on COSI v1.2+).
    partitioning_info: Option<CosiPartitioningInfo>,

    /// Internal reader for the COSI file.
    reader: FileReader,

    /// Cache of file entries in the COSI file. This is populated on demand as
    /// files are read from the COSI file, starting with the metadata file. This
    /// allows us to avoid reading the entire COSI archive at once, while still
    /// allowing for efficient access to files after they've been read once.
    entries: CosiEntries,
}

#[derive(Debug, Clone)]
pub(super) struct CosiPartitioningInfo {
    /// The raw data of the protective MBR (LBA 0) of the GPT disk. This is
    /// needed for deploying the GPT to disk, as the protective MBR is not part
    /// of the GPT disk structure and needs to be written separately.
    pub(super) lba0: Vec<u8>,

    /// The parsed GPT disk data from the COSI file.
    pub(super) gpt_disk: GptDisk<Cursor<Vec<u8>>>,

    /// Partition metadata.
    pub(super) partitions: BTreeMap<u32, CosiPartition>,
}

#[derive(Debug, Clone)]
pub(super) struct CosiPartition {
    /// The image file in the COSI file that contains the partition data.
    pub image_file: ImageFile,
    /// Information about the GPT partition.
    pub info: GptPartitionInfo,
}

impl Cosi {
    /// Creates a new COSI file instance from the given source URL.
    pub(super) fn new(source: &OsImage, timeout: Duration) -> Result<Self, Error> {
        trace!("Scanning COSI file from '{}'", source.url);

        // Create a new COSI reader factory. This will let us cleverly build
        // readers for the COSI file regardless of its location.
        let cosi_reader =
            FileReader::new(&source.url, timeout).context("Failed to create COSI reader.")?;

        let entries = read_entries_until_file(COSI_METADATA_PATH, cosi_reader.reader()?)?;
        trace!("Collected {} COSI entries", entries.len());

        // Read metadata from COSI file. Checksum validation is performed here.
        let (metadata, sha384) = read_cosi_metadata(&cosi_reader, &entries, source.sha384.clone())
            .context("Failed to read COSI file metadata.")?;

        // Create a new COSI instance.
        Ok(Cosi {
            metadata,
            source: source.url.clone(),
            reader: cosi_reader,
            metadata_sha384: sha384,
            entries,
            partitioning_info: None,
        })
    }

    /// Returns the ESP filesystem image.
    pub(super) fn esp_filesystem(&self) -> Result<OsImageFileSystem, Error> {
        self.metadata
            .get_esp_filesystem()
            .map(cosi_image_to_os_image_filesystem)
    }

    /// Returns an iterator of available mount points in the COSI file.
    pub(super) fn available_mount_points(&self) -> impl Iterator<Item = &Path> {
        self.metadata
            .get_regular_filesystems()
            .map(|image| image.mount_point.as_path())
    }

    /// Returns an iterator over all images that are NOT the ESP filesystem image.
    pub(super) fn filesystems(&self) -> impl Iterator<Item = OsImageFileSystem> {
        self.metadata
            .get_regular_filesystems()
            .map(cosi_image_to_os_image_filesystem)
    }

    /// Returns an iterator over all partitions defined in the metadata.
    pub(super) fn partitions(&self) -> Option<impl Iterator<Item = OsImagePartition>> {
        Some(
            self.partitioning_info
                .as_ref()?
                .partitions
                .values()
                .map(|part| OsImagePartition {
                    image_file: OsImageFile {
                        path: part.image_file.path.clone(),
                        compressed_size: part.image_file.compressed_size,
                        sha384: part.image_file.sha384.clone(),
                        uncompressed_size: part.image_file.uncompressed_size,
                    },
                    info: part.info.clone(),
                }),
        )
    }

    /// Returns the full disk size of the original disk that the OS image was
    /// created from, if specified in the metadata.
    pub(super) fn original_disk_size(&self) -> Option<u64> {
        self.metadata.disk.as_ref().map(|disk| disk.size)
    }

    /// Returns the GPT disk if it is present in the COSI file. This will be
    /// present if the COSI version is >= 1.2 and the metadata contains a disk
    /// section with a GPT region.
    pub(super) fn partitioning_info(&mut self) -> Result<Option<&CosiPartitioningInfo>, Error> {
        if self.partitioning_info.is_none() {
            ensure!(
                self.metadata.version >= KnownMetadataVersion::V1_2,
                "GPT data is not available for COSI versions below 1.2"
            );

            self.populate_gpt_data()
                .context("Failed to populate GPT data for COSI version >= 1.2")?;
        }

        Ok(self.partitioning_info.as_ref())
    }

    /// Derives the `image` and `storage` sections of the host configuration
    /// from the COSI file. This requires COSI >= 1.2.
    pub(super) fn derive_host_configuration(
        &mut self,
        target_disk: impl AsRef<Path>,
    ) -> Result<HostConfiguration, Error> {
        ensure!(
            self.metadata.version >= KnownMetadataVersion::V1_2,
            "Host configuration derivation requires COSI version {} or higher, found {}",
            KnownMetadataVersion::V1_2,
            self.metadata.version
        );

        // If we don't have GPT data, attempt to populate it from the disk metadata.
        if self.partitioning_info.is_none() {
            self.populate_gpt_data()
                .context("Failed to populate GPT data for COSI version >= 1.2")?;
        }

        let partitioning_info = self.partitioning_info.as_ref().context("Partitioning information is not available after populating it, cannot derive host configuration")?;

        derived_hc::derive_host_configuration_inner(
            &self.source,
            &self.metadata_sha384,
            target_disk,
            &self.metadata.images,
            partitioning_info,
        )
        .context("Failed to derive host configuration from COSI metadata and GPT data")
    }

    pub(super) fn read_images<F>(&self, mut f: F) -> Result<(), TridentError>
    where
        F: FnMut(&Path, Box<dyn Read>) -> ControlFlow<Result<(), TridentError>>,
    {
        let mut archive = Archive::new(
            self.reader
                .reader()
                .structured(InternalError::Internal("read COSI archive"))?,
        );
        for entry in
            read_entries(&mut archive).structured(InternalError::Internal("read COSI archive"))?
        {
            let (path, entry) = entry.structured(InternalError::Internal("read COSI archive"))?;

            let reader = Box::new(
                self.reader
                    .section_reader(entry.offset, entry.size)
                    .structured(InternalError::Internal("read COSI archive"))?,
            );
            match f(&path, reader) {
                ControlFlow::Continue(()) => {}
                ControlFlow::Break(b) => return b,
            }
        }
        Ok(())
    }

    /// Retrieves a reader for the given file inside the COSI file using cached
    /// entries when possible and scanning the COSI archive otherwise, and
    /// updating the cache as we scan.
    fn get_file_reader(&mut self, path: impl AsRef<Path>) -> Result<Box<dyn Read>, Error> {
        // Check if this entry has already been found.
        if let Some(entry) = self.entries.get(path.as_ref()) {
            return self
                .reader
                .section_reader(entry.offset, entry.size)
                .context(format!(
                    "Failed to create reader for COSI file entry '{}'",
                    path.as_ref().display()
                ));
        }

        // Otherwise, read the entries until we find the requested file, storing
        // all seen entries in the process. For extra efficiency, we know that
        // unknown entries must come _after_ any entries we've already seen, so
        // we can start reading after the last known entry.

        let next_header = self.entries.next_entry_offset();

        let mut reader = self
            .reader
            .reader()
            .context("Failed to create COSI archive reader")?;

        trace!(
            "Seeking to position {} to look for COSI file entry '{}'",
            next_header,
            path.as_ref().display()
        );

        reader
            .seek(SeekFrom::Start(next_header))
            .with_context(|| format!("Failed to seek to position {}", next_header))?;

        let mut archive = Archive::new(reader);

        for entry in read_entries_with_offset(&mut archive, next_header)
            .context("Failed to read COSI entries")?
        {
            let (entry_path, entry) = entry.context("Failed to read COSI entry")?;
            self.entries.register(&entry_path, entry)?;
            if entry_path == path.as_ref() {
                return self
                    .reader
                    .section_reader(entry.offset, entry.size)
                    .context(format!(
                        "Failed to create reader for COSI file entry '{}'",
                        path.as_ref().display()
                    ));
            }
        }

        bail!("COSI file entry '{}' not found", path.as_ref().display());
    }

    /// Reads the given ImageFile into the provided writer. Returns the number of bytes read.
    ///
    /// Will error when:
    /// - The image is not found in the COSI file.
    /// - The image cannot be read from the COSI file. (Decompression errors, etc.)
    /// - The read data size does not match the expected uncompressed size in the metadata.
    /// - The image hash does not match the expected hash in the metadata.
    fn stream_image(&mut self, image: &ImageFile, writer: &mut dyn Write) -> Result<u64, Error> {
        let mut hashing_reader = HashingReader384::new(self.get_file_reader(&image.path)?);
        let mut reader = Decoder::new(BufReader::new(&mut hashing_reader))?;

        // If the metadata specifies a max window log for compression, set it on
        // the reader to guarantee successful decompression.
        if let Some(max_window_log) = self.metadata.compression.as_ref().map(|c| c.max_window_log) {
            reader.window_log_max(max_window_log).with_context(|| {
                format!(
                    "Failed to set max window log of {} on COSI file entry '{}'",
                    max_window_log,
                    image.path.display()
                )
            })?;
        }

        let copied = io::copy(&mut reader, writer).context(format!(
            "Failed to read COSI file entry '{}'",
            image.path.display()
        ))?;

        if copied != image.uncompressed_size {
            bail!(
                "COSI file entry '{}' uncompressed size mismatch: expected {}, got {}",
                image.path.display(),
                image.uncompressed_size,
                copied
            );
        }

        if image.sha384 != hashing_reader.hash() {
            bail!(
                "COSI file entry '{}' hash '{}' does not match expected hash '{}'",
                image.path.display(),
                hashing_reader.hash(),
                image.sha384
            );
        }

        Ok(copied)
    }

    /// Retrieves the raw data of the given file inside the COSI file. Should only be used
    /// for small files as it reads the entire file into memory!
    fn get_file_data(&mut self, image: &ImageFile) -> Result<Vec<u8>, Error> {
        let mut data = Vec::with_capacity(image.uncompressed_size as usize);
        self.stream_image(image, &mut data).context(format!(
            "Failed to read COSI file entry '{}'",
            image.path.display()
        ))?;
        Ok(data)
    }

    /// On COSI >= v1.2, populates GPT data for images that require it. The
    /// caller is expected to have already checked that the COSI version is >=
    /// 1.2 before calling this function.
    fn populate_gpt_data(&mut self) -> Result<(), Error> {
        trace!("Populating GPT data for COSI file: {}", self.source);
        // First, get the gpt region from the metadata. All of the possible
        // errors here should have been checked in validation already.
        let disk_info = self
            .metadata
            .disk
            .as_ref()
            .context("Disk information not populated for COSI >= 1.2")?
            .clone(); // Clone to avoid borrowing issues later.

        let gpt_region = disk_info
            .gpt_regions
            .first()
            .context("GPT regions not defined in COSI >= 1.2")?;

        // This should be checked by validation, but we double check here.
        ensure!(
            gpt_region.region_type == GptRegionType::PrimaryGpt,
            "GPT region is not of type PrimaryGpt"
        );

        let raw_gpt_data = self
            .get_file_data(&gpt_region.image)
            .context("Failed to read GPT image data from COSI file")?;

        self.partitioning_info = Some(
            create_cosi_partitioning_info(raw_gpt_data, disk_info)
                .context("Failed to produce COSI partitioning information.")?,
        );

        Ok(())
    }
}

/// Creates the complete COSI partitioning information by reading the raw GPT
/// data from the COSI file and correlating it with the disk metadata in the
/// COSI metadata.
fn create_cosi_partitioning_info(
    raw_gpt_data: Vec<u8>,
    disk_info: DiskInfo,
) -> Result<CosiPartitioningInfo, Error> {
    // Extract a copy of the protective MBR (LBA 0) from the raw GPT data.
    // This is needed because the protective MBR is not part of the GPT disk
    // structure and needs to be written separately when deploying the GPT
    // to disk.
    let lba0 = raw_gpt_data[0..disk_info.lba_size as usize].to_vec();

    // Now get a reader for the image that contains the GPT.
    let raw_gpt_cursor = Cursor::new(raw_gpt_data);

    // Determine the LBA size from the disk metadata. This is needed to
    // calculate partition sizes from the GPT data. The GPT library we use
    // only supports 512 and 4096 byte LBAs, so we error if it's any other
    // value.
    let lba_size_gpt = match disk_info.lba_size {
        512 => LogicalBlockSize::Lb512,
        4096 => LogicalBlockSize::Lb4096,
        other => bail!("Unsupported LBA size: {}", other),
    };

    let gpt_disk = GptConfig::new()
        .writable(false)
        .logical_block_size(lba_size_gpt)
        .open_from_device(raw_gpt_cursor)?;

    // We now have the gpt data. Next, we correlate it with the COSI metadata.
    let gpt_partitions = gpt_disk.partitions();

    trace!(
        "Successfully read GPT data from COSI file, found {} partitions",
        gpt_partitions.len()
    );

    let metadata_partitions = disk_info
        .gpt_regions
        .into_iter()
        .filter_map(|r| match r.region_type {
            GptRegionType::Partition { number } => Some((r.image, number)),
            _ => None,
        })
        .collect::<Vec<_>>();

    ensure!(
        metadata_partitions.len() == gpt_partitions.len(),
        "Number of partitions in disk metadata ({}) does not match number of GPT partitions ({})",
        metadata_partitions.len(),
        gpt_partitions.len()
    );

    let partitions = metadata_partitions
        .into_iter()
        .map(|(image, number)| {
            let gpt_partition = gpt_partitions.get(&number).with_context(|| {
                format!(
                    "GPT partition number {} referenced in disk metadata not found in GPT data",
                    number
                )
            })?;

            let size = gpt_partition
                .bytes_len(lba_size_gpt)
                .with_context(|| format!("Failed to calculate size of partition {number}"))?;

            let part = CosiPartition {
                image_file: image,
                info: GptPartitionInfo {
                    partition_number: number,
                    size,
                    part_type: DiscoverablePartitionType::from_uuid(
                        &gpt_partition.part_type_guid.guid,
                    ),
                    part_uuid: gpt_partition.part_guid,
                    first_lba: gpt_partition.first_lba,
                    last_lba: gpt_partition.last_lba,
                    flags: gpt_partition.flags,
                    name: gpt_partition.name.clone(),
                },
            };

            Ok((number, part))
        })
        .collect::<Result<BTreeMap<u32, CosiPartition>, Error>>()
        .context("Failed to correlate GPT partitions with COSI metadata partitions")?;

    Ok(CosiPartitioningInfo {
        lba0,
        gpt_disk,
        partitions,
    })
}

/// Converts a COSI metadata Image to an OsImageFileSystem.
fn cosi_image_to_os_image_filesystem(image: &metadata::Image) -> OsImageFileSystem {
    // Make an early copy so the borrow checker knows that we are not keeping a reference to the
    // original image. Calling as_rer().map() on image.verity seems to tell the borrow checker
    // that we are keeping a reference to the original image, even if we only clone stuff and don't
    // keep a reference to the original image.
    let image = image.clone();
    OsImageFileSystem {
        mount_point: image.mount_point,
        fs_type: image.fs_type,
        fs_uuid: image.fs_uuid,
        part_type: image.part_type,
        image_file: OsImageFile {
            compressed_size: image.file.compressed_size,
            sha384: image.file.sha384,
            uncompressed_size: image.file.uncompressed_size,
            path: image.file.path.clone(),
        },
        verity: image.verity.map(|verity| OsImageVerityHash {
            hash_image_file: OsImageFile {
                compressed_size: verity.file.compressed_size,
                sha384: verity.file.sha384,
                uncompressed_size: verity.file.uncompressed_size,
                path: verity.file.path,
            },
            roothash: verity.roothash,
        }),
    }
}

fn read_entries_until_file<R: Read + Seek>(
    file_name: impl AsRef<Path>,
    cosi_reader: R,
) -> Result<CosiEntries, Error> {
    let mut entries = CosiEntries::default();
    for entry in read_entries(&mut Archive::new(cosi_reader))? {
        let (path, entry) = entry?;
        entries.register(&path, entry)?;
        if path == file_name.as_ref() {
            break;
        }
    }

    Ok(entries)
}

/// Iterate over entries from the given COSI tar archive.
fn read_entries<'a, R: Read + Seek + 'a>(
    archive: &'a mut Archive<R>,
) -> Result<impl Iterator<Item = Result<(PathBuf, CosiEntry), Error>> + 'a, Error> {
    read_entries_with_offset(archive, 0)
}

/// Iterate over entries from the given COSI tar archive that located at a specific offset of the reader.
fn read_entries_with_offset<'a, R: Read + Seek + 'a>(
    archive: &'a mut Archive<R>,
    offset: u64,
) -> Result<impl Iterator<Item = Result<(PathBuf, CosiEntry), Error>> + 'a, Error> {
    Ok(archive
        .entries_with_seek()
        .context("Failed to read COSI file")?
        .inspect(|entry| {
            trace!("Reading COSI file entry");
            match entry {
                Ok(entry) => {
                    trace!(
                        "Successfully read COSI file entry: {}",
                        match entry.path() {
                            Ok(path) => path.display().to_string(),
                            Err(err) => format!("Failed to read entry path: {err}"),
                        }
                    );
                }
                Err(err) => {
                    trace!("Failed to read COSI file entry: {}", err);
                }
            };
        })
        .map(move |entry_res| {
            let entry = entry_res.context("Failed to read COSI file entry")?;
            let entry = (
                {
                    let path = entry.path().context("Failed to read entry path")?;
                    path.strip_prefix("./").unwrap_or(&path).to_path_buf()
                },
                CosiEntry {
                    offset: entry.raw_file_position() + offset,
                    size: entry.size(),
                },
            );

            trace!(
                "Found COSI entry '{}' at {} [{} bytes]",
                entry.0.display(),
                entry.1.offset,
                entry.1.size
            );
            Ok(entry)
        }))
}

/// Retrieves the COSI metadata from the given COSI file.
///
/// It also:
/// - Validates the metadata version.
/// - Ensures that all images defined in the metadata are present in the COSI file.
/// - Populates metadata with the actual content location of the images.
fn read_cosi_metadata(
    cosi_reader: &FileReader,
    entries: &CosiEntries,
    expected_sha384: ImageSha384,
) -> Result<(CosiMetadata, Sha384Hash), Error> {
    trace!(
        "Retrieving metadata from COSI file from '{}'",
        COSI_METADATA_PATH
    );
    let metadata_location = entries
        .get(Path::new(COSI_METADATA_PATH))
        .context("COSI metadata not found")?;
    trace!(
        "Found COSI metadata in '{}' at {} [{} bytes]",
        COSI_METADATA_PATH,
        metadata_location.offset,
        metadata_location.size
    );

    let mut metadata_reader = HashingReader384::new(
        cosi_reader
            .section_reader(metadata_location.offset, metadata_location.size)
            .context("Failed to create COSI metadata reader")?,
    );

    let mut raw_metadata = String::new();
    metadata_reader
        .read_to_string(&mut raw_metadata)
        .context("Failed to read COSI metadata")?;

    let actual_sha384 = Sha384Hash::from(metadata_reader.hash());
    if let ImageSha384::Checksum(ref sha384) = expected_sha384 {
        if actual_sha384 != *sha384 {
            bail!("COSI metadata hash '{actual_sha384}' does not match expected hash '{sha384}'");
        }
    }
    trace!("Raw COSI metadata:\n{}", raw_metadata);

    // First, attempt to ONLY parse the metadata version to ensure we can read the rest.
    validate_cosi_metadata_version(
        &serde_json::from_str::<CosiMetadataVersion>(&raw_metadata)
            .context("Failed to parse COSI metadata version")?
            .version,
    )?;

    // Now, parse the full metadata.
    let metadata: CosiMetadata =
        serde_json::from_str(&raw_metadata).context("Failed to parse COSI metadata")?;

    // Validate the metadata.
    metadata.validate()?;

    debug!(
        "Successfully read COSI metadata [v{}.{}]",
        metadata.version.major, metadata.version.minor
    );

    Ok((metadata, actual_sha384))
}

/// Validates the COSI metadata version.
fn validate_cosi_metadata_version(version: &MetadataVersion) -> Result<(), Error> {
    trace!(
        "Validating COSI metadata version: {}.{}",
        version.major,
        version.minor
    );

    if version.major != 1 {
        bail!(
            "Unsupported COSI version: {}.{}, (minimum: 1.0)",
            version.major,
            version.minor
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        io::{Cursor, Write},
        str,
    };

    use metadata::{Image, VerityMetadata};
    use sha2::{Digest, Sha384};
    use tar::{Builder, Header};
    use tempfile::NamedTempFile;
    use uuid::Uuid;

    use osutils::osrelease::OsRelease;
    use sysdefs::{
        arch::SystemArchitecture, osuuid::OsUuid, partition_types::DiscoverablePartitionType,
    };
    use trident_api::primitives::hash::Sha384Hash;

    use crate::osimage::{cosi::metadata::ImageFile, OsImageFileSystemType};

    use super::metadata::KnownMetadataVersion;

    /// Generate a test tarball with the given entries.
    ///
    /// An entry is a tuple of (path, data).
    fn generate_test_tarball<'a>(
        entries: impl Iterator<Item = (&'static str, &'a [u8])>,
    ) -> Vec<u8> {
        let mut archive = Builder::new(Vec::with_capacity(4096));
        for (path, data) in entries {
            // Create a new header with appropriate metadata.
            let mut header = Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_cksum();

            // Write the data to the archive.
            archive.append_data(&mut header, path, data).unwrap();
        }

        // Finish the archive, retrieve inner Vec.
        archive.finish().unwrap();
        archive.into_inner().unwrap()
    }

    /// Generate a sample COSI v1.0 metadata file from the given entries.
    ///
    /// An entry is a tuple of (path, data).
    ///
    /// Since this is a spec, hard-coding a string made by following the spec
    /// means we also check compliance. It also prevents us from having to
    /// implement Serialize for the metadata only for testing.
    fn generate_sample_metadata_v1_0<'a>(
        images: impl Iterator<Item = (&'static str, u64, &'a str)>,
    ) -> String {
        let images = images
            .map(|(path, size, hash)| {
                format!(
                    indoc::indoc! {r#"
                        {{
                            "image": {{
                                "path": "{}",
                                "compressedSize": {},
                                "uncompressedSize": 2048,
                                "sha384": "{sha384}"
                            }},
                            "mountPoint": "/some/mount/point/{}",
                            "fsType": "ext4",
                            "fsUuid": "{fsuuid}",
                            "partType": "{parttype}"
                        }}
                    "#},
                    path,
                    size,
                    path,
                    sha384 = hash,
                    fsuuid = Uuid::new_v4(),
                    parttype = DiscoverablePartitionType::LinuxGeneric.to_uuid(),
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");

        format!(
            indoc::indoc! {r#"
                {{
                    "version": "1.0",
                    "id": "{id}",
                    "osArch": "x86_64",
                    "osRelease": "",
                    "images": [
                        {}
                    ]
                }}
            "#},
            images,
            id = Uuid::new_v4(),
        )
    }

    /// Tests the [`read_entries`] function for reading tar archive entries.
    ///
    /// Verifies that the function correctly iterates over entries in a tar archive,
    /// extracting their paths, offsets, and sizes. The test creates a sample tarball
    /// with multiple files (including nested paths), reads the entries, and validates
    /// that the returned metadata matches the original data.
    #[test]
    fn test_read_entries_from_tar_archive() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let sample_data = [
            ("file1.txt", "file1-data"),
            ("file2.txt", "file2-data"),
            ("directory/file3.txt", "file3-data"),
        ];

        // Form a test archive.
        let cosi_file = generate_test_tarball(
            sample_data
                .iter()
                .map(|(path, data)| (*path, data.as_bytes())),
        );

        // Read the entries. Use a Cursor as a file stand-in. (Cursor implements Read + Seek)
        let mut archive = Archive::new(Cursor::new(&cosi_file));
        let entries: CosiEntries = super::read_entries(&mut archive)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                (e.0.to_owned(), e.1)
            })
            .collect();

        // Check the entries
        assert_eq!(
            entries.len(),
            sample_data.len(),
            " Incorrect number of entries"
        );

        // Check that each entry matches the expected data.
        for (path, data) in sample_data.iter() {
            let entry = entries.get(path).unwrap();
            assert_eq!(entry.size, data.len() as u64, "Incorrect entry size");
            let read_data = cosi_file
                .get(entry.offset as usize..(entry.offset + entry.size) as usize)
                .unwrap();

            assert_eq!(
                read_data,
                data.as_bytes(),
                "Incorrect entry data, expected '{}', got '{}'",
                data,
                String::from_utf8_lossy(read_data)
            );
        }
    }

    /// Tests the [`validate_cosi_metadata_version`] function for version validation.
    ///
    /// Verifies that:
    /// - All COSI 1.x versions (1.0, 1.1, 1.2) are accepted as valid.
    /// - Major version 0.x and 2.x are rejected as unsupported.
    #[test]
    fn test_validate_cosi_metadata_version() {
        // Test accepted versions
        super::validate_cosi_metadata_version(&MetadataVersion { major: 1, minor: 0 }).unwrap();
        super::validate_cosi_metadata_version(&MetadataVersion { major: 1, minor: 1 }).unwrap();
        super::validate_cosi_metadata_version(&MetadataVersion { major: 1, minor: 2 }).unwrap();

        // Test unsupported versions
        super::validate_cosi_metadata_version(&MetadataVersion { major: 0, minor: 0 }).unwrap_err();
        super::validate_cosi_metadata_version(&MetadataVersion { major: 0, minor: 1 }).unwrap_err();
        super::validate_cosi_metadata_version(&MetadataVersion { major: 2, minor: 1 }).unwrap_err();
    }

    /// Tests the [`read_cosi_metadata`] function for parsing COSI metadata.
    ///
    /// Validates that the function correctly:
    /// - Reads and parses the metadata.json file from a COSI archive.
    /// - Validates the SHA384 checksum of the metadata content.
    /// - Parses image definitions with their paths and sizes.
    ///
    /// The test creates a mock metadata file with sample image entries, writes it
    /// to a temporary file, and verifies that the parsed metadata matches the
    /// expected structure.
    #[test]
    fn test_read_cosi_metadata() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Fake images we will insert in the metadata. All data is purely
        // arbitrary, the only restriction is that the paths must be unique.
        //
        // We will then create fake entries for them, and finally, cross check
        // the entries with the metadata.
        //
        // The layout is (image_path_in_tarball, offset, size_in_tarball).
        let image_paths = [
            ("some/image/path/A", 1024u64, 1024u64),
            ("some/image/path/B", 2048u64, 4096u64),
            ("some/image/path/C", 6144u64, 8192u64),
        ];

        let dummy_hash = "0".repeat(96);

        let sample_metadata = generate_sample_metadata_v1_0(
            image_paths
                .iter()
                .map(|(path, _, size)| (*path, *size, dummy_hash.as_str())),
        );
        let metadata_sha384 = format!("{:x}", Sha384::digest(sample_metadata.as_bytes()));

        // To mock the COSI file reader, we'll need to dump the metadata into a
        // file. It doesn't really matter what the file is as long as the
        // metadata is written raw. :)
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(sample_metadata.as_bytes()).unwrap();

        // Create a COSI reader from the temp file.
        let cosi_reader = FileReader::new(
            &Url::from_file_path(temp_file.path()).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();

        // Create mock entries in a "hypothetical" COSI file. We will only read
        // the metadata from the file, so this is the only entry where accurate
        // data is needed.
        let entries = [(
            PathBuf::from(COSI_METADATA_PATH),
            CosiEntry {
                offset: 0,
                size: sample_metadata.len() as u64,
            },
        )]
        .into_iter()
        .chain(image_paths.iter().map(|(path, offset, size)| {
            // Create a fake entry for each image.
            (
                PathBuf::from(*path),
                CosiEntry {
                    offset: *offset,
                    size: *size,
                },
            )
        }))
        .collect();

        // Read the metadata.
        let metadata = read_cosi_metadata(
            &cosi_reader,
            &entries,
            ImageSha384::Checksum(metadata_sha384.into()),
        )
        .unwrap()
        .0;

        // Now check that the images in the metadata have the correct entries.
        for (image, (path, _offset, _size)) in metadata.images.iter().zip(image_paths.iter()) {
            assert_eq!(image.file.path, Path::new(path), "Incorrect image path",);
        }
    }

    /// Tests the [`Cosi::new`] constructor for creating a COSI instance.
    ///
    /// Validates that a COSI instance can be successfully created from a valid
    /// tarball containing metadata and image files. The test builds a complete
    /// COSI file with mock images and metadata, writes it to a temporary file,
    /// and verifies that `Cosi::new` correctly initializes the instance with
    /// the expected source URL.
    #[test]
    fn test_create_cosi() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // In this test we're building a fake COSI file with a few mock images
        // (aka files with arbitrary data) and metadata. We will then create a
        // COSI file instance from it and validate that the metadata is correct.

        // These are the mock "images". We don't need them to actually be
        // images, so we just have text files.
        let mock_images = [
            ("some/image/path/A", "this is some example data [A]"),
            ("some/image/path/B", "this is some example data [B]"),
            ("some/image/path/C", "this is some example data [C]"),
        ];

        let data_hashes = mock_images
            .iter()
            .map(|(_, data)| format!("{:x}", Sha384::digest(data.as_bytes())))
            .collect::<Vec<_>>();

        // Generate a sample COSI metadata file.
        let sample_metadata = generate_sample_metadata_v1_0(
            mock_images
                .iter()
                .zip(data_hashes.iter())
                .map(|((path, data), hash)| (*path, data.len() as u64, hash.as_str())),
        );

        // Generate a sample COSI file.
        let cosi_file = generate_test_tarball(
            [(COSI_METADATA_PATH, sample_metadata.as_bytes())]
                .into_iter()
                .chain(
                    mock_images
                        .iter()
                        .map(|(path, data)| (*path, data.as_bytes())),
                ),
        );

        // Write the COSI file to a temp file.
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&cosi_file).unwrap();

        // Create a COSI instance from the temp file.
        let url = Url::from_file_path(temp_file.path()).unwrap();
        let cosi = Cosi::new(
            &OsImage {
                url: url.clone(),
                sha384: ImageSha384::Ignored,
            },
            Duration::from_secs(5),
        )
        .unwrap();

        assert_eq!(url, cosi.source, "Incorrect source URL in COSI instance")
    }

    /// Tests [`Cosi::new`] with a COSI file that has a VHD footer appended.
    ///
    /// COSI files may be wrapped in VHD format for deployment scenarios. This test
    /// verifies that the COSI parser correctly handles tar archives that have
    /// additional data (a mock VPC/VHD footer with the "conectix" signature)
    /// appended after the tar content, without failing to parse the archive.
    #[test]
    fn test_create_cosi_with_footer() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // In this test we're building a fake COSI file with a few mock images
        // (aka files with arbitrary data) and metadata. We will then create a
        // COSI file instance from it and validate that the metadata is correct.

        // These are the mock "images". We don't need them to actually be
        // images, so we just have text files.
        let mock_images = [
            ("some/image/path/A", "this is some example data [A]"),
            ("some/image/path/B", "this is some example data [B]"),
            ("some/image/path/C", "this is some example data [C]"),
        ];

        let data_hashes = mock_images
            .iter()
            .map(|(_, data)| format!("{:x}", Sha384::digest(data.as_bytes())))
            .collect::<Vec<_>>();

        // Generate a sample COSI metadata file.
        let sample_metadata = generate_sample_metadata_v1_0(
            mock_images
                .iter()
                .zip(data_hashes.iter())
                .map(|((path, data), hash)| (*path, data.len() as u64, hash.as_str())),
        );

        // Generate a sample COSI file.
        let mut cosi_file = generate_test_tarball(
            [(COSI_METADATA_PATH, sample_metadata.as_bytes())]
                .into_iter()
                .chain(
                    mock_images
                        .iter()
                        .map(|(path, data)| (*path, data.as_bytes())),
                ),
        );

        // Append a mock vpc footer to the COSI file.
        let mut mock_vpc_footer = vec![0u8; 512];
        mock_vpc_footer[0..8].copy_from_slice(b"conectix");
        cosi_file.append(&mut mock_vpc_footer);

        // Write the COSI file to a temp file.
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&cosi_file).unwrap();

        // Create a COSI instance from the temp file.
        let url = Url::from_file_path(temp_file.path()).unwrap();
        let cosi = Cosi::new(
            &OsImage {
                url: url.clone(),
                sha384: ImageSha384::Ignored,
            },
            Duration::from_secs(5),
        )
        .unwrap();

        assert_eq!(url, cosi.source, "Incorrect source URL in COSI instance")
    }

    /// Tests the [`cosi_image_to_os_image_filesystem`] conversion function.
    ///
    /// Verifies that COSI metadata `Image` structures are correctly converted to
    /// `OsImageFileSystem` structures, preserving all fields including:
    /// - Mount point, filesystem type, and partition type.
    /// - Image file metadata (sizes, SHA384 hash, path).
    /// - Optional verity metadata when present.
    ///
    /// The test runs two scenarios: one without verity data and one with verity
    /// metadata to ensure both cases are handled correctly.
    #[test]
    fn test_cosi_image_to_os_image_filesystem() {
        let data = "some data";
        let mut cosi_img = Image {
            file: ImageFile {
                path: PathBuf::from("some/path"),
                compressed_size: data.len() as u64,
                uncompressed_size: data.len() as u64,
                sha384: Sha384Hash::from(format!("{:x}", Sha384::digest(data.as_bytes()))),
            },
            mount_point: PathBuf::from("/some/mount/point"),
            fs_type: OsImageFileSystemType::Ext4,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::LinuxGeneric,
            verity: None,
        };
        let os_fs = cosi_image_to_os_image_filesystem(&cosi_img);

        assert_eq!(os_fs.mount_point, cosi_img.mount_point);
        assert_eq!(os_fs.fs_type, cosi_img.fs_type);
        assert_eq!(os_fs.part_type, cosi_img.part_type);
        assert_eq!(
            os_fs.image_file.compressed_size,
            cosi_img.file.compressed_size
        );
        assert_eq!(os_fs.image_file.sha384, cosi_img.file.sha384);
        assert_eq!(
            os_fs.image_file.uncompressed_size,
            cosi_img.file.uncompressed_size
        );
        assert!(os_fs.verity.is_none());

        assert_eq!(
            os_fs.image_file.compressed_size,
            cosi_img.file.compressed_size
        );

        // Now test with verity.
        let root_hash = "some-root-hash-1234";
        let verity_data = "some data";
        cosi_img.verity = Some(VerityMetadata {
            file: ImageFile {
                path: PathBuf::from("some/verity/path"),
                compressed_size: verity_data.len() as u64,
                uncompressed_size: verity_data.len() as u64,
                sha384: Sha384Hash::from(format!("{:x}", Sha384::digest(verity_data.as_bytes()))),
            },
            roothash: root_hash.to_string(),
        });

        let os_fs = cosi_image_to_os_image_filesystem(&cosi_img);

        assert_eq!(os_fs.mount_point, cosi_img.mount_point);
        assert_eq!(os_fs.fs_type, cosi_img.fs_type);
        assert_eq!(os_fs.part_type, cosi_img.part_type);
        assert_eq!(
            os_fs.image_file.compressed_size,
            cosi_img.file.compressed_size
        );
        assert_eq!(os_fs.image_file.sha384, cosi_img.file.sha384);
        assert_eq!(
            os_fs.image_file.uncompressed_size,
            cosi_img.file.uncompressed_size
        );
        assert!(os_fs.verity.is_some());

        let os_fs_verity = os_fs.verity.unwrap();
        let cosi_img_verity = cosi_img.verity.unwrap();

        assert_eq!(os_fs_verity.roothash, root_hash);
        assert_eq!(
            os_fs_verity.hash_image_file.compressed_size,
            cosi_img_verity.file.compressed_size
        );
        assert_eq!(
            os_fs_verity.hash_image_file.sha384,
            cosi_img_verity.file.sha384
        );
        assert_eq!(
            os_fs_verity.hash_image_file.uncompressed_size,
            cosi_img_verity.file.uncompressed_size
        );
    }

    /// Creates a mock COSI instance for testing filesystem-related methods.
    ///
    /// This helper function builds a minimal `Cosi` struct with mock images stored
    /// in an in-memory buffer. Each image entry is created from the provided tuple
    /// containing mount point, filesystem type, partition type, and raw file data.
    /// The data is written sequentially to a buffer, and the corresponding entry
    /// offsets are tracked for the cache.
    ///
    /// Note: The images are stored uncompressed, making this helper unsuitable for
    /// testing methods that require zstd decompression (use dedicated test setup
    /// for those cases).
    fn sample_verity_cosi_file(
        mock_images: &[(&str, OsImageFileSystemType, DiscoverablePartitionType, &str)],
    ) -> Cosi {
        // Reader data
        let mut data = Cursor::new(Vec::<u8>::new());
        let mut entries = CosiEntries::default();
        let mut images = Vec::new();

        for (mntpt, fs_type, pt_type, file_data) in mock_images.iter() {
            let filename = Uuid::new_v4().to_string();
            let entry = CosiEntry {
                offset: data.position(),
                size: file_data.len() as u64,
            };
            entries.register(&filename, entry).unwrap();

            data.write_all(file_data.as_bytes()).unwrap();

            images.push(Image {
                file: ImageFile {
                    path: PathBuf::from(filename),
                    compressed_size: file_data.len() as u64,
                    uncompressed_size: file_data.len() as u64,
                    sha384: Sha384Hash::from(format!("{:x}", Sha384::digest(file_data.as_bytes()))),
                },
                mount_point: PathBuf::from(mntpt),
                fs_type: *fs_type,
                fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                part_type: *pt_type,
                verity: None,
            });
        }

        Cosi {
            source: Url::parse("mock://").unwrap(),
            metadata: CosiMetadata {
                version: KnownMetadataVersion::V1_0.as_version(),
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images,
                bootloader: None,
                disk: None,
                compression: Default::default(),
            },
            reader: FileReader::Buffer(data),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            entries,
            partitioning_info: None,
        }
    }

    /// Tests the [`Cosi::esp_filesystem`] method for retrieving the ESP partition.
    ///
    /// Validates that:
    /// - An error is returned when no ESP filesystem exists in the COSI.
    /// - The correct ESP filesystem is returned when present among multiple images.
    ///
    /// The test uses a mock COSI file with multiple filesystem images (ESP, /boot,
    /// /, /var) and verifies that only the ESP partition is returned.
    #[test]
    fn test_esp_filesystem() {
        // Test with an empty COSI file.
        let empty = Cosi {
            source: Url::parse("mock://").unwrap(),
            metadata: CosiMetadata {
                version: KnownMetadataVersion::V1_0.as_version(),
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                images: vec![],
                os_packages: None,
                bootloader: None,
                disk: None,
                compression: Default::default(),
            },
            reader: FileReader::Buffer(Cursor::new(Vec::<u8>::new())),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            entries: CosiEntries::default(),
            partitioning_info: None,
        };

        // Weird behavior with none/multiple ESPs is primarily tested by the
        // unit tests checking underlying metadata methods.
        assert_eq!(
            empty.esp_filesystem().unwrap_err().to_string(),
            "Expected exactly one ESP filesystem image, found 0"
        );

        // Test with a COSI file with multiple images.
        let mock_images = [
            (
                "/boot/efi",
                OsImageFileSystemType::Vfat,
                DiscoverablePartitionType::Esp,
                "my-esp-data",
            ),
            (
                "/boot",
                OsImageFileSystemType::Ext4,
                // Prism does not guarantee accurate partition types, for non-esp
                // partitions, so we test with linux generic here to ensure that's
                // ok.
                DiscoverablePartitionType::LinuxGeneric,
                "my-boot-data",
            ),
            (
                "/",
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::LinuxGeneric,
                "my-root-data",
            ),
            (
                "/var",
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::LinuxGeneric,
                "my-var-data",
            ),
        ];
        let cosi = sample_verity_cosi_file(&mock_images);
        let esp = cosi.esp_filesystem().unwrap();

        let expected = cosi_image_to_os_image_filesystem(
            // The ESP is the first image in the list.
            &cosi.metadata.images[0],
        );

        assert_eq!(esp.mount_point, expected.mount_point);
        assert_eq!(esp.fs_type, expected.fs_type);
        assert_eq!(esp.part_type, expected.part_type);
        assert_eq!(
            esp.image_file.compressed_size,
            expected.image_file.compressed_size
        );
        assert_eq!(esp.image_file.sha384, expected.image_file.sha384);
        assert_eq!(
            esp.image_file.uncompressed_size,
            expected.image_file.uncompressed_size
        );
        assert_eq!(esp.verity.is_none(), expected.verity.is_none());
    }

    /// Tests the [`Cosi::available_mount_points`] method.
    ///
    /// Verifies that the method returns an iterator over all non-ESP mount points
    /// defined in the COSI metadata. ESP partitions are excluded since they have
    /// a separate accessor method.
    #[test]
    fn test_available_mount_points() {
        let mock_images = [
            (
                "/boot/efi",
                OsImageFileSystemType::Vfat,
                DiscoverablePartitionType::Esp,
                "my-esp-data",
            ),
            (
                "/boot",
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::LinuxGeneric,
                "my-boot-data",
            ),
            (
                "/",
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::LinuxGeneric,
                "my-root-data",
            ),
            (
                "/var",
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::LinuxGeneric,
                "my-var-data",
            ),
        ];
        let cosi = sample_verity_cosi_file(&mock_images);

        let mount_points = cosi.available_mount_points().collect::<Vec<_>>();
        let expected = mock_images
            .iter()
            .filter(|data| data.2 != DiscoverablePartitionType::Esp)
            .map(|(mntpt, _, _, _)| Path::new(*mntpt))
            .collect::<Vec<_>>();

        assert_eq!(mount_points, expected);
    }

    /// Tests the [`Cosi::filesystems`] method for iterating over regular filesystems.
    ///
    /// Verifies that the method returns an iterator over all non-ESP filesystem
    /// images, correctly converting them to `OsImageFileSystem` structures. The
    /// test uses a mock COSI with ESP and regular partitions, confirming that
    /// only the regular filesystems are returned and all metadata is preserved.
    #[test]
    fn test_filesystems() {
        let mock_images = [
            (
                "/boot/efi",
                OsImageFileSystemType::Vfat,
                DiscoverablePartitionType::Esp,
                "my-esp-data",
            ),
            (
                "/boot",
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::LinuxGeneric,
                "my-boot-data",
            ),
            (
                "/",
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::LinuxGeneric,
                "my-root-data",
            ),
            (
                "/var",
                OsImageFileSystemType::Ext4,
                DiscoverablePartitionType::LinuxGeneric,
                "my-var-data",
            ),
        ];
        let cosi = sample_verity_cosi_file(&mock_images);

        let filesystems = cosi.filesystems().collect::<Vec<_>>();
        let expected = cosi
            .metadata
            .images
            .iter()
            .skip(1)
            .map(cosi_image_to_os_image_filesystem)
            .collect::<Vec<_>>();
        let img_data = mock_images
            .iter()
            .skip(1)
            .map(|(_, _, _, data)| *data)
            .collect::<Vec<_>>();
        assert_eq!(expected.len(), img_data.len());
        assert_eq!(filesystems.len(), expected.len());

        for (fs, (expected_fs, _expected_data)) in filesystems
            .iter()
            .zip(expected.iter().zip(img_data.into_iter()))
        {
            assert_eq!(fs.mount_point, expected_fs.mount_point);
            assert_eq!(fs.fs_type, expected_fs.fs_type);
            assert_eq!(fs.part_type, expected_fs.part_type);
            assert_eq!(
                fs.image_file.compressed_size,
                expected_fs.image_file.compressed_size
            );
            assert_eq!(fs.image_file.sha384, expected_fs.image_file.sha384);
            assert_eq!(
                fs.image_file.uncompressed_size,
                expected_fs.image_file.uncompressed_size
            );
            assert_eq!(fs.verity.is_none(), expected_fs.verity.is_none());
        }
    }

    /// Tests the [`Cosi::get_file_reader`] method for lazy file access.
    ///
    /// This method implements lazy loading of COSI archive entries, caching
    /// file locations as they are discovered. The test validates:
    /// 1. Reading a file already present in the entry cache returns correct content.
    /// 2. Reading an uncached file triggers archive scanning and caches the entry.
    /// 3. Nested file paths are correctly resolved.
    /// 4. Requesting a non-existent file returns an appropriate error.
    ///
    /// The test creates a tarball with mock files, pre-populates the cache with
    /// only the first entry, then exercises all code paths.
    #[test]
    fn test_get_file_reader() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Create mock files with known content.
        let mock_files = [
            ("file_a.txt", "content of file A"),
            ("file_b.txt", "content of file B"),
            ("nested/file_c.txt", "content of file C"),
        ];

        // Generate a test tarball with the mock files.
        let tarball = generate_test_tarball(
            mock_files
                .iter()
                .map(|(path, data)| (*path, data.as_bytes())),
        );

        // Write the tarball to a temp file.
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&tarball).unwrap();

        // Create a FileReader from the temp file.
        let url = Url::from_file_path(temp_file.path()).unwrap();
        let reader = FileReader::new(&url, Duration::from_secs(5)).unwrap();

        // Pre-populate entries with only the first file to test both cached
        // and uncached scenarios.
        let mut entries = CosiEntries::default();
        {
            let mut archive = Archive::new(Cursor::new(&tarball));
            for entry in read_entries(&mut archive).unwrap() {
                let (path, entry) = entry.unwrap();
                entries.register(&path, entry).unwrap();
                // Only cache the first file.
                if path == Path::new("file_a.txt") {
                    break;
                }
            }
        }

        // Create a minimal COSI instance for testing get_file_reader.
        let mut cosi = Cosi {
            source: url,
            metadata: CosiMetadata {
                version: KnownMetadataVersion::V1_0.as_version(),
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images: vec![],
                bootloader: None,
                disk: None,
                compression: Default::default(),
            },
            reader,
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            entries,
            partitioning_info: None,
        };

        // Test 1: Read a file that's already in the cache.
        {
            let mut reader = cosi.get_file_reader("file_a.txt").unwrap();
            let mut content = String::new();
            reader.read_to_string(&mut content).unwrap();
            assert_eq!(content, "content of file A", "Cached file content mismatch");
        }

        // Test 2: Read a file that's not in the cache (should scan and cache).
        {
            assert!(
                !cosi.entries.contains_key(Path::new("file_b.txt")),
                "file_b.txt should not be cached yet"
            );
            let mut reader = cosi.get_file_reader("file_b.txt").unwrap();
            let mut content = String::new();
            reader.read_to_string(&mut content).unwrap();
            assert_eq!(
                content, "content of file B",
                "Uncached file content mismatch"
            );
            // Verify it's now cached.
            assert!(
                cosi.entries.contains_key(Path::new("file_b.txt")),
                "file_b.txt should be cached after reading"
            );
        }

        // Test 3: Read a nested file that's not in the cache.
        {
            let mut reader = cosi.get_file_reader("nested/file_c.txt").unwrap();
            let mut content = String::new();
            reader.read_to_string(&mut content).unwrap();
            assert_eq!(content, "content of file C", "Nested file content mismatch");
        }

        // Test 4: Attempt to read a non-existent file.
        {
            let result = cosi.get_file_reader("non_existent.txt");
            assert!(result.is_err(), "Reading non-existent file should fail");
            let Err(err) = result else {
                unreachable!("Expected error for non-existent file")
            };
            let err_msg = err.to_string();
            assert!(
                err_msg.contains("not found"),
                "Error message should indicate file not found: {}",
                err_msg
            );
        }
    }

    /// Tests [`Cosi::stream_image`] and [`Cosi::get_file_data`] for reading compressed images.
    ///
    /// These methods handle zstd-compressed image files in COSI archives, performing
    /// decompression and integrity validation. The test validates:
    /// 1. `stream_image` correctly decompresses data and writes to a provided buffer.
    /// 2. `get_file_data` returns the same decompressed content.
    /// 3. Size mismatch between declared and actual uncompressed size is detected.
    /// 4. Hash mismatch between declared and actual compressed data hash is detected.
    /// 5. Non-existent files produce appropriate errors.
    ///
    /// The test creates zstd-compressed data, packages it in a tarball, and exercises
    /// both success and error paths.
    #[test]
    fn test_stream_image_and_get_file_data() {
        use zstd::stream::encode_all;

        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Create uncompressed test data.
        let uncompressed_data = b"This is some test data that will be compressed with zstd!";

        // Compress the data using zstd.
        let compressed_data =
            encode_all(uncompressed_data.as_slice(), 3).expect("Failed to compress test data");

        // Compute the SHA384 hash of the compressed data (stream_image hashes compressed data).
        let compressed_hash = format!("{:x}", Sha384::digest(&compressed_data));

        // Generate a test tarball containing the compressed file.
        let file_path = "test_image.zst";
        let tarball = generate_test_tarball([(file_path, compressed_data.as_slice())].into_iter());

        // Write the tarball to a temp file.
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&tarball).unwrap();

        // Create a FileReader from the temp file.
        let url = Url::from_file_path(temp_file.path()).unwrap();
        let reader = FileReader::new(&url, Duration::from_secs(5)).unwrap();

        // Read entries from the tarball to populate the cache.
        let entries = {
            let mut archive = Archive::new(Cursor::new(&tarball));
            read_entries(&mut archive)
                .unwrap()
                .map(|e| e.unwrap())
                .collect()
        };

        // Create a minimal COSI instance for testing.
        let mut cosi = Cosi {
            source: url,
            metadata: CosiMetadata {
                version: KnownMetadataVersion::V1_0.as_version(),
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images: vec![],
                bootloader: None,
                disk: None,
                compression: Default::default(),
            },
            reader,
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            entries,
            partitioning_info: None,
        };

        // Create an ImageFile descriptor for the test image.
        let image_file = ImageFile {
            path: PathBuf::from(file_path),
            compressed_size: compressed_data.len() as u64,
            uncompressed_size: uncompressed_data.len() as u64,
            sha384: Sha384Hash::from(compressed_hash.clone()),
        };

        // Test stream_image: stream decompressed data into a buffer.
        {
            let mut output = Vec::new();
            let bytes_read = cosi.stream_image(&image_file, &mut output).unwrap();

            assert_eq!(
                bytes_read,
                uncompressed_data.len() as u64,
                "stream_image returned incorrect byte count"
            );
            assert_eq!(
                output, uncompressed_data,
                "stream_image decompressed data mismatch"
            );
        }

        // Test get_file_data: should return the same decompressed data.
        {
            let data = cosi.get_file_data(&image_file).unwrap();
            assert_eq!(
                data, uncompressed_data,
                "get_file_data decompressed data mismatch"
            );
        }

        // Test error case: incorrect uncompressed size should fail.
        {
            let bad_size_image = ImageFile {
                path: PathBuf::from(file_path),
                compressed_size: compressed_data.len() as u64,
                uncompressed_size: uncompressed_data.len() as u64 + 100, // Wrong size
                sha384: Sha384Hash::from(compressed_hash.clone()),
            };
            let mut output = Vec::new();
            let result = cosi.stream_image(&bad_size_image, &mut output);
            assert!(result.is_err(), "stream_image should fail on size mismatch");
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("uncompressed size mismatch"),
                "Error should mention size mismatch: {}",
                err_msg
            );
        }

        // Test error case: incorrect hash should fail.
        {
            let bad_hash_image = ImageFile {
                path: PathBuf::from(file_path),
                compressed_size: compressed_data.len() as u64,
                uncompressed_size: uncompressed_data.len() as u64,
                sha384: Sha384Hash::from("0".repeat(96)), // Wrong hash
            };
            let mut output = Vec::new();
            let result = cosi.stream_image(&bad_hash_image, &mut output);
            assert!(result.is_err(), "stream_image should fail on hash mismatch");
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("does not match expected hash"),
                "Error should mention hash mismatch: {}",
                err_msg
            );
        }

        // Test error case: non-existent file should fail.
        {
            let missing_image = ImageFile {
                path: PathBuf::from("non_existent.zst"),
                compressed_size: 100,
                uncompressed_size: 100,
                sha384: Sha384Hash::from("0".repeat(96)),
            };
            let result = cosi.get_file_data(&missing_image);
            assert!(
                result.is_err(),
                "get_file_data should fail for non-existent file"
            );
        }
    }

    /// Tests [`Cosi::populate_gpt_data`] for parsing GPT partition tables.
    ///
    /// For COSI >= 1.2, the archive may contain GPT partition table data that needs
    /// to be parsed for verity and partition metadata. This test:
    /// 1. Creates a valid GPT disk image in memory with a protective MBR and one partition.
    /// 2. Compresses the primary GPT region with zstd.
    /// 3. Packages it in a COSI-like tarball with appropriate metadata.
    /// 4. Verifies that `populate_gpt_data` successfully parses the GPT.
    /// 5. Confirms the parsed partition table contains the expected partition.
    #[test]
    fn test_populate_gpt_data() {
        use gpt::mbr::ProtectiveMBR;
        use metadata::{DiskInfo, GptDiskRegion, PartitionTableType};
        use zstd::stream::encode_all;

        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Create a GPT disk image in memory.
        // We need at least enough space for protective MBR + GPT header + partition entries.
        // Minimum is typically: 512 (MBR) + 512 (GPT header) + 128*128 (partition entries) = ~17KB
        // We'll use 1MB to be safe and allow for some partitions.
        let disk_size: u64 = 1024 * 1024; // 1 MB
        let lba_size: u32 = 512;

        // Create a buffer that will hold our GPT disk.
        let mut disk_buffer = vec![0u8; disk_size as usize];

        // Write a protective MBR.
        {
            let mut cursor = Cursor::new(&mut disk_buffer[..]);
            let mbr = ProtectiveMBR::with_lb_size(
                u32::try_from((disk_size / lba_size as u64) - 1).unwrap_or(0xFFFFFFFF),
            );
            mbr.overwrite_lba0(&mut cursor).unwrap();
        }

        // Initialize and write GPT.
        {
            let cursor = Cursor::new(&mut disk_buffer[..]);
            let mut gpt_disk = GptConfig::new()
                .writable(true)
                .logical_block_size(LogicalBlockSize::Lb512)
                .create_from_device(cursor, None)
                .expect("Failed to create GPT disk");

            // Add a test partition.
            gpt_disk
                .add_partition(
                    "test_partition",
                    64 * 1024, // 64 KB
                    gpt::partition_types::LINUX_FS,
                    0,
                    None,
                )
                .expect("Failed to add partition");

            // Write the GPT to the buffer.
            gpt_disk.write().expect("Failed to write GPT");
        }

        // The primary GPT is from LBA 0 to the end of the partition entries.
        // For a 512-byte LBA size: MBR (512) + GPT Header (512) + 128 entries * 128 bytes = 17408 bytes.
        // But to be safe, we'll include more. The header says where entries end.
        // Typically LBA 0-33 (34 sectors * 512 = 17408 bytes).
        let primary_gpt_size: u64 = 34 * lba_size as u64;
        let raw_gpt_data = disk_buffer[..primary_gpt_size as usize].to_vec();

        // Compress the GPT data with zstd.
        let compressed_gpt =
            encode_all(raw_gpt_data.as_slice(), 3).expect("Failed to compress GPT data");

        // Compute hash of compressed data.
        let compressed_hash = format!("{:x}", Sha384::digest(&compressed_gpt));

        // Generate a test tarball containing the compressed GPT.
        let gpt_file_path = "gpt_primary.zst";
        let tarball =
            generate_test_tarball([(gpt_file_path, compressed_gpt.as_slice())].into_iter());

        // Write the tarball to a temp file.
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&tarball).unwrap();

        // Create a FileReader from the temp file.
        let url = Url::from_file_path(temp_file.path()).unwrap();
        let reader = FileReader::new(&url, Duration::from_secs(5)).unwrap();

        // Read entries from the tarball.
        let entries = {
            let mut archive = Archive::new(Cursor::new(&tarball));
            read_entries(&mut archive)
                .unwrap()
                .map(|e| e.unwrap())
                .collect()
        };

        // Create the GPT image file descriptor.
        let gpt_image_file = ImageFile {
            path: PathBuf::from(gpt_file_path),
            compressed_size: compressed_gpt.len() as u64,
            uncompressed_size: raw_gpt_data.len() as u64,
            sha384: Sha384Hash::from(compressed_hash),
        };

        // Create a partition image file descriptor (for the test partition).
        let partition_image_file = ImageFile {
            path: PathBuf::from("images/test_partition.img.zst"),
            compressed_size: 1024,
            uncompressed_size: 2048,
            sha384: Sha384Hash::from("0".repeat(96)),
        };

        // Create disk info with GPT region and partition region.
        let disk_info = DiskInfo {
            size: disk_size,
            lba_size,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![
                GptDiskRegion {
                    image: gpt_image_file,
                    region_type: GptRegionType::PrimaryGpt,
                },
                GptDiskRegion {
                    image: partition_image_file,
                    region_type: GptRegionType::Partition { number: 1 },
                },
            ],
        };

        // Create a COSI instance with disk metadata (version 1.2 to trigger GPT parsing).
        let mut cosi = Cosi {
            source: url,
            metadata: CosiMetadata {
                version: KnownMetadataVersion::V1_2.as_version(),
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images: vec![],
                bootloader: None,
                disk: Some(disk_info),
                compression: Default::default(),
            },
            reader,
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            entries,
            partitioning_info: None,
        };

        // Test: populate_gpt_data should succeed.
        cosi.populate_gpt_data()
            .expect("populate_gpt_data should succeed");

        // Verify GPT was populated.
        assert!(
            cosi.partitioning_info.is_some(),
            "GPT should be populated after call"
        );

        let gpt_data = cosi.partitioning_info.as_ref().unwrap();

        // Verify we can read the partition we added.
        assert_eq!(
            gpt_data.gpt_disk.partitions().len(),
            1,
            "Should have exactly one partition"
        );

        let (_, partition) = gpt_data.gpt_disk.partitions().iter().next().unwrap();
        assert_eq!(
            partition.name, "test_partition",
            "Partition name should match"
        );
    }

    /// Tests [`Cosi::populate_gpt_data`] error handling when disk info is missing.
    ///
    /// For COSI >= 1.2, disk metadata is required to locate GPT data. This test
    /// verifies that calling `populate_gpt_data` on a COSI instance without
    /// `disk` metadata returns an appropriate error message.
    #[test]
    fn test_populate_gpt_data_missing_disk_info() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Create a COSI instance without disk metadata.
        let mut cosi = Cosi {
            source: Url::parse("mock://").unwrap(),
            metadata: CosiMetadata {
                version: KnownMetadataVersion::V1_2.as_version(),
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images: vec![],
                bootloader: None,
                disk: None, // No disk info
                compression: Default::default(),
            },
            reader: FileReader::Buffer(Cursor::new(Vec::<u8>::new())),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            entries: CosiEntries::default(),
            partitioning_info: None,
        };

        // Test: populate_gpt_data should fail without disk info.
        let result = cosi.populate_gpt_data();
        assert!(result.is_err(), "Should fail without disk info");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Disk information not populated"),
            "Error should mention missing disk info: {}",
            err_msg
        );
    }

    /// Tests [`Cosi::populate_gpt_data`] error handling when GPT regions are empty.
    ///
    /// Even when disk metadata is present, the `gpt_regions` array must contain
    /// at least a `PrimaryGpt` entry. This test verifies that an empty
    /// `gpt_regions` array produces an appropriate error.
    #[test]
    fn test_populate_gpt_data_missing_gpt_regions() {
        use metadata::{DiskInfo, PartitionTableType};

        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Create disk info without GPT regions.
        let disk_info = DiskInfo {
            size: 1024 * 1024,
            lba_size: 512,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![], // Empty
        };

        // Create a COSI instance with disk info but no GPT regions.
        let mut cosi = Cosi {
            source: Url::parse("mock://").unwrap(),
            metadata: CosiMetadata {
                version: KnownMetadataVersion::V1_2.as_version(),
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images: vec![],
                bootloader: None,
                disk: Some(disk_info),
                compression: Default::default(),
            },
            reader: FileReader::Buffer(Cursor::new(Vec::<u8>::new())),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            entries: CosiEntries::default(),
            partitioning_info: None,
        };

        // Test: populate_gpt_data should fail without GPT regions.
        let result = cosi.populate_gpt_data();
        assert!(result.is_err(), "Should fail without GPT regions");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("GPT regions not defined"),
            "Error should mention missing GPT regions: {}",
            err_msg
        );
    }

    /// Tests [`Cosi::gpt`] for lazy-loading and accessing GPT partition data.
    ///
    /// The `gpt()` method provides lazy access to the GPT partition table for
    /// COSI >= 1.2 files. This test validates:
    /// 1. GPT data is successfully loaded on first access.
    /// 2. The returned GPT contains the expected partition.
    /// 3. Subsequent calls return the cached GPT without re-parsing.
    ///
    /// The test creates a valid GPT disk image in memory, compresses it with zstd,
    /// packages it in a tarball, and verifies the `gpt()` method correctly loads
    /// and caches the partition table.
    #[test]
    fn test_cosi_gpt_lazy_loading() {
        use gpt::mbr::ProtectiveMBR;
        use metadata::{DiskInfo, GptDiskRegion, PartitionTableType};
        use zstd::stream::encode_all;

        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Create a GPT disk image in memory (same setup as test_populate_gpt_data).
        let disk_size: u64 = 1024 * 1024;
        let lba_size: u32 = 512;
        let mut disk_buffer = vec![0u8; disk_size as usize];

        // Write protective MBR.
        {
            let mut cursor = Cursor::new(&mut disk_buffer[..]);
            let mbr = ProtectiveMBR::with_lb_size(
                u32::try_from((disk_size / lba_size as u64) - 1).unwrap_or(0xFFFFFFFF),
            );
            mbr.overwrite_lba0(&mut cursor).unwrap();
        }

        // Initialize and write GPT with a test partition.
        {
            let cursor = Cursor::new(&mut disk_buffer[..]);
            let mut gpt_disk = GptConfig::new()
                .writable(true)
                .logical_block_size(LogicalBlockSize::Lb512)
                .create_from_device(cursor, None)
                .expect("Failed to create GPT disk");

            gpt_disk
                .add_partition(
                    "gpt_test_partition",
                    64 * 1024,
                    gpt::partition_types::LINUX_FS,
                    0,
                    None,
                )
                .expect("Failed to add partition");

            gpt_disk.write().expect("Failed to write GPT");
        }

        // Extract and compress the primary GPT region.
        let primary_gpt_size: u64 = 34 * lba_size as u64;
        let raw_gpt_data = disk_buffer[..primary_gpt_size as usize].to_vec();
        let compressed_gpt =
            encode_all(raw_gpt_data.as_slice(), 3).expect("Failed to compress GPT data");
        let compressed_hash = format!("{:x}", Sha384::digest(&compressed_gpt));

        // Create tarball and temp file.
        let gpt_file_path = "gpt_primary.zst";
        let tarball =
            generate_test_tarball([(gpt_file_path, compressed_gpt.as_slice())].into_iter());
        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&tarball).unwrap();

        let url = Url::from_file_path(temp_file.path()).unwrap();
        let reader = FileReader::new(&url, Duration::from_secs(5)).unwrap();

        let entries = {
            let mut archive = Archive::new(Cursor::new(&tarball));
            read_entries(&mut archive)
                .unwrap()
                .map(|e| e.unwrap())
                .collect()
        };

        let gpt_image_file = ImageFile {
            path: PathBuf::from(gpt_file_path),
            compressed_size: compressed_gpt.len() as u64,
            uncompressed_size: raw_gpt_data.len() as u64,
            sha384: Sha384Hash::from(compressed_hash),
        };

        let partition_image_file = ImageFile {
            path: PathBuf::from("images/gpt_test_partition.img.zst"),
            compressed_size: 1024,
            uncompressed_size: 2048,
            sha384: Sha384Hash::from("0".repeat(96)),
        };

        let disk_info = DiskInfo {
            size: disk_size,
            lba_size,
            partition_table_type: PartitionTableType::Gpt,
            gpt_regions: vec![
                GptDiskRegion {
                    image: gpt_image_file,
                    region_type: GptRegionType::PrimaryGpt,
                },
                GptDiskRegion {
                    image: partition_image_file,
                    region_type: GptRegionType::Partition { number: 1 },
                },
            ],
        };

        let mut cosi = Cosi {
            source: url,
            metadata: CosiMetadata {
                version: KnownMetadataVersion::V1_2.as_version(),
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images: vec![],
                bootloader: None,
                disk: Some(disk_info),
                compression: Default::default(),
            },
            reader,
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            entries,
            partitioning_info: None,
        };

        // Verify GPT is not loaded initially.
        assert!(
            cosi.partitioning_info.is_none(),
            "GPT should not be pre-loaded"
        );

        // First call to gpt() should load the GPT.
        let gpt_result = cosi.partitioning_info();

        let gpt = gpt_result.unwrap();
        assert!(gpt.is_some(), "GPT should be present after gpt() call");

        let gpt_data = gpt.unwrap();
        assert_eq!(
            gpt_data.gpt_disk.partitions().len(),
            1,
            "Should have one partition"
        );

        let (_, partition) = gpt_data.gpt_disk.partitions().iter().next().unwrap();
        assert_eq!(
            partition.name, "gpt_test_partition",
            "Partition name should match"
        );

        // Verify GPT is now cached.
        assert!(
            cosi.partitioning_info.is_some(),
            "GPT should be cached after first call"
        );

        // Second call should return cached GPT (no re-parsing).
        let gpt_result2 = cosi.partitioning_info();
        assert!(gpt_result2.is_ok(), "Second gpt() call should succeed");
        assert!(
            gpt_result2.unwrap().is_some(),
            "Cached GPT should still be present"
        );
    }

    /// Tests [`Cosi::gpt`] error handling for COSI versions below 1.2.
    ///
    /// GPT partition data is only available in COSI >= 1.2. This test verifies
    /// that calling `gpt()` on a COSI 1.0 or 1.1 file returns an appropriate error
    /// indicating the version requirement.
    #[test]
    fn test_cosi_gpt_version_too_low() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Create a COSI instance with version 1.0 (below 1.2).
        let mut cosi = Cosi {
            source: Url::parse("mock://").unwrap(),
            metadata: CosiMetadata {
                version: KnownMetadataVersion::V1_0.as_version(),
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images: vec![],
                bootloader: None,
                disk: None,
                compression: Default::default(),
            },
            reader: FileReader::Buffer(Cursor::new(Vec::<u8>::new())),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            entries: CosiEntries::default(),
            partitioning_info: None,
        };

        // Calling gpt() on COSI < 1.2 should fail.
        let result = cosi.partitioning_info();
        assert!(result.is_err(), "gpt() should fail for COSI < 1.2");

        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("below 1.2"),
            "Error should mention version requirement: {}",
            err_msg
        );

        cosi.metadata.version = KnownMetadataVersion::V1_1.as_version();
        let result2 = cosi.partitioning_info();
        assert!(result2.is_err(), "gpt() should fail for COSI 1.1 as well");
        let err_msg2 = result2.unwrap_err().to_string();
        assert!(
            err_msg2.contains("below 1.2"),
            "Error should mention version requirement: {}",
            err_msg2
        );
    }
}
