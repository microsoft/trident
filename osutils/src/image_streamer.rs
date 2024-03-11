use std::{
    fs::File,
    io::{self, BufWriter, Read},
    path::Path,
};

use anyhow::{bail, Context, Error};
use log::info;

use crate::hashing_reader::HashingReader;

pub fn stream_zstd(
    mut reader: HashingReader<Box<dyn Read>>,
    destination_path: &Path,
    destination_size: Option<u64>,
) -> Result<(String, u64), Error> {
    // Instantiate decoder for ZSTD stream
    let mut decoder = zstd::stream::read::Decoder::new(&mut reader)?;

    // Open the partition for writing.
    let file = File::options()
        .write(true)
        .open(destination_path)
        .context(format!("Failed to open '{}'", destination_path.display()))?;

    // Buffer small writes to the disk, ensuring we write blocks of at least 4MB.
    let mut file = BufWriter::with_capacity(4 << 20, file);

    // Decompress the image and write it to the block device. If destination is a block device and
    // destination_size is provided, ensure that no more than destination_size bytes are written
    let bytes_copied = match destination_size {
        Some(size) => {
            io::copy(&mut (&mut decoder).take(size), &mut file).context("Failed to copy image")?
        }
        None => io::copy(&mut decoder, &mut file).context("Failed to copy image")?,
    };

    info!(
        "Copied {} bytes to {}",
        bytes_copied,
        destination_path.display()
    );

    file.into_inner()
        .context("Failed to flush")?
        .sync_all()
        .context("Failed to sync")?;

    // Attempt to read an additional byte from the stream to see whether the whole image was
    // consumed.
    if decoder.read(&mut [0])? != 0 {
        bail!("Image is larger than destination ({} bytes already copied, however additional bytes remaining)", bytes_copied);
    }

    let computed_sha256 = &reader.hash();

    Ok((computed_sha256.to_string(), bytes_copied)) // Return both values as a tuple
}
