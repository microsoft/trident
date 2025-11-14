use std::path::Path;

use anyhow::{Context, Error};
use serde_json::json;
use trident_api::config::HostConfiguration;

use crate::schema_renderer::{renderer::Page, SchemaDocBuilder, SchemaDocSettings};

pub(crate) fn build(dest: impl AsRef<Path>, settings: SchemaDocSettings) -> Result<(), Error> {
    let builder = SchemaDocBuilder::new(HostConfiguration::generate_schema(), settings.clone())
        .context("Failed to create schema doc builder")?;
    let pages = builder.build_pages().context("Failed to build pages")?;

    // If docusaurus is enabled, make the first page be the category index.
    if let Some(docusaurus_root) = &settings.docusaurus {
        let first_page = pages
            .first()
            .context("No pages generated for schema documentation")?;
        make_docusaurus_category_file(docusaurus_root, dest.as_ref(), first_page)
            .context("Failed to create docusaurus category file")?;
    }

    for page in pages {
        let path = dest.as_ref().join(&page.relative_path);
        std::fs::write(path, &page.content)?;
    }

    Ok(())
}

fn make_docusaurus_category_file(
    docusaurus_root: &Path,
    dest: &Path,
    first_page: &Page,
) -> Result<(), Error> {
    // Create the _category_.json file in the dest directory
    let category_index_path = dest.join("_category_.json");

    // Set the name of the category to the last component of the dest path
    let category_name = dest
        .components()
        .last()
        .context("Failed to get category name")?
        .as_os_str()
        .to_string_lossy();

    // Get the output path relative to docusaurus root
    let output_relative_path = dest.strip_prefix(docusaurus_root).with_context(|| {
        format!(
            "Failed to get relative path of output '{}' to docusaurus root '{}'",
            dest.display(),
            docusaurus_root.display(),
        )
    })?;

    // Remove leading ./ from first page relative path if present
    let first_page_relative_path = first_page
        .relative_path
        .strip_prefix("./")
        .unwrap_or(&first_page.relative_path);

    // Get the first page relative path to docusaurus root without extension
    let first_page_relative_path_to_docusaurus = output_relative_path
        .join(&first_page_relative_path)
        .with_extension("")
        .to_string_lossy()
        .to_string();

    // Create the category object
    let category_obj = json!({
        "label": category_name,
        "link": {
            "type": "doc",
            "id": first_page_relative_path_to_docusaurus
        }
    });

    let category_index_content = serde_json::to_string_pretty(&category_obj)
        .context("Failed to serialize docusaurus category index")?;

    std::fs::write(category_index_path, category_index_content)
        .context("Failed to write docusaurus category index file")?;

    Ok(())
}
