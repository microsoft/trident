use std::path::{self, PathBuf};

use anyhow::{bail, Context, Error};
use clap::{Args, Parser, Subcommand, ValueEnum};
use log::info;

use crate::schema_renderer::SchemaDocSettings;

mod clap_model;
mod host_config;
mod markdown;
mod schema_renderer;
mod trident_arch;
mod trident_cli;

#[derive(Parser, Debug)]
struct Cli {
    #[clap(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Build markdown docs for Host Configuration
    HostConfig(HostConfigCli),

    /// Output documentation for Trident's CLI
    TridentCli(TridentCliOpts),

    /// Output a Trident arch diagram
    TridentArch(TridentArchOpts),
}

#[derive(Args, Debug)]
struct SetsailOpts {
    /// Optional output file
    ///
    /// If not specified, will print to stdout.
    #[clap(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct TridentCliOpts {
    /// Optional output file
    ///
    /// If not specified, will print to stdout.
    #[clap(short, long)]
    output: Option<PathBuf>,
}

#[derive(Args, Debug)]
struct HostConfigCli {
    #[clap(subcommand)]
    command: HostConfigCommands,
}

#[derive(Args, Debug)]
struct TridentArchOpts {
    /// Optional output file
    ///
    /// If not specified, will print to stdout.
    #[clap(short, long)]
    output: Option<PathBuf>,

    /// Arch diagram to output
    selected: TridentArchSelection,
}

#[derive(Debug, ValueEnum, Clone, Copy)]
#[clap(rename_all = "kebab-case")]
enum TridentArchSelection {
    Install,
    Update,
}

#[derive(Subcommand, Debug)]
enum HostConfigCommands {
    /// Build markdown docs for Host Configuration
    Markdown(HostConfigMarkdownOpts),

    /// Output the Host Configuration JSON Schema
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

    /// Output the Storage Rules
    ///
    /// If no output file is specified, will print to stdout.
    StorageRules {
        /// Output file
        #[clap(short, long)]
        output: Option<PathBuf>,
    },
}

#[derive(Debug, Args)]
struct HostConfigMarkdownOpts {
    /// Output folder.
    ///
    /// Will delete existing contents of this folder and replace with new docs.
    #[clap(required = true)]
    output: PathBuf,

    /// Whether to create DevOps Wiki ordering file.
    #[clap(long, group = "flavor")]
    devops_wiki: bool,

    /// Whether to use docfx-only features.
    ///
    /// This enables features such as tabs.
    #[clap(long, group = "flavor")]
    docfx: bool,

    /// Enable docusaurus-specific features. Expects the path to the root of the
    /// docusaurus site.
    #[clap(long, group = "flavor")]
    docusaurus_root: Option<PathBuf>,
}

fn main() -> Result<(), Error> {
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "docbuilder=info");
    }

    pretty_env_logger::init();

    let opts = Cli::parse();

    match opts.command {
        Commands::HostConfig(opts) => match opts.command {
            HostConfigCommands::Markdown(opts) => {
                build_host_config_docs(opts).context("Failed to build host config docs")
            }
            HostConfigCommands::Schema { output } => {
                host_config::schema::write(output).context("Failed to print schema")
            }
            HostConfigCommands::Sample {
                markdown,
                output,
                name,
            } => host_config::samples::print(name, output, markdown)
                .context("Failed to print sample"),
            HostConfigCommands::StorageRules { output } => {
                host_config::storage_rules::write(output)
            }
        },
        Commands::TridentCli(opts) => {
            build_tricent_cli_docs(opts).context("Failed to build CLI docs")
        }
        Commands::TridentArch(opts) => {
            build_trident_arch_diagram(opts).context("Failed to build arch diagram")
        }
    }
}

fn build_host_config_docs(mut opts: HostConfigMarkdownOpts) -> Result<(), Error> {
    info!("Building host config docs");

    opts.output = path::absolute(&opts.output).context(format!(
        "Failed to get absolute path for output: {}",
        opts.output.display()
    ))?;

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

    if let Some(docusaurus_root) = &mut opts.docusaurus_root {
        // Canonicalize docusaurus root path.
        *docusaurus_root = docusaurus_root.canonicalize().context(format!(
            "Failed to canonicalize docusaurus root path {}",
            docusaurus_root.display()
        ))?;

        // Ensure docusaurus root exists.
        if !docusaurus_root.is_dir() {
            bail!(format!(
                "Docusaurus root path '{}' is not an existing directory",
                docusaurus_root.display()
            ));
        }

        // Ensure the output path is inside the docusaurus root.
        if !opts.output.starts_with(&docusaurus_root) {
            bail!(
                "Output path '{}' is not inside the docusaurus root '{}'",
                opts.output.display(),
                docusaurus_root.display()
            );
        }
    }

    host_config::docs::build(
        opts.output,
        SchemaDocSettings {
            devops_wiki: opts.devops_wiki,
            docfx: opts.docfx,
            docusaurus: opts.docusaurus_root,
        },
    )
    .context("Failed to build host config docs")
}

fn build_tricent_cli_docs(opts: TridentCliOpts) -> Result<(), Error> {
    info!("Building trident cli docs");

    let docs = trident_cli::build_docs().context("Failed to build trident cli docs")?;

    if let Some(output) = opts.output {
        let parent = output.parent().context("Failed to get parent directory")?;
        std::fs::create_dir_all(parent).context(format!(
            "Failed to create parent directory {}",
            parent.display()
        ))?;

        std::fs::write(&output, docs)
            .context(format!("Failed to write to file {}", output.display()))?;
    } else {
        println!("{docs}");
    }

    Ok(())
}

fn build_trident_arch_diagram(opts: TridentArchOpts) -> Result<(), Error> {
    info!("Building trident arch diagram");

    let diagram = trident_arch::build_arch_diagram(opts.selected)
        .context("Failed to build trident arch diagram")?;

    if let Some(output) = opts.output {
        let parent = output.parent().context("Failed to get parent directory")?;
        std::fs::create_dir_all(parent).context(format!(
            "Failed to create parent directory {}",
            parent.display()
        ))?;

        std::fs::write(&output, diagram)
            .context(format!("Failed to write to file {}", output.display()))?;
    } else {
        println!("{diagram}");
    }

    Ok(())
}
