use std::{
    collections::HashMap,
    io::{Read, Seek},
    iter,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, trace};
use tar::Archive;
use url::Url;

use trident_api::{
    config::{ImageSha384, OsImage},
    primitives::hash::Sha384Hash,
};

use crate::io_utils::{
    file_reader::FileReader,
    hashing_reader::{HashingReader, HashingReader384},
};

mod metadata;

use metadata::{CosiMetadata, CosiMetadataVersion, ImageFile, MetadataVersion};

use super::{OsImageFile, OsImageFileSystem, OsImageVerityHash};

/// Path to the COSI metadata file. Part of the COSI specification.
const COSI_METADATA_PATH: &str = "metadata.json";

/// Size of a tar block in bytes.
const TAR_BLOCK_SIZE: u64 = 512;

/// Top-level COSI file representation.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct Cosi {
    pub source: Url,
    pub metadata: CosiMetadata,
    pub metadata_sha384: Sha384Hash,
    pub host_configuration_template: Option<Vec<u8>>,
    reader: FileReader,
}

/// Entry inside the COSI file.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq)]
struct CosiEntry {
    offset: u64,
    size: u64,
}

impl Cosi {
    /// Creates a new COSI file instance from the given source URL.
    pub(super) fn new(source: &OsImage, timeout: Duration) -> Result<Self, Error> {
        trace!("Scanning COSI file from '{}'", source.url);

        // Create a new COSI reader factory. This will let us cleverly build
        // readers for the COSI file regardless of its location.
        let cosi_reader =
            FileReader::new(&source.url, timeout).context("Failed to create COSI reader.")?;

        // First, attempt to read the metadata with offsets from the metadata.
        // This will only fail if there was an actual IO error when reading the
        // file. If the the full entry map cannot be obtained from the metadata,
        // this will return None.
        let (metadata, sha384) = if let Some(result) =
            read_cosi_metadata_from_tar_archive(&cosi_reader, source.sha384.clone())
                .context("Failed to read COSI metadata with offsets.")?
        {
            log::debug!("COSI metadata successfully read with relative offsets.");
            result
        } else {
            log::debug!(
                "COSI metadata does not contain relative offsets; falling back to full scan"
            );
            // If that didn't work, fallback to full scan. Scan all entries in
            // the COSI file by seeking to all headers in the file.
            let entries = read_entries_from_tar_archive(cosi_reader.reader()?)?;
            trace!("Collected {} COSI entries", entries.len());

            let (metadata, sha384) =
                read_cosi_metadata_and_validate(&cosi_reader, &entries, source.sha384.clone())
                    .context("Failed to read COSI file metadata.")?;

            (metadata, sha384)
        };

        let host_configuration_template =
            if let Some(ref file) = metadata.host_configuration_template {
                let mut contents = Vec::new();
                let mut reader = HashingReader384::new(
                    cosi_reader.section_reader(file.entry.offset, file.entry.size)?,
                );
                reader.read_to_end(&mut contents)?;

                if file.sha384 != reader.hash() {
                    bail!("COSI host configuration template hash does not match expected hash");
                }

                Some(contents)
            } else {
                None
            };

        // Create a new COSI instance.
        Ok(Cosi {
            metadata,
            source: source.url.clone(),
            reader: cosi_reader,
            metadata_sha384: sha384,
            host_configuration_template,
        })
    }

    /// Returns the ESP filesystem image.
    pub(super) fn esp_filesystem(&self) -> Result<OsImageFileSystem<'_>, Error> {
        self.metadata
            .get_esp_filesystem()
            .map(|image| cosi_image_to_os_image_filesystem(&self.reader, image))
    }

    /// Returns an iterator of available mount points in the COSI file.
    pub(super) fn available_mount_points(&self) -> impl Iterator<Item = &Path> {
        self.metadata
            .get_regular_filesystems()
            .map(|image| image.mount_point.as_path())
    }

    /// Returns an iterator over all images that are NOT the ESP filesystem image.
    pub(super) fn filesystems(&self) -> impl Iterator<Item = OsImageFileSystem<'_>> {
        self.metadata
            .get_regular_filesystems()
            .map(|image| cosi_image_to_os_image_filesystem(&self.reader, image))
    }
}

