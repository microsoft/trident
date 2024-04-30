use std::{io::Write, path::Path};

use anyhow::{Context, Error};
use trident_api::config::HostConfiguration;

pub(crate) fn write(dest: Option<impl AsRef<Path>>) -> Result<(), Error> {
    let schema = HostConfiguration::generate_schema();
    let schema = serde_json::to_string_pretty(&schema)?;

    if let Some(dest) = dest {
        let mut file = osutils::files::create_file(dest.as_ref())
            .context(format!("Failed to create file {}", dest.as_ref().display()))?;
        file.write_all(schema.as_bytes()).context(format!(
            "Failed to write to file {}",
            dest.as_ref().display()
        ))?;
    } else {
        println!("{}", schema);
    }

    Ok(())
}
