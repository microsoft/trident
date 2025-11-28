use std::path::PathBuf;

use anyhow::{Context, Error};
use clap::CommandFactory;
use tera::{Context as TeraCtx, Tera};
use trident::cli::Cli;

use crate::clap_model::command::CommandModel;

pub(crate) fn build_docs() -> Result<String, Error> {
    let cli_root = CommandModel::from(&Cli::command());

    let tera = Tera::new(
        PathBuf::from(file!())
            .parent()
            .unwrap()
            .join("templates/*")
            .to_str()
            .expect("Failed to get template path"),
    )
    .context("Failed to create Tera instance")?;

    let rendered = tera
        .render(
            "doc.md.jinja2",
            &TeraCtx::from_serialize(cli_root).context("Failed to serialize data model")?,
        )
        .context("Failed to render template")?;

    Ok(rendered)
}