/// Converts a COSI metadata Image to an OsImageFileSystem.
fn cosi_image_to_os_image_filesystem<'a>(
    cosi_reader: &'a FileReader,
    image: &metadata::Image,
) -> OsImageFileSystem<'a> {
    // Make an early copy so the borrow checker knows that we are not keeping a
    // reference to the original image. Calling as_rer().map() on image.verity
    // seems to tell the borrow checker that we are keeping a reference to the
    // original image, even if we only clone stuff and don't keep a reference to
    // the original image.
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
            reader: {
                Box::new(move || {
                    cosi_reader.section_reader(image.file.entry.offset, image.file.entry.size)
                })
            },
        },
        verity: image.verity.map(|verity| OsImageVerityHash {
            hash_image_file: OsImageFile {
                compressed_size: verity.file.compressed_size,
                sha384: verity.file.sha384,
                uncompressed_size: verity.file.uncompressed_size,
                reader: {
                    Box::new(move || {
                        cosi_reader.section_reader(verity.file.entry.offset, verity.file.entry.size)
                    })
                },
            },
            roothash: verity.roothash,
        }),
    }
}

/// Reads JUST the metadata from the given COSI file.
///
/// Returns the metadata populated with entries, and its SHA-384 hash.
///
/// This will only fail if there was an actual IO error when reading the file.
/// If the the full entry map cannot be obtained from the metadata, this will
/// return None.
///
/// A None will happen when:
/// - The metadata is not the first entry in the tar archive.
/// - Not all file references include an offset.
fn read_cosi_metadata_from_tar_archive(
    cosi_reader: &FileReader,
    expected_sha384: ImageSha384,
) -> Result<Option<(CosiMetadata, Sha384Hash)>, Error> {
    // Create a tar archive reader.
    let mut archive = Archive::new(cosi_reader.reader()?);
    // Get the first entry in the archive.
    let first_entry = archive
        .entries()
        .context("Failed to read COSI file")?
        .next()
        .context("COSI file is empty")?
        .context("Failed to read first COSI entry")?;

    // Ensure that the first entry is the metadata.
    if first_entry
        .path()
        .context("Failed to read first COSI entry path")?
        != Path::new(COSI_METADATA_PATH)
    {
        // First entry is NOT the metadata, so we cannot have relative offsets.
        log::warn!(
            "Non-compliant COSI file: metadata is not the first entry; relative offsets will not be supported"
        );
        return Ok(None);
    }

    let a = CosiEntry {
        offset: first_entry.raw_file_position(),
        size: first_entry.size(),
    };

    read_cosi_metadata_with_offsets(cosi_reader, &a, expected_sha384)
}

