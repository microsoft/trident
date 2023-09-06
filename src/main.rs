use std::{fs, mem, path::PathBuf, time::Duration};

use anyhow::Context;
use clap::{Parser, Subcommand};
use log::{debug, error, info, warn};

use trident_api::config::{HostConfigSource, LocalConfigFile};

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
    let mut config: LocalConfigFile = serde_yaml::from_str(&config_contents)
        .map_err(|e| warn!("Failed to parse config file: {e}"))
        .unwrap_or_default();
    debug!("Config: {:#?}", config);

    let host_config = match &mut config.host_config_source {
        HostConfigSource::File(path) => {
            info!("Loading host config from '{}'", path.display());
            fs::read_to_string(path)
                .map_err(|e| warn!("Failed to read host config file: {e}"))
                .ok()
                .and_then(|contents| {
                    serde_yaml::from_str(&contents)
                        .map_err(|e| warn!("Failed to parse host config file: {e}"))
                        .ok()
                })
        }
        HostConfigSource::Embedded(contents) => Some(mem::take(contents)),
        HostConfigSource::GrpcCommand { .. } => None,
    };
    // let host_config = Some(&config.host_config);
    debug!("Host config: {:#?}", host_config);

    info!("Starting network");
    trident::start_provisioning_network(
        config.network_override,
        host_config
            .as_ref()
            .and_then(|c| c.network_provision.clone()),
        host_config.as_ref().and_then(|c| c.network.clone()),
    )
    .context("Failed to start provisioning network")?;

    if let Some(phonehome) = config.phonehome {
        info!("Phonehome to {}", phonehome);
        for _ in 0..5 {
            if reqwest::Client::new()
                .post(&phonehome)
                .body("hello-from-trident")
                .send()
                .await
                .map_err(|e| error!("Failed to phonehome: {}", e))
                .is_ok()
            {
                break;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    }

    match args.subcmd {
        SubCommand::Run => match config.host_config_source {
            HostConfigSource::File(_) | HostConfigSource::Embedded(_) => {
                info!("Running");
                if let Err(e) = trident::run(host_config.as_ref().unwrap(), config.datastore) {
                    error!("{e:?}");
                }
            }
            HostConfigSource::GrpcCommand { listen_port } => {
                info!("Listening");
                trident::serve("0.0.0.0".parse().unwrap(), listen_port.unwrap_or(50051)).await?;
            }
        },
    }

    Ok(())
}
