use anyhow::{bail, Context, Error};
use log::info;
use std::{
    fs::{self},
    io::{self, BufWriter, Read},
};
use zstd;

use crate::modules::image::HashingReader;
use trident_api::{
    status::{BlockDeviceContents, BlockDeviceInfo, HostStatus},
    BlockDeviceId,
};

/// This function is called from image/mod.rs, to stream the bytes of an image onto a block device.
/// The func takes the following arguments:
/// 1. host_status: A mutable reference to HostStatus object, which is updated to communicate that
/// the block device is being written to.
/// 2. stream: A HashingReader instance, which wraps a stream of bytes.
/// 3. block_device: A BlockDeviceInfo instance, which contains information about the block device
/// that is being written to, e.g., its path.
/// 4. block_device_id: BlockDeviceId of the block device.
/// The func returns a tuple of (String, u64), where the first element is the SHA256 hash of the
/// stream, and the second element is the number of bytes written to the block device.
pub(super) fn stream_zstd_image(
    host_status: &mut HostStatus,
    mut stream: HashingReader<Box<dyn Read>>,
    block_device: &BlockDeviceInfo,
    block_device_id: &BlockDeviceId,
) -> Result<(String, u64), Error> {
    // Instantiate decoder for ZSTD stream
    let mut decoder = zstd::stream::read::Decoder::new(&mut stream)?;

    // Open the partition for writing.
    let file = fs::File::options()
        .write(true)
        .open(&block_device.path)
        .context(format!("Failed to open '{}'", block_device.path.display()))?;

    // Buffer small writes to the disk, ensuring we write blocks of at least 4MB.
    let mut file = BufWriter::with_capacity(4 << 20, file);

    // Mark the block device as having unknown contents in case the write operation is interrupted.
    super::set_host_status_block_device_contents(
        host_status,
        block_device_id,
        BlockDeviceContents::Unknown,
    )?;

    // Decompress the image and write it to the block device, making sure not to write past the end.
    let bytes_copied = io::copy(&mut (&mut decoder).take(block_device.size), &mut file)
        .context("Failed to copy image")?;

    info!(
        "Copied {} bytes to {}",
        bytes_copied,
        block_device.path.display()
    );

    file.into_inner()
        .context("Failed to flush")?
        .sync_all()
        .context("Failed to sync")?;

    // Attempt to read an additional byte from the stream to see whether the whole image was
    // consumed.
    if decoder.read(&mut [0])? != 0 {
        bail!("Image is larger than destination");
    }

    let computed_sha256 = &stream.hash();

    Ok((computed_sha256.to_string(), bytes_copied)) // Return both values as a tuple
}
