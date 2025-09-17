use std::{
    fs::File,
    io::{self, BufReader, BufWriter, Read},
    path::Path,
    time::Instant,
};

use anyhow::{bail, Context, Error};
use log::{debug, trace};

use trident_api::primitives::bytes::ByteCount;

use crate::io_utils::hashing_reader::HashingReader;

/// Decompresses a ZSTD-compressed stream from a reader and writes the output
/// to the specified destination path.
///
/// Returns the hash of the compressed input stream.
pub fn stream_zstd<R>(mut reader: R, destination_path: &Path) -> Result<String, Error>
where
    R: Read + HashingReader,
{
    // Instantiate decoder for ZSTD stream
    let mut decoder = zstd::stream::read::Decoder::new(BufReader::new(&mut reader))?;

    copy_to_destination(&mut decoder, destination_path)?;

    Ok(reader.hash())
}

/// Streams data from a reader to a new file at the specificed destination path.
///
/// Returns the hash of the input stream.
pub fn stream_reader<R>(mut reader: R, destination_path: &Path) -> Result<String, Error>
where
    R: Read + HashingReader,
{
    copy_to_destination(&mut reader, destination_path)?;

    Ok(reader.hash())
}

fn copy_to_destination<R>(mut reader: R, destination_path: &Path) -> Result<(), Error>
where
    R: Read,
{
    // Open the partition for writing.
    let file = File::options()
        .write(true)
        .open(destination_path)
        .context(format!("Failed to open '{}'", destination_path.display()))?;

    // Buffer small writes to the disk, ensuring we write blocks of at least 4MB.
    let mut file = BufWriter::with_capacity(4 << 20, file);

    let t = Instant::now();

    // Decompress the image and write it to the block device
    let bytes_copied = io::copy(&mut reader, &mut file).context("Failed to copy image")?;

    trace!("Decompressed {} bytes.", bytes_copied);

    // Attempt to read an additional byte from the stream to see whether the whole image was
    // consumed.
    if reader.read(&mut [0])? != 0 {
        bail!("Image is larger than destination ({} bytes already copied, however additional bytes remaining)", bytes_copied);
    }

    trace!(
        "Syncing '{}' to finish writing image.",
        destination_path.display()
    );

    // Flush and sync the file to ensure all data is written to disk.
    file.into_inner()
        .context("Failed to flush")?
        .sync_all()
        .context("Failed to sync")?;

    debug!(
        "Copied {} [{}] to '{}'{} in {:.2} seconds",
        ByteCount::from(bytes_copied).to_human_readable_approx(),
        bytes_copied,
        destination_path.display(),
        // Try to resolve path, only print extra context if it differs.
        match destination_path.canonicalize() {
            Ok(real_path) if real_path != destination_path =>
                format!(" ('{}')", real_path.display()),
            _ => "".into(),
        },
        t.elapsed().as_secs_f32()
    );

    Ok(())
}
