use std::path::Path;

use anyhow::{Context, Error};
use trident_api::config::HostConfiguration;

use crate::schema_renderer::{SchemaDocBuilder, SchemaDocSettings};

pub(crate) fn build(dest: impl AsRef<Path>, settings: SchemaDocSettings) -> Result<(), Error> {
    let builder = SchemaDocBuilder::new(HostConfiguration::generate_schema(), settings)
        .context("Failed to create schema doc builder")?;
    let pages = builder.build_pages().context("Failed to build pages")?;

    for page in pages {
        let path = dest.as_ref().join(&page.relative_path);
        std::fs::write(path, &page.content)?;
    }

    Ok(())
}
