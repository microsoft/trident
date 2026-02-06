//! Simple binary to read and validate a COSI file.
//!
//! This tool takes a path to a COSI file and attempts to load it as an OsImage,
//! verifying that it can be read successfully.

use std::{path::PathBuf, time::Duration};

use anyhow::{Context, Result};
use clap::Parser;
use log::{info, LevelFilter};
use url::Url;

use trident::osimage::OsImage;
use trident_api::{
    config::{ImageSha384, OsImage as ConfigOsImage},
    error::TridentResultExt,
};

/// A simple tool to validate COSI files by loading them as OsImage objects.
#[derive(Parser, Debug)]
#[command(name = "cosi-reader")]
#[command(about = "Reads and validates a COSI file", long_about = None)]
struct Args {
    /// Path to the COSI file to read
    #[arg(value_name = "COSI_FILE")]
    cosi_path: String,

    /// Timeout in seconds for reading the COSI file
    #[arg(short, long, default_value = "30")]
    timeout: u64,

    /// Verbosity level (e.g., info, debug)
    #[arg(short, long, default_value = "info")]
    verbosity: LevelFilter,
}

fn main() -> Result<()> {
    let args = Args::parse();

    env_logger::Builder::new()
        .filter_level(args.verbosity)
        .init();

    // If the provided path starts with a scheme (e.g., "http://", "file://"),
    // treat it as a URL. Otherwise, treat it as a file path.
    let cosi_url = if let Ok(url) = Url::parse(&args.cosi_path) {
        url
    } else {
        // Convert the file path to a file:// URL
        let cosi_path = PathBuf::from(&args.cosi_path)
            .canonicalize()
            .with_context(|| format!("Failed to canonicalize path: {:?}", args.cosi_path))?;

        Url::from_file_path(&cosi_path)
            .map_err(|()| anyhow::anyhow!("Failed to convert path to URL: {:?}", cosi_path))?
    };

    info!("Loading COSI file from: {}", cosi_url);

    let mut image_source = ConfigOsImage {
        url: cosi_url,
        sha384: ImageSha384::Ignored,
    };

    let timeout = Duration::from_secs(args.timeout);

    let mut os_image =
        OsImage::load(&mut image_source, timeout).unstructured("Failed to load COSI file")?;

    if let Some(_gpt) = os_image.gpt().context("Failed to obtain GPT information")? {
        info!("GPT data found in COSI file");
    } else {
        info!("No GPT data found in COSI file.");
    }

    info!("COSI file read successfully!");
    println!("{}", os_image.metadata_sha384());

    Ok(())
}