/// Reads the metadata from the COSI file using the given entry offsets.
/// Returns the metadata populated with entries, and its SHA-384 hash.
///
/// Inner implementation of `read_cosi_metadata_from_tar_archive`.
fn read_cosi_metadata_with_offsets(
    cosi_reader: &FileReader,
    metadata_entry: &CosiEntry,
    expected_sha384: ImageSha384,
) -> Result<Option<(CosiMetadata, Sha384Hash)>, Error> {
    // Read the metadata from the first entry.
    let (mut metadata, actual_sha384) =
        read_cosi_metadata(cosi_reader, metadata_entry, expected_sha384)?;

    // Store the size of the full cosi file to validate offsets later.
    let cosi_file_size = cosi_reader
        .size()
        .context("Failed to get size of COSI file")?;

    // Get an iterator over all image files in the metadata.
    let image_files = metadata
        .images
        // Iterate over all images
        .iter_mut()
        // Get a flattened iterator over the image files and their verity files
        // (if any)
        .flat_map(|fs| {
            iter::once(&mut fs.file).chain(fs.verity.as_mut().map(|verity| &mut verity.file))
        })
        .collect::<Vec<_>>();

    // Check if at least one image has an offset defined in the metadata. If
    // not, we assume this is an older COSI and needs full-file scanning.
    if !image_files.iter().any(|img| img.offset.is_some()) {
        // No images have relative offsets, we cannot build the full list of
        // entries.
        log::trace!("No images in the COSI metadata have relative offsets.");
        return Ok(None);
    }

    // Calculate the address of the first byte after the metadata.
    let address_after_metadata = metadata_entry.offset + metadata_entry.size;

    // Now calculate the start of the second header. This is the first multiple
    // of 512 bytes (tar block boundary) after the end of the metadata. If the
    // metadata ends exactly at a TAR block boundary, the start of the second
    // header is the same as the address immediately after the metadata. In
    // other words, if address_after_metadata % TAR_BLOCK_SIZE == 0, then
    // start_of_second_header == address_after_metadata. In all other cases, we
    // round up to the next TAR block boundary.
    //
    // This can be done without the IF, but this is more readable.
    let start_of_second_header = if address_after_metadata % TAR_BLOCK_SIZE == 0 {
        // Metadata ends exactly at a TAR block boundary. The start of the
        // second header is the same as the address immediately after the
        // metadata.
        address_after_metadata
    } else {
        // Metadata does not end at a TAR block boundary. Round up to the next
        // TAR block boundary by adding the difference between the next TAR
        // block boundary and the current address.
        address_after_metadata + (TAR_BLOCK_SIZE - (address_after_metadata % TAR_BLOCK_SIZE))
    };

    for img in image_files {
        let Some(relative_offset) = img.offset else {
            // Image does not have a relative offset, we cannot build the full
            // list of entries.
            log::error!(
                "Image '{}' in the COSI metadata does not have a relative offset.",
                img.path.display()
            );
            return Ok(None);
        };

        // Since offsets are relative to a tar block boundary, they must also be aligned to a tar block boundary.
        if relative_offset % TAR_BLOCK_SIZE != 0 {
            log::error!(
                "COSI metadata specifies a relative offset of {} for image at path '{}', which is not aligned to a tar block boundary ({} bytes)",
                relative_offset,
                img.path.display(),
                TAR_BLOCK_SIZE
            );
            return Ok(None);
        }

        img.entry = CosiEntry {
            offset: start_of_second_header + relative_offset,
            size: img.compressed_size,
        };

        // Now, ensure that the absolute offset + size does not exceed the file size.
        if img.entry.offset + img.entry.size > cosi_file_size {
            log::error!(
                "COSI metadata specifies an image at path '{}' with offset {} and size {}, which exceeds the total COSI file size of {} bytes",
                img.path.display(),
                img.entry.offset,
                img.entry.size,
                cosi_file_size
            );
            return Ok(None);
        }

        trace!(
            "Computed absolute offset for image '{}' at {} [{} bytes]",
            img.path.display(),
            img.entry.offset,
            img.entry.size
        );
    }

    if let Some(hc_file) = metadata.host_configuration_template.as_mut() {
        trace!(
            "Processing Host Configuration template at path for absolute offset '{}'",
            hc_file.path.display()
        );

        let Some(relative_offset) = hc_file.offset else {
            // Host Configuration template does not have a relative offset,
            // we cannot build the full list of entries.
            log::error!(
                "Host Configuration template '{}' in the COSI metadata does not have a relative offset.",
                hc_file.path.display()
            );
            return Ok(None);
        };

        hc_file.entry = CosiEntry {
            offset: start_of_second_header + relative_offset,
            size: hc_file.size,
        };

        // Now, ensure that the absolute offset + size does not exceed the file size.
        if hc_file.entry.offset + hc_file.entry.size > cosi_file_size {
            log::error!(
                "COSI metadata specifies an image at path '{}' with offset {} and size {}, which exceeds the total COSI file size of {} bytes",
                hc_file.path.display(),
                hc_file.entry.offset,
                hc_file.entry.size,
                cosi_file_size
            );
            return Ok(None);
        }
    }

    debug!(
        "Successfully read COSI metadata [v{}.{}] with relative offsets",
        metadata.version.major, metadata.version.minor
    );

    Ok(Some((metadata, actual_sha384)))
}

