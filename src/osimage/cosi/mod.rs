use std::{
    collections::{HashMap, HashSet},
    io::Read,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Error};
use log::trace;
use url::Url;

mod metadata;
mod reader;

use metadata::CosiMetadata;
use reader::CosiReader;

/// Path to the COSI metadata file. Part of the COSI specification.
const COSI_METADATA_PATH: &str = "metadata.json";

/// List of COSI versions that are accepted by this implementation.
const ACCEPTED_COSI_VERSIONS: [(u32, u32); 1] = [(1, 0)];

/// Top-level COSI file representation.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(super) struct Cosi {
    entries: HashMap<PathBuf, (u64, u64)>,
    metadata: CosiMetadata,
    source: Url,
}

impl Cosi {
    /// Creates a new COSI file instance from the given source URL.
    pub(super) fn new(source: &Url) -> Result<Self, Error> {
        trace!("Scanning COSI file from '{}'", source);
        let mut tar_reader = tar::Archive::new(CosiReader::new(source)?);

        let entries = tar_reader
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
                                Err(err) => format!("Failed to read entry path: {}", err),
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
                Ok((
                    {
                        let path = entry.path().context("Failed to read entry path")?;
                        let path = path.strip_prefix("./").unwrap_or(&path).to_path_buf();
                        trace!("Found COSI entry: {}", path.display());
                        path
                    },
                    (entry.raw_file_position(), entry.size()),
                ))
            })
            .collect::<Result<HashMap<_, _>, Error>>()
            .context("Failed to process COSI entries")?;

        trace!("Collected {} COSI entries", entries.len());

        // Return image reader to us
        let mut reader = tar_reader.into_inner();

        trace!("Reading COSI metadata from '{}'", COSI_METADATA_PATH);
        let raw_metadata = String::from_utf8(
            reader
                .read_range(
                    *entries
                        .get(Path::new(COSI_METADATA_PATH))
                        .context("COSI metadata not found")?,
                )
                .context("Failed to read COSI metadata")?,
        )
        .context("Failed to parse COSI metadata as utf8")?;

        trace!("Raw COSI metadata:\n{}", raw_metadata);

        let metadata =
            serde_json::from_str(&raw_metadata).context("Failed to parse COSI metadata")?;

        trace!("Successfully parsed COSI metadata");
        let cosi = Cosi {
            entries,
            metadata,
            source: source.clone(),
        };

        cosi.validate_metadata()?;

        Ok(cosi)
    }

    /// Returns the source URL of the COSI file.
    pub(super) fn source(&self) -> &Url {
        &self.source
    }

    /// Returns a list of all entries in the COSI file.
    #[allow(dead_code)]
    pub(super) fn entries(&self) -> impl Iterator<Item = &PathBuf> {
        self.entries.keys()
    }

    /// Returns a reader for the given COSI entry.
    #[allow(dead_code)]
    pub(super) fn entry_reader(&self, path: impl AsRef<Path>) -> Result<impl Read, Error> {
        let range = self
            .entries
            .get(path.as_ref())
            .with_context(|| format!("COSI entry not found: {}", path.as_ref().display()))?;
        self.reader()?
            .section_reader(*range)
            .context("Failed to create COSI section reader")
    }

    /// Returns an iterator over the available mount points provided by the COSI file.
    pub(super) fn available_mount_points(&self) -> impl Iterator<Item = &PathBuf> {
        self.metadata.images.iter().map(|image| &image.mount_point)
    }

    /// Returns the entry path for the given mount point.
    #[allow(dead_code)]
    pub(super) fn entry_for_mount_point(&self, mount_point: &Path) -> Option<&PathBuf> {
        self.metadata
            .images
            .iter()
            .find(|image| image.mount_point == mount_point)
            .map(|image| &image.image.path)
    }

    /// Returns a reader for the entry associated with the given mount point.
    #[allow(dead_code)]
    pub(super) fn entry_reader_for_mount_point(
        &self,
        mount_point: impl AsRef<Path>,
    ) -> Option<Result<impl Read + '_, Error>> {
        self.entry_for_mount_point(mount_point.as_ref())
            .map(|path| self.entry_reader(path))
    }

    /// Returns a COSI reader for the COSI file.
    fn reader(&self) -> Result<CosiReader, Error> {
        CosiReader::new(&self.source)
            .with_context(|| format!("Failed to create COSI reader for '{}'", self.source))
    }

    /// Validates the COSI metadata.
    fn validate_metadata(&self) -> Result<(), Error> {
        trace!("Validating COSI metadata");
        if !ACCEPTED_COSI_VERSIONS.iter().any(|(major, minor)| {
            self.metadata.version.major == *major && self.metadata.version.minor == *minor
        }) {
            bail!(
                "Unsupported COSI version: {}.{}",
                self.metadata.version.major,
                self.metadata.version.minor
            );
        }

        let mut mount_points = HashSet::new();
        for image in &self.metadata.images {
            if !mount_points.insert(&image.mount_point) {
                bail!("Duplicate mount point: '{}'", image.mount_point.display());
            }
        }

        // TODO: Validate image entries

        Ok(())
    }
}
