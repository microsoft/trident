use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{bail, ensure, Context, Error};
use log::{debug, trace};
use tar::Archive;
use url::Url;

use sysdefs::arch::SystemArchitecture;
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

/// Top-level COSI file representation.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct Cosi {
    source: Url,
    entries: HashMap<PathBuf, CosiEntry>,
    metadata: CosiMetadata,
    metadata_sha384: Sha384Hash,
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

        // Scan entries to find and read the metadata, process remaining based
        // on metadata
        let (entries, metadata, metadata_sha384) =
            read_cosi_with_metadata(&cosi_reader, source.sha384.clone())
                .context("Failed to read COSI file with metadata.")?;

        trace!("Collected {} COSI entries", entries.len());

        // Create a new COSI instance.
        Ok(Cosi {
            metadata,
            entries,
            source: source.url.clone(),
            reader: cosi_reader,
            metadata_sha384,
        })
    }

    /// Returns the source URL of the COSI file.
    pub(super) fn source(&self) -> &Url {
        &self.source
    }

    pub(super) fn is_uki(&self) -> bool {
        self.metadata.is_uki()
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

    /// Returns the architecture of the OS contained in the COSI file.
    pub(super) fn architecture(&self) -> SystemArchitecture {
        self.metadata.os_arch
    }

    pub(super) fn metadata_sha384(&self) -> Sha384Hash {
        self.metadata_sha384.clone()
    }
}

/// Converts a COSI metadata Image to an OsImageFileSystem.
fn cosi_image_to_os_image_filesystem<'a>(
    cosi_reader: &'a FileReader,
    image: &metadata::Image,
) -> OsImageFileSystem<'a> {
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

/// Finds the metadata entry in the COSI tar archive.
///
/// Returns the CosiEntry for the metadata.json file.
fn find_metadata_entry(cosi_reader: &FileReader) -> Result<CosiEntry, Error> {
    trace!("Scanning COSI archive for metadata entry");

    let mut archive = Archive::new(cosi_reader.reader()?);
    for entry in archive
        .entries_with_seek()
        .context("Failed to read COSI file")?
    {
        let entry = entry.context("Failed to read COSI file entry")?;
        let path = entry.path().context("Failed to read entry path")?;
        let path = path.strip_prefix("./").unwrap_or(&path);

        if path == Path::new(COSI_METADATA_PATH) {
            let metadata_entry = CosiEntry {
                offset: entry.raw_file_position(),
                size: entry.size(),
            };
            trace!(
                "Found COSI metadata at {} [{} bytes]",
                metadata_entry.offset,
                metadata_entry.size
            );
            return Ok(metadata_entry);
        }
    }

    bail!("COSI metadata not found in tar archive")
}

