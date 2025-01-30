use std::{fs::File, io::Read, path::Path};

use anyhow::Error;

use crate::{hashing_reader::HashingReader256, image_streamer};

pub fn stream_zstd(image: &Path, destination: &Path) -> Result<(), Error> {
    let stream: Box<dyn Read> = Box::new(File::open(image)?);
    let reader = HashingReader256::new(stream);
    image_streamer::stream_zstd(reader, destination)?;

    Ok(())
}