/// Reads all entries from the given COSI tar archive.
fn read_entries_from_tar_archive<R: Read + Seek>(
    cosi_reader: R,
) -> Result<HashMap<PathBuf, CosiEntry>, Error> {
    Archive::new(cosi_reader)
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
        .collect::<Result<Vec<_>, _>>()
        .context("Failed to read COSI file entries")?
        .into_iter()
        .map(|entry| {
            let entry = (
                {
                    let path = entry.path().context("Failed to read entry path")?;
                    let path = path.strip_prefix("./").unwrap_or(&path).to_path_buf();
                    path
                },
                CosiEntry {
                    offset: entry.raw_file_position(),
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
        })
        .collect::<Result<HashMap<_, _>, Error>>()
        .context("Failed to process COSI entries")
}

/// Retrieves the COSI metadata from the given COSI file.
///
/// It also:
/// - Validates the metadata version.
/// - Ensures that all images defined in the metadata are present in the COSI file.
/// - Populates metadata with the actual content location of the images.
fn read_cosi_metadata_and_validate(
    cosi_reader: &FileReader,
    entries: &HashMap<PathBuf, CosiEntry>,
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

    let (mut metadata, actual_sha384) =
        read_cosi_metadata(cosi_reader, metadata_location, expected_sha384)?;

    // Populate the metadata with the actual content location of the images.
    populate_cosi_metadata_content_location(entries, &mut metadata)?;

    debug!(
        "Successfully read COSI metadata [v{}.{}]",
        metadata.version.major, metadata.version.minor
    );

    Ok((metadata, actual_sha384))
}

/// Retrieves the COSI metadata from the given COSI file.
///
/// It also:
/// - Validates the metadata version.
/// - Ensures that all images defined in the metadata are present in the COSI file.
/// - Populates metadata with the actual content location of the images.
fn read_cosi_metadata(
    cosi_reader: &FileReader,
    entry: &CosiEntry,
    expected_sha384: ImageSha384,
) -> Result<(CosiMetadata, Sha384Hash), Error> {
    let mut metadata_reader = HashingReader384::new(
        cosi_reader
            .section_reader(entry.offset, entry.size)
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

/// Populates the metadata with the actual content location of the images.
/// As a side effect, this function also validates that all images defined in the
/// metadata are present in the COSI file, and that their basic properties match.
fn populate_cosi_metadata_content_location(
    entries: &HashMap<PathBuf, CosiEntry>,
    metadata: &mut CosiMetadata,
) -> Result<(), Error> {
    let find_entry = |img: &ImageFile| {
        let Some(entry) = entries.get(&img.path) else {
            bail!(
                "COSI metadata contains an entry for a filesystem image at '{}', but the entry was not found in the COSI file",
                img.path.display()
            );
        };

        ensure!(entry.size == img.compressed_size,
                "COSI metadata specifies a compressed size of {} bytes for the filesystem image at '{}', but the actual entry size is {} bytes",
                img.compressed_size,
                img.path.display(),
                entry.size
        );

        Ok(*entry)
    };

    // Ensure that all images defined in the metadata are present in the COSI file.
    for image in metadata.images.iter_mut() {
        trace!(
            "Looking for entry for image mounted at '{}'",
            image.mount_point.display()
        );
        image.file.entry = find_entry(&image.file).with_context(|| {
            format!(
                "Failed to find entry for image mounted at '{}'",
                image.mount_point.display()
            )
        })?;

        if let Some(verity) = image.verity.as_mut() {
            verity.file.entry = find_entry(&verity.file).with_context(|| {
                format!(
                    "Failed to find entry for verity hash of image mounted at '{}'",
                    image.mount_point.display()
                )
            })?;
        }
    }

    if let Some(host_config) = metadata.host_configuration_template.as_mut() {
        trace!(
            "Looking for entry for Host Configuration template at '{}'",
            host_config.path.display()
        );
        let Some(entry) = entries.get(&host_config.path) else {
            bail!(
                "COSI metadata contains an entry for a Host Configuration template at '{}', but the entry was not found in the COSI file",
                host_config.path.display()
            );
        };

        host_config.entry = *entry;
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

    use crate::osimage::OsImageFileSystemType;

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

    /// Generate a sample COSI v1.3 metadata file from the given entries.
    ///
    /// An entry is a tuple of (path, data).
    ///
    /// Since this is a spec, hard-coding a string made by following the spec
    /// means we also check compliance. It also prevents us from having to
    /// implement Serialize for the metadata only for testing.
    fn generate_sample_metadata_v1_3<'a>(
        images: impl Iterator<Item = (&'static str, u64, &'a str, u64)>,
    ) -> String {
        let images = images
            .map(|(path, size, hash, relative_offset)| {
                format!(
                    indoc::indoc! {r#"
                        {{
                            "image": {{
                                "path": "{}",
                                "compressedSize": {},
                                "uncompressedSize": 2048,
                                "sha384": "{sha384}",
                                "offset": {relative_offset}
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
                    relative_offset = relative_offset,
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");

        format!(
            indoc::indoc! {r#"
                {{
                    "version": "1.3",
                    "id": "{id}",
                    "osArch": "x86_64",
                    "osRelease": "",
                    "images": [
                        {}
                    ],
                    "bootloader": {{
                        "type": "grub"
                    }}
                }}
            "#},
            images,
            id = Uuid::new_v4(),
        )
    }

    /// Asserts that all CosiEntries in the metadata are populated.
    /// Returns the total number of entries found.
    fn assert_entries_populated(metadata: &CosiMetadata) -> usize {
        let mut total_entries = 0;
        for image in metadata.images.iter() {
            total_entries += 1;
            assert!(
                image.file.entry.size > 0,
                "Image file entry '{}' size not populated",
                image.file.path.display()
            );
            assert!(
                image.file.entry.offset > 0,
                "Image file entry '{}' offset not populated",
                image.file.path.display()
            );

            if let Some(verity) = image.verity.as_ref() {
                total_entries += 1;
                assert!(
                    verity.file.entry.size > 0,
                    "Verity file entry '{}' size not populated",
                    verity.file.path.display()
                );
                assert!(
                    verity.file.entry.offset > 0,
                    "Verity file entry '{}' offset not populated",
                    verity.file.path.display()
                );
            }
        }

        if let Some(host_config) = metadata.host_configuration_template.as_ref() {
            total_entries += 1;
            assert!(
                host_config.entry.size > 0,
                "Host Configuration template entry size not populated"
            );
            assert!(
                host_config.entry.offset > 0,
                "Host Configuration template entry offset not populated"
            );
        }

        total_entries
    }

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
        let entries = super::read_entries_from_tar_archive(Cursor::new(&cosi_file)).unwrap();

        // Check the entries
        assert_eq!(
            entries.len(),
            sample_data.len(),
            " Incorrect number of entries"
        );

        // Check that each entry matches the expected data.
        for (path, data) in sample_data.iter() {
            let entry = entries.get(Path::new(path)).unwrap();
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
        .collect::<HashMap<_, _>>();

        // Read the metadata.
        let metadata = read_cosi_metadata_and_validate(
            &cosi_reader,
            &entries,
            ImageSha384::Checksum(metadata_sha384.into()),
        )
        .unwrap()
        .0;

        // Now check that the images in the metadata have the correct entries.
        for (image, (path, offset, size)) in metadata.images.iter().zip(image_paths.iter()) {
            assert_eq!(image.file.path, Path::new(path), "Incorrect image path",);
            assert_eq!(image.file.entry.offset, *offset, "Incorrect image offset");
            assert_eq!(image.file.entry.size, *size, "Incorrect image size");
        }
    }

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

        let total_entries = assert_entries_populated(&cosi.metadata);

        assert_eq!(
            total_entries,
            mock_images.len(),
            "Incorrect number of entries"
        );

        assert_eq!(url, cosi.source, "Incorrect source URL in COSI instance")
    }

    #[test]
    fn test_cosi_image_to_os_image_filesystem() {
        let data = "some data";
        let reader = FileReader::Buffer(Cursor::new(data.as_bytes().to_vec()));
        let mut cosi_img = Image {
            file: ImageFile {
                path: PathBuf::from("some/path"),
                compressed_size: data.len() as u64,
                uncompressed_size: data.len() as u64,
                sha384: Sha384Hash::from(format!("{:x}", Sha384::digest(data.as_bytes()))),
                entry: CosiEntry {
                    offset: 0,
                    size: data.len() as u64,
                },
                offset: None,
            },
            mount_point: PathBuf::from("/some/mount/point"),
            fs_type: OsImageFileSystemType::Ext4,
            fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
            part_type: DiscoverablePartitionType::LinuxGeneric,
            verity: None,
        };
        let os_fs = cosi_image_to_os_image_filesystem(&reader, &cosi_img);

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

        let mut read_data = String::new();
        os_fs
            .image_file
            .reader()
            .unwrap()
            .read_to_string(&mut read_data)
            .unwrap();
        assert_eq!(read_data, data);

        // Now test with verity.
        let root_hash = "some-root-hash-1234";
        let verity_data = "some data";
        let reader = FileReader::Buffer(Cursor::new(verity_data.as_bytes().to_vec()));
        cosi_img.verity = Some(VerityMetadata {
            file: ImageFile {
                path: PathBuf::from("some/verity/path"),
                compressed_size: verity_data.len() as u64,
                uncompressed_size: verity_data.len() as u64,
                sha384: Sha384Hash::from(format!("{:x}", Sha384::digest(verity_data.as_bytes()))),
                entry: CosiEntry {
                    offset: 0,
                    size: verity_data.len() as u64,
                },
                offset: None,
            },
            roothash: root_hash.to_string(),
        });

        let os_fs = cosi_image_to_os_image_filesystem(&reader, &cosi_img);

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

        let mut read_data = String::new();
        os_fs_verity
            .hash_image_file
            .reader()
            .unwrap()
            .read_to_string(&mut read_data)
            .unwrap();

        assert_eq!(read_data, verity_data);
    }

    fn sample_verity_cosi_file(
        mock_images: &[(&str, OsImageFileSystemType, DiscoverablePartitionType, &str)],
    ) -> Cosi {
        // Reader data
        let mut data = Cursor::new(Vec::<u8>::new());
        let mut images = Vec::new();

        for (mntpt, fs_type, pt_type, file_data) in mock_images.iter() {
            let filename = Uuid::new_v4().to_string();
            let entry = CosiEntry {
                offset: data.position(),
                size: file_data.len() as u64,
            };

            data.write_all(file_data.as_bytes()).unwrap();

            images.push(Image {
                file: ImageFile {
                    path: PathBuf::from(filename),
                    compressed_size: file_data.len() as u64,
                    uncompressed_size: file_data.len() as u64,
                    sha384: Sha384Hash::from(format!("{:x}", Sha384::digest(file_data.as_bytes()))),
                    entry,
                    offset: None,
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
                version: MetadataVersion { major: 1, minor: 0 },
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images,
                bootloader: None,
                host_configuration_template: None,
            },
            reader: FileReader::Buffer(data),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            host_configuration_template: None,
        }
    }

    #[test]
    fn test_esp_filesystem() {
        // Test with an empty COSI file.
        let empty = Cosi {
            source: Url::parse("mock://").unwrap(),
            metadata: CosiMetadata {
                version: MetadataVersion { major: 1, minor: 0 },
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                images: vec![],
                os_packages: None,
                bootloader: None,
                host_configuration_template: None,
            },
            reader: FileReader::Buffer(Cursor::new(Vec::<u8>::new())),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
            host_configuration_template: None,
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
            &cosi.reader,
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

        let read_data = {
            let mut data = String::new();
            esp.image_file
                .reader()
                .unwrap()
                .read_to_string(&mut data)
                .unwrap();
            data
        };

        assert_eq!(read_data, mock_images[0].3);
    }

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
            .map(|img| cosi_image_to_os_image_filesystem(&cosi.reader, img))
            .collect::<Vec<_>>();
        let img_data = mock_images
            .iter()
            .skip(1)
            .map(|(_, _, _, data)| *data)
            .collect::<Vec<_>>();
        assert_eq!(expected.len(), img_data.len());
        assert_eq!(filesystems.len(), expected.len());

        for (fs, (expected_fs, expected_data)) in filesystems
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

            let read_data = {
                let mut data = String::new();
                fs.image_file
                    .reader()
                    .unwrap()
                    .read_to_string(&mut data)
                    .unwrap();
                data
            };

            assert_eq!(read_data, expected_data);
        }
    }

    #[test]
    fn test_cosi_with_relative_offsets() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let mut offset = 0u64;
        let mock_images_good = [
            ("some/image/path/A", "this is some example data [A]"),
            ("some/image/path/B", "this is some example data [B]"),
            ("some/image/path/C", "this is some example data [C]"),
        ]
        .into_iter()
        .map(|(path, data)| {
            let size = data.len() as u64;
            let hash = format!("{:x}", Sha384::digest(data.as_bytes()));
            let relative_offset = offset;
            offset += TAR_BLOCK_SIZE + size;
            if offset % TAR_BLOCK_SIZE != 0 {
                offset += TAR_BLOCK_SIZE - (offset % TAR_BLOCK_SIZE);
            }
            (path, data, size, hash, relative_offset)
        })
        .collect::<Vec<_>>();

        // Helper function to test reading COSI metadata with offsets.
        fn test_read_cosi_metadata_with_offsets(
            mock_images: &Vec<(&'static str, &'static str, u64, String, u64)>,
        ) -> Result<Option<(CosiMetadata, Sha384Hash)>, Error> {
            // Generate a sample COSI metadata file.
            let sample_metadata = generate_sample_metadata_v1_3(mock_images.iter().map(
                |(path, _, size, hash, relative_offset)| {
                    (*path, *size, hash.as_str(), *relative_offset)
                },
            ))
            .as_bytes()
            .to_vec();

            // Generate a sample COSI file.
            let cosi_file = FileReader::Buffer(Cursor::new(generate_test_tarball(
                [(COSI_METADATA_PATH, sample_metadata.as_slice())]
                    .into_iter()
                    .chain(
                        mock_images
                            .iter()
                            .map(|(path, data, _, _, _)| (*path, data.as_bytes())),
                    ),
            )));

            // Create a FileReader from the sample metadata.
            read_cosi_metadata_from_tar_archive(&cosi_file, ImageSha384::Ignored)
        }

        // First a good pass to ensure the test function works.
        let (metadata, _) = test_read_cosi_metadata_with_offsets(&mock_images_good)
            .unwrap()
            .unwrap();
        let entries = assert_entries_populated(&metadata);
        assert_eq!(
            entries,
            mock_images_good.len(),
            "Incorrect number of entries with relative offsets"
        );

        // Now let's make the file small so all offsets are out of bounds.
        let mut mock_images_1 = mock_images_good.clone();
        for img in mock_images_1.iter_mut() {
            img.4 = 0x8_0000; // Set an out-of-bounds offset. This is a multiple of TAR_BLOCK_SIZE.
        }
        let val = test_read_cosi_metadata_with_offsets(&mock_images_1).unwrap();
        assert!(val.is_none(), "Expected None due to out-of-bounds offsets");

        // Now let's make the size of the last image too large.
        let mut mock_images_2 = mock_images_good.clone();
        if let Some(last) = mock_images_2.last_mut() {
            last.2 = 0x8_0000; // Set a too-large size.
        }
        let val = test_read_cosi_metadata_with_offsets(&mock_images_2).unwrap();
        assert!(val.is_none(), "Expected None due to too-large size");

        // Now let's make the last offset unaligned.
        let mut mock_images_3 = mock_images_good.clone();
        if let Some(last) = mock_images_3.last_mut() {
            last.4 += 1; // Set an unaligned offset.
        }
        let val = test_read_cosi_metadata_with_offsets(&mock_images_3).unwrap();
        assert!(val.is_none(), "Expected None due to unaligned offset");
    }
}
