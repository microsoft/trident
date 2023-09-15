use std::{fs, mem, path::PathBuf, time::Duration};

use anyhow::Context;
use clap::{Parser, Subcommand};
use log::{debug, error, info, warn};

use trident_api::config::{HostConfigSource, LocalConfigFile};

use setsail::{load_kickstart_file, load_kickstart_string, KsTranslator};

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
    #[clap(name = "start-network")]
    StartNetwork,
    ParseKickstart {
        file: String,
    },
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
        HostConfigSource::KickstartEmbedded(contents) => {
            match KsTranslator::new().translate(load_kickstart_string(contents)) {
                Ok(hc) => Some(Box::new(hc)),
                Err(e) => {
                    // TODO: handle & report kickstart errors
                    error!(
                        "Failed to translate kickstart:\n{}",
                        serde_json::to_string_pretty(&e)?
                    );
                    None
                }
            }
        }
        HostConfigSource::Kickstart(file) => {
            match KsTranslator::new().translate(load_kickstart_file(
                file.to_str()
                    .context(format!("Failed to resolve path {}", file.display()))?,
            )?) {
                Ok(hc) => Some(Box::new(hc)),
                Err(e) => {
                    error!(
                        // TODO: handle & report kickstart errors
                        "Failed to translate kickstart:\n{}",
                        serde_json::to_string_pretty(&e)?
                    );
                    None
                }
            }
        }
    };
    // let host_config = Some(&config.host_config);
    debug!("Host config: {:#?}", host_config);

    match args.subcmd {
        SubCommand::Run => {
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

            match config.host_config_source {
                HostConfigSource::File(_)
                | HostConfigSource::Embedded(_)
                | HostConfigSource::Kickstart(_)
                | HostConfigSource::KickstartEmbedded(_) => {
                    info!("Running");
                    if let Err(e) = trident::run(
                        host_config.as_ref().unwrap(),
                        config.allowed_operations,
                        config.datastore,
                    ) {
                        error!("{e:?}");
                    }
                }
                HostConfigSource::GrpcCommand { listen_port } => {
                    info!("Listening");
                    trident::serve("0.0.0.0".parse().unwrap(), listen_port.unwrap_or(50051))
                        .await?;
                }
            }
        }

        SubCommand::StartNetwork => {
            info!("Starting network");
            trident::start_provisioning_network(config.network_override, host_config.as_deref())
                .context("Failed to start provisioning network")?;
        }

        // TODO: Remove this in the future
        // It's very useful for testing
        SubCommand::ParseKickstart { file } => {
            match KsTranslator::new()
                .translate(load_kickstart_file(&file).context(format!("Failed to read {}", &file))?)
            {
                Ok(hc) => {
                    println!("{}", serde_yaml::to_string(&hc)?);
                }
                Err(e) => {
                    error!(
                        "Failed to translate kickstart:\n{}",
                        serde_json::to_string_pretty(&e)?
                    );
                }
            }
        }
    }

    Ok(())
}
