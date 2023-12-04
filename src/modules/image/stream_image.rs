use anyhow::{bail, Context, Error};
use log::{error, info};
use reqwest::{blocking::Response, StatusCode, Url};
use std::{
    fs::{self, File},
    io::{self, BufWriter, Read},
    time::Duration,
};
use zstd;

use crate::modules::image::HashingReader;
use trident_api::{
    config::Image,
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

/// Directly deploys images via stream_image.rs; returns error if image cannot be downloaded or
/// installed correctly. Takes in 5 arg-s:
/// 1. image_url: &Url, which is the URL of the image to be downloaded,
/// 2. image: &Image, which is the Image object from HostConfig,
/// 3. host_status: &mut HostStatus, which is the HostStatus object,
/// 4. block_device: &BlockDeviceInfo, which is the BlockDeviceInfo object,
/// 5. is_local: bool, which is a boolean indicating whether the image is a local file or not.
pub(super) fn deploy(
    image_url: Url,
    image: &Image,
    host_status: &mut HostStatus,
    block_device: &BlockDeviceInfo,
    is_local: bool,
) -> Result<(), Error> {
    // Check whether image_url is local; depending on result, create a boxed trait object for the
    // read stream
    let stream: Box<dyn Read> = if is_local {
        // For local files, open the file at the given path
        Box::new(File::open(image_url.path()).context(format!("Failed to open {}", image.url))?)
    } else {
        // For remote files, perform a blocking GET request
        exponential_backoff_get(&image_url, 5)?
    };

    // Initialize HashingReader instance on stream
    let stream = HashingReader::new(stream);
    info!("Calling Stream-Image sub-module to stream image to block device");
    // Stream image to block device
    let (computed_sha256, bytes_copied) =
        stream_zstd_image(host_status, stream, block_device, &image.target_id)
            .context(format!("Failed to stream image from {}", image.url))?;

    // Update HostStatus
    super::set_host_status_block_device_contents(
        host_status,
        &image.target_id,
        BlockDeviceContents::Image {
            sha256: computed_sha256.clone(),
            length: bytes_copied,
            url: image.url.clone(),
        },
    )?;

    // If SHA256 is ignored, log message and skip hash validation; otherwise, ensure computed
    // SHA256 matches SHA256 in HostConfig
    if image.sha256 == super::HASH_IGNORED {
        info!("Ignoring SHA256 for image from '{}'", image.url);
    } else if computed_sha256 != image.sha256 {
        bail!(
            "SHA256 mismatch for disk image {}: expected {}, got {}",
            image.url,
            image.sha256,
            computed_sha256
        );
    }

    Ok(())
}

/// Perform a GET request with retries and exponential backoff.
///
/// The function will do a GET request to the given URL and return the response
/// if the status code is OK.
///
/// `max_retries` is the number of *additional* attempts to make after the first.
/// Passing 0 will make a single attempt, passing 1 will make at most two attempts, etc.
///
/// The backoff is exponential, starting at 500ms and doubling each time, up to a maximum of 64s.
fn exponential_backoff_get(url: &Url, max_retries: u8) -> Result<Box<Response>, Error> {
    let mut counter = 0u8;
    let client = reqwest::blocking::ClientBuilder::new()
        .timeout(None)
        .build()
        .context("Failed to create HTTP client")?;
    loop {
        // Try to execute the GET request, if it works and we get a 200 OK,
        // return the response immediately. Otherwise, store the error and
        // continue the loop.
        let err: Error = match client.get(url.clone()).send().context("Failed to GET") {
            Ok(response) if matches!(response.status(), StatusCode::OK) => {
                // On success, exit exponential_backoff_get() by returning the response
                return Ok(Box::new(response));
            }
            // Otherwise store the error to report it later and continue the loop
            Ok(response) => anyhow::anyhow!("Failed to GET with status {}", response.status()),
            Err(e) => e,
        };

        // Check if we reached the limit
        if counter >= max_retries {
            return Err(err).context(format!(
                "Failed to GET from {url} after {} attempts.",
                (counter as u16) + 1, // change to u16 to avoid overflow
            ));
        }

        counter += 1;

        // Calculate exponential backoff.
        // After 64 seconds it becomes a bit ridiculous so we cap it there.
        // Because it's just a couple values it's easier to just hardcode it
        // than spending the extra ticks to calculate a power of 2.
        //
        // backoff = min( (0.5 * 2^(counter - 1)), 64) seconds
        // 0.5, 1, 2, 4, 8, 16, 32, 64, 64, 64, ...
        let backoff = Duration::from_millis(match counter {
            1 => 500,
            2 => 1000,
            3 => 2000,
            4 => 4000,
            5 => 8000,
            6 => 16000,
            7 => 32000,
            _ => 64000,
        });

        // Log the error and backoff
        error!(
            "Failed to GET from {}: {}. Retrying in {:4.1} seconds.",
            url,
            err,
            backoff.as_secs_f32()
        );
        std::thread::sleep(backoff);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    #[test]
    fn test_exponential_backoff() {
        let fake_url = Url::try_from("http://127.0.0.1:3030").unwrap();
        let start = Instant::now();
        let result = exponential_backoff_get(&fake_url, 2);
        let duration = start.elapsed();

        // 2 retries means 3 attempts:
        //(attempt) + 0.5s delay + (attempt) + 1s delay + (attempt)
        //
        // Because these are blocking calls the total duration should be at
        // least 1.5s
        assert!(
            duration >= Duration::from_millis(1500),
            "Duration was {:?}",
            duration
        );

        assert!(result.is_err(), "Expected error, got {:?}", result);

        let error = result.unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Failed to GET from {} after {} attempts.", fake_url, 3),
            "Error doesn't match expected"
        );
    }
}
