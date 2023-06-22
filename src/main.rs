use std::{fs, path::PathBuf};

use clap::{Parser, Subcommand};
use log::{info, warn};
use trident::config::ConfigFile;

#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[clap(global = true, short, long, default_value = "/etc/trident/config.yaml")]
    config: PathBuf,
    #[clap(global = true, short, long)]
    verbose: bool,
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Subcommand, Debug)]
enum SubCommand {
    Validate,
    Run,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder().format_timestamp(None).init();

    let args = Args::parse();

    let config_contents = fs::read_to_string(&args.config)
        .map_err(|e| warn!("Failed to read config file: {e}"))
        .unwrap_or_default();
    let config: ConfigFile = serde_yaml::from_str(&config_contents)
        .map_err(|e| warn!("Failed to parse config file: {e}"))
        .unwrap_or_default();

    info!("Starting network!");
    trident::start_provisioning_network(config.network, config.network_provision);

    if let Some(phonehome) = config.core.phonehome {
        reqwest::Client::new()
            .post(&phonehome)
            .body("hello-from-trident")
            .send()
            .await?;
    }

    match args.subcmd {
        SubCommand::Validate => {}
        SubCommand::Run => {
            info!("Running");
            trident::serve(
                "0.0.0.0".parse().unwrap(),
                config.core.listen_port.unwrap_or(50051),
            )
            .await?;
        }
    }

    Ok(())
}