/// Reads and parses the COSI metadata from the given entry.
///
/// Returns the parsed metadata and its SHA384 hash.
fn parse_cosi_metadata(
    cosi_reader: &FileReader,
    metadata_entry: &CosiEntry,
    expected_sha384: ImageSha384,
) -> Result<(CosiMetadata, Sha384Hash), Error> {
    trace!("Reading and parsing COSI metadata");

    let mut metadata_reader = HashingReader384::new(
        cosi_reader
            .section_reader(metadata_entry.offset, metadata_entry.size)
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

/// Extracts the set of required file paths from the COSI metadata.
///
/// Returns a HashSet of all image and verity file paths referenced in the metadata.
fn extract_required_paths(metadata: &CosiMetadata) -> HashSet<PathBuf> {
    metadata
        .images
        .iter()
        .flat_map(|image| {
            let mut paths = vec![image.file.path.clone()];
            if let Some(ref verity) = image.verity {
                paths.push(verity.file.path.clone());
            }
            paths
        })
        .collect()
}

/// Collects only the required entries from the COSI tar archive.
///
/// Scans the archive and stops early once all required entries are found.
/// Returns a HashMap of all collected entries.
fn collect_required_entries(
    cosi_reader: &FileReader,
    mut required_paths: HashSet<PathBuf>,
) -> Result<HashMap<PathBuf, CosiEntry>, Error> {
    trace!(
        "Collecting {} required entries from COSI archive",
        required_paths.len()
    );

    let mut entries = HashMap::new();
    let mut archive = Archive::new(cosi_reader.reader()?);

    for entry in archive
        .entries_with_seek()
        .context("Failed to read COSI file")?
    {
        let entry = entry.context("Failed to read COSI file entry")?;
        let path = entry.path().context("Failed to read entry path")?;
        let path = path.strip_prefix("./").unwrap_or(&path);

        if required_paths.contains(path) {
            let cosi_entry = CosiEntry {
                offset: entry.raw_file_position(),
                size: entry.size(),
            };

            trace!(
                "Found required COSI entry '{}' at {} [{} bytes]",
                path.display(),
                cosi_entry.offset,
                cosi_entry.size
            );

            entries.insert(path.to_path_buf(), cosi_entry);
            required_paths.remove(path);

            // Early exit if we found all required entries
            if required_paths.is_empty() {
                trace!("All required entries found, stopping tar scan early");
                break;
            }
        }
    }

    // Verify we found all required entries
    if !required_paths.is_empty() {
        bail!(
            "Missing {} required entries from COSI file: {:?}",
            required_paths.len(),
            required_paths
        );
    }

    Ok(entries)
}

/// Reads COSI metadata and only the required entries from the tar archive.
///
/// This is an optimized version that:
/// 1. First scans the tar archive to find the metadata.json file
/// 2. Reads and parses the metadata to determine which image files are needed
/// 3. Scans the tar archive again, collecting only the required entries
/// 4. Stops scanning early once all required entries are found
///
/// This reduces I/O by avoiding reading the entire tar archive when only a subset
/// of entries is needed. This is particularly beneficial for large COSI files or
/// when reading from remote sources with high latency.
fn read_cosi_with_metadata(
    cosi_reader: &FileReader,
    expected_sha384: ImageSha384,
) -> Result<(HashMap<PathBuf, CosiEntry>, CosiMetadata, Sha384Hash), Error> {
    trace!("Optimized COSI reading: scanning for metadata first");

    // Step 1: Find the metadata entry in the tar archive
    let metadata_entry = find_metadata_entry(cosi_reader)?;

    // Step 2: Read and parse the metadata
    let (mut metadata, actual_sha384) =
        parse_cosi_metadata(cosi_reader, &metadata_entry, expected_sha384)?;

    // Step 3: Extract required paths from metadata
    let mut required_paths = extract_required_paths(&metadata);

    // Step 4: Collect the required entries (excluding metadata which we already have)
    required_paths.remove(Path::new(COSI_METADATA_PATH));
    let mut entries = collect_required_entries(cosi_reader, required_paths)?;

    // Add the metadata entry we already found
    entries.insert(PathBuf::from(COSI_METADATA_PATH), metadata_entry);

    // Step 5: Populate the metadata with the actual content location of the images
    populate_cosi_metadata_content_location(&entries, &mut metadata)?;

    Ok((entries, metadata, actual_sha384))
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
    use sysdefs::{osuuid::OsUuid, partition_types::DiscoverablePartitionType};
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
    fn test_find_metadata_entry() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let sample_metadata = "{ \"test\": \"metadata\" }";
        let cosi_file = generate_test_tarball(
            [(COSI_METADATA_PATH, sample_metadata.as_bytes())]
                .into_iter()
                .chain([("other/file.txt", "other data".as_bytes())]),
        );

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&cosi_file).unwrap();

        let cosi_reader = FileReader::new(
            &Url::from_file_path(temp_file.path()).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();

        let metadata_entry = super::find_metadata_entry(&cosi_reader).unwrap();
        assert_eq!(metadata_entry.size, sample_metadata.len() as u64);
    }

    #[test]
    fn test_find_metadata_entry_not_found() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Create a COSI file without metadata
        let cosi_file =
            generate_test_tarball([("other/file.txt", "other data".as_bytes())].into_iter());

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&cosi_file).unwrap();

        let cosi_reader = FileReader::new(
            &Url::from_file_path(temp_file.path()).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();

        let result = super::find_metadata_entry(&cosi_reader);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("metadata not found"));
    }

    #[test]
    fn test_extract_required_paths() {
        let metadata = CosiMetadata {
            version: MetadataVersion { major: 1, minor: 0 },
            id: Some(Uuid::new_v4()),
            os_arch: SystemArchitecture::Amd64,
            os_release: OsRelease::default(),
            os_packages: None,
            bootloader: None,
            images: vec![
                Image {
                    file: ImageFile {
                        path: PathBuf::from("image1.img"),
                        compressed_size: 1024,
                        uncompressed_size: 2048,
                        sha384: Sha384Hash::from("0".repeat(96)),
                        entry: CosiEntry::default(),
                    },
                    mount_point: PathBuf::from("/"),
                    fs_type: OsImageFileSystemType::Ext4,
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                    verity: None,
                },
                Image {
                    file: ImageFile {
                        path: PathBuf::from("image2.img"),
                        compressed_size: 1024,
                        uncompressed_size: 2048,
                        sha384: Sha384Hash::from("0".repeat(96)),
                        entry: CosiEntry::default(),
                    },
                    mount_point: PathBuf::from("/var"),
                    fs_type: OsImageFileSystemType::Ext4,
                    fs_uuid: OsUuid::Uuid(Uuid::new_v4()),
                    part_type: DiscoverablePartitionType::LinuxGeneric,
                    verity: Some(VerityMetadata {
                        file: ImageFile {
                            path: PathBuf::from("image2.verity"),
                            compressed_size: 512,
                            uncompressed_size: 512,
                            sha384: Sha384Hash::from("0".repeat(96)),
                            entry: CosiEntry::default(),
                        },
                        roothash: "hash123".to_string(),
                    }),
                },
            ],
        };

        let required_paths = super::extract_required_paths(&metadata);

        assert_eq!(required_paths.len(), 3);
        assert!(required_paths.contains(Path::new("image1.img")));
        assert!(required_paths.contains(Path::new("image2.img")));
        assert!(required_paths.contains(Path::new("image2.verity")));
    }

    #[test]
    fn test_collect_required_entries() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        // Create a COSI file with multiple entries
        let files = [
            ("file1.img", "data1"),
            ("file2.img", "data2"),
            ("file3.img", "data3"),
            ("unwanted.txt", "unwanted"),
        ];

        let cosi_file =
            generate_test_tarball(files.iter().map(|(path, data)| (*path, data.as_bytes())));

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&cosi_file).unwrap();

        let cosi_reader = FileReader::new(
            &Url::from_file_path(temp_file.path()).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();

        // We only want file1 and file2
        let required_paths = [PathBuf::from("file1.img"), PathBuf::from("file2.img")]
            .into_iter()
            .collect();

        let entries = super::collect_required_entries(&cosi_reader, required_paths).unwrap();

        assert_eq!(entries.len(), 2);
        assert!(entries.contains_key(Path::new("file1.img")));
        assert!(entries.contains_key(Path::new("file2.img")));
        assert!(!entries.contains_key(Path::new("file3.img")));
        assert!(!entries.contains_key(Path::new("unwanted.txt")));
    }

    #[test]
    fn test_collect_required_entries_missing() {
        env_logger::builder()
            .filter_level(log::LevelFilter::Trace)
            .is_test(true)
            .try_init()
            .ok();

        let cosi_file = generate_test_tarball([("file1.img", "data1".as_bytes())].into_iter());

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(&cosi_file).unwrap();

        let cosi_reader = FileReader::new(
            &Url::from_file_path(temp_file.path()).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();

        // Request files that don't exist
        let required_paths = [PathBuf::from("file1.img"), PathBuf::from("missing.img")]
            .into_iter()
            .collect();

        let result = super::collect_required_entries(&cosi_reader, required_paths);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Missing"));
    }

    #[test]
    fn test_read_cosi_with_metadata() {
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
        // The layout is (image_path_in_tarball, data).
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

        // Create a COSI reader from the temp file.
        let cosi_reader = FileReader::new(
            &Url::from_file_path(temp_file.path()).unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();

        // Read the metadata and entries using the optimized function.
        let (entries, metadata, _sha384) =
            read_cosi_with_metadata(&cosi_reader, ImageSha384::Ignored).unwrap();

        // Verify we got all entries (metadata + 3 images)
        assert_eq!(
            entries.len(),
            mock_images.len() + 1,
            "Incorrect number of entries"
        );

        // Now check that the images in the metadata have the correct entries.
        for (image, (path, data)) in metadata.images.iter().zip(mock_images.iter()) {
            assert_eq!(image.file.path, Path::new(path), "Incorrect image path");

            let entry = entries.get(&image.file.path).unwrap();
            assert_eq!(entry.size, data.len() as u64, "Incorrect image size");
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

        assert_eq!(
            cosi.entries.len(),
            mock_images.len() + 1,
            "Incorrect number of entries"
        );

        assert_eq!(&url, cosi.source(), "Incorrect source URL in COSI instance")
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
        let mut entries = HashMap::new();
        let mut images = Vec::new();

        for (mntpt, fs_type, pt_type, file_data) in mock_images.iter() {
            let filename = Uuid::new_v4().to_string();
            let entry = CosiEntry {
                offset: data.position(),
                size: file_data.len() as u64,
            };
            entries.insert(PathBuf::from(&filename), entry);

            data.write_all(file_data.as_bytes()).unwrap();

            images.push(Image {
                file: ImageFile {
                    path: PathBuf::from(filename),
                    compressed_size: file_data.len() as u64,
                    uncompressed_size: file_data.len() as u64,
                    sha384: Sha384Hash::from(format!("{:x}", Sha384::digest(file_data.as_bytes()))),
                    entry,
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
            entries,
            metadata: CosiMetadata {
                version: MetadataVersion { major: 1, minor: 0 },
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                os_packages: None,
                images,
                bootloader: None,
            },
            reader: FileReader::Buffer(data),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
        }
    }

    #[test]
    fn test_esp_filesystem() {
        // Test with an empty COSI file.
        let empty = Cosi {
            source: Url::parse("mock://").unwrap(),
            entries: HashMap::new(),
            metadata: CosiMetadata {
                version: MetadataVersion { major: 1, minor: 0 },
                id: Some(Uuid::new_v4()),
                os_arch: SystemArchitecture::Amd64,
                os_release: OsRelease::default(),
                images: vec![],
                os_packages: None,
                bootloader: None,
            },
            reader: FileReader::Buffer(Cursor::new(Vec::<u8>::new())),
            metadata_sha384: Sha384Hash::from("0".repeat(96)),
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
}
