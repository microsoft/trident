use std::{fs, path::PathBuf};

use clap::{Parser, Subcommand};
use log::{debug, error, info, warn};
use trident::config::{ConfigFile, HostConfigSource};

#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[clap(global = true, short, long, default_value = "/etc/trident/config.yaml")]
    config: PathBuf,
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Subcommand, Debug)]
enum SubCommand {
    Run,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::builder().format_timestamp(None).init();

    let args = Args::parse();

    info!("Loading config from '{}'", args.config.display());
    let config_contents = fs::read_to_string(&args.config)
        .map_err(|e| warn!("Failed to read config file: {e}"))
        .unwrap_or_default();
    let config: ConfigFile = serde_yaml::from_str(&config_contents)
        .map_err(|e| warn!("Failed to parse config file: {e}"))
        .unwrap_or_default();
    debug!("Config: {:#?}", config);

    let host_config = config.host_config.and_then(|source| match source {
        HostConfigSource::File(path) => {
            info!("Loading host config from '{}'", path.display());
            let contents = fs::read_to_string(path)
                .map_err(|e| warn!("Failed to read host config file: {e}"))
                .ok()?;
            serde_yaml::from_str(&contents)
                .map_err(|e| warn!("Failed to parse host config file: {e}"))
                .ok()
        }
        HostConfigSource::Embedded(contents) => Some(contents),
    });
    debug!("Host config: {:#?}", host_config);

    info!("Starting network");
    trident::start_provisioning_network(
        config.network_override,
        host_config
            .as_ref()
            .and_then(|c| c.network_provision.clone()),
        host_config.as_ref().and_then(|c| c.network.clone()),
    );

    if let Some(phonehome) = config.phonehome {
        info!("Phonehome to {}", phonehome);
        let _ = reqwest::Client::new()
            .post(&phonehome)
            .body("hello-from-trident")
            .send()
            .await
            .map_err(|e| error!("Failed to phonehome: {}", e));
    }

    match args.subcmd {
        SubCommand::Run => match config.mode {
            trident::config::Mode::AutoProvision => match host_config {
                Some(config) => {
                    info!("Auto provisioning");
                    trident::auto_provision(&config).await.unwrap();
                }
                None => {
                    error!("No host config available, cannot auto provision");
                }
            },
            trident::config::Mode::Listen => {
                info!("Listening");
                trident::serve(
                    "0.0.0.0".parse().unwrap(),
                    config.listen_port.unwrap_or(50051),
                )
                .await?;
            }
        },
    }

    Ok(())
}
