use std::path::PathBuf;

use anyhow::{Context, Error};
use clap::{Args, Parser, Subcommand};
use log::info;

use crate::schema_renderer::SchemaDocSettings;

mod host_config;
mod schema_renderer;
mod setsail;

#[derive(Parser, Debug)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Build markdown docs for setsail
    Setsail(SetsailOpts),

    /// Build markdown docs for Host Configuration
    HostConfig(HostConfigCli),
}

#[derive(Args, Debug)]
struct SetsailOpts {
    /// Optional output file
    ///
    /// If not specified, will print to stdout.
    #[clap(short, long)]
    output: Option<PathBuf>,
}

#[derive(Parser, Debug)]
struct HostConfigCli {
    #[clap(subcommand)]
    command: HostConfigCommands,
}

#[derive(Subcommand, Debug)]
enum HostConfigCommands {
    /// Build markdown docs for Host Configuration
    Markdown(HostConfigMarkdownOpts),

    /// Print the Host Configuration JSON Schema
    ///
    /// If no output file is specified, will print to stdout.
    Schema {
        /// Output file
        #[clap(short, long)]
        output: Option<PathBuf>,
    },

    /// Print the Host Configuration Sample
    ///
    /// If no output file is specified, will print to stdout.
    Sample {
        /// Whether to output raw or as markdown
        #[clap(short, long)]
        markdown: bool,

        /// Output file
        #[clap(short, long)]
        output: Option<PathBuf>,

        /// Name
        #[clap(short, long)]
        name: String,
    },
}

#[derive(Debug, Args)]
struct HostConfigMarkdownOpts {
    /// Output folder
    ///
    /// Will delete existing contents of this folder and replace with new docs.
    #[clap(required = true)]
    output: PathBuf,

    /// Whether to create DevOps Wiki ordering file
    #[clap(long)]
    devops_wiki: bool,

    /// Whether to use docfx-only features
    ///
    /// This enables features such as tabs.
    #[clap(long)]
    docfx: bool,
}

fn main() -> Result<(), Error> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "docbuilder=info");
    }

    pretty_env_logger::init();

    let opts = Cli::parse();

    match opts.command {
        Commands::Setsail(opts) => build_setsail_docs(opts).context("Failed to build setsail docs"),
        Commands::HostConfig(opts) => match opts.command {
            HostConfigCommands::Markdown(opts) => {
                build_host_config_docs(opts).context("Failed to build host config docs")
            }
            HostConfigCommands::Schema { output } => {
                host_config::print_schema(output).context("Failed to print schema")
            }
            HostConfigCommands::Sample {
                markdown,
                output,
                name,
            } => {
                host_config::print_sample(name, output, markdown).context("Failed to print sample")
            }
        },
    }
}

fn build_setsail_docs(opts: SetsailOpts) -> Result<(), Error> {
    info!("Building setsail docs");
    let doc = setsail::build_docs();

    if let Some(parent) = opts.output.as_ref().and_then(|p| p.parent()) {
        std::fs::create_dir_all(parent).context(format!(
            "Failed to create parent directory {}",
            parent.display()
        ))?;
    } else {
        println!("{}", doc);
    }

    Ok(())
}

fn build_host_config_docs(opts: HostConfigMarkdownOpts) -> Result<(), Error> {
    info!("Building host config docs");
    // Create output directory if it doesn't exist.
    osutils::files::create_dirs(&opts.output).context(format!(
        "Failed to create directory {}",
        opts.output.display()
    ))?;

    // Ensure output directory is empty.
    osutils::files::clean_directory(&opts.output).context(format!(
        "Failed to clean directory {}",
        opts.output.display()
    ))?;

    host_config::build(
        opts.output,
        SchemaDocSettings {
            devops_wiki: opts.devops_wiki,
            docfx: opts.docfx,
        },
    )
    .context("Failed to build host config docs")
}
