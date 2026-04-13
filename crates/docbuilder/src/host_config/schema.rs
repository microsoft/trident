use std::{io::Write, path::Path};

use anyhow::{Context, Error};
use log::debug;
use serde_json::Value;
use tera::{Context as TeraCtx, Tera};

use trident_api::{config::HostConfiguration, schema::CONSTANTS_MAP, schemars::schema::RootSchema};

/// Tag used to indicate that anything after it in a description is intended for
/// internal use and should not be rendered in the documentation.
const INTERNAL_DESCRIPTION_TAG: &str = "# INTERNAL";

pub(crate) fn write(dest: Option<impl AsRef<Path>>) -> Result<(), Error> {
    let schema = serde_json::to_string_pretty(&host_config_schema()?)?;

    if let Some(dest) = dest {
        let mut file = osutils::files::create_file(dest.as_ref())
            .context(format!("Failed to create file {}", dest.as_ref().display()))?;
        file.write_all(schema.as_bytes()).context(format!(
            "Failed to write to file {}",
            dest.as_ref().display()
        ))?;
    } else {
        println!("{schema}");
    }

    Ok(())
}

/// Returns the schema with all description fields rendered through Tera.
pub(super) fn host_config_schema() -> Result<RootSchema, Error> {
    render_descriptions(HostConfiguration::generate_schema())
        .context("Failed to render descriptions in Host Configuration schema")
}

/// Recursively renders all description fields in the schema, replacing variable
/// placeholders with their actual values.
fn render_descriptions(root: RootSchema) -> Result<RootSchema, Error> {
    let mut tera_ctx = TeraCtx::new();
    for (key, value) in CONSTANTS_MAP {
        tera_ctx.insert(*key, *value);
    }

    let mut json = serde_json::to_value(&root).context("Failed to serialize schema to JSON")?;
    render_descriptions_recursive(&mut json, &tera_ctx)?;
    serde_json::from_value(json).context("Failed to deserialize schema from JSON")
}

/// Walks a JSON value tree and renders any string value under a `"description"`
/// key through Tera.
fn render_descriptions_recursive(value: &mut Value, ctx: &TeraCtx) -> Result<(), Error> {
    match value {
        Value::Object(map) => {
            for (key, child) in map.iter_mut() {
                if key == "description" {
                    if let Value::String(desc) = child {
                        if let Some(idx) = desc.find(INTERNAL_DESCRIPTION_TAG) {
                            desc.truncate(idx);
                            *desc = desc.trim_end().to_string();
                        }

                        debug!("Rendering description: {desc}");
                        *desc = Tera::one_off(desc, ctx, false)
                            .with_context(|| format!("Failed to render description: {desc}"))?;
                    }
                } else {
                    render_descriptions_recursive(child, ctx)?;
                }
            }
        }
        Value::Array(arr) => {
            for item in arr {
                render_descriptions_recursive(item, ctx)?;
            }
        }
        _ => {}
    }
    Ok(())
}
