use std::path::PathBuf;

use anyhow::{bail, Context, Error};
use clap::{Parser, Subcommand};
use log::error;

use trident::{Logstream, MultiLogger};

use setsail::KsTranslator;

#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[clap(global = true, short, long)]
    config: Option<PathBuf>,
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Subcommand, Debug)]
enum SubCommand {
    /// Apply the HostConfiguration
    Run,

    /// Configure OS networking based on Trident Configuration
    #[clap(name = "start-network")]
    StartNetwork,

    /// Get the HostStatus
    #[clap(name = "get-host-status")]
    GetHostStatus,

    /// Validates input KickStart file
    // TODO(5910): Remove this in the future
    ParseKickstart { file: String },
}

fn run_trident(logstream: Logstream) -> Result<(), Error> {
    let args = Args::parse();

    // TODO(5910): Remove this in the future
    if let SubCommand::ParseKickstart { ref file } = args.subcmd {
        let translator = KsTranslator::new().include_fail_is_error(false);
        match translator.translate(
            setsail::load_kickstart_file(file).context(format!("Failed to read {file}"))?,
        ) {
            Ok(hc) => {
                println!("{}", serde_yaml::to_string(&hc)?);
                return Ok(());
            }
            Err(e) => {
                error!(
                    "Failed to translate kickstart:\n{}",
                    serde_json::to_string_pretty(&e)?
                );
                bail!("Failed to translate kickstart");
            }
        };
    }

    let mut trident = trident::Trident::new(args.config, logstream)?;

    match args.subcmd {
        SubCommand::Run => trident
            .run()
            .context("Failed to execute Trident run command")?,
        SubCommand::StartNetwork => trident.start_network().context("Failed to start network")?,
        SubCommand::GetHostStatus => trident
            .print_host_status()
            .context("Failed to retrieve Host Status")?,

        // TODO(5910): Remove this in the future
        SubCommand::ParseKickstart { .. } => unreachable!(),
    }

    Ok(())
}

fn main() {
    // Initialize the loggers

    // Create the logstream
    let logstream = Logstream::create();

    // Set up the multilogger
    MultiLogger::new()
        .with_logger(Box::new(
            env_logger::builder().format_timestamp(None).build(),
        ))
        .with_logger(logstream.make_logger())
        .init()
        .expect("Logger already registered");

    if let Err(e) = run_trident(logstream) {
        error!("Trident failed: {e:?}");
        std::process::exit(1);
    }
}
