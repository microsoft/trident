use std::{fs, mem, path::PathBuf};

use anyhow::{bail, Context, Error};
use clap::{Parser, Subcommand};
use log::{debug, error, info, warn};

use trident::{OrchestratorConnection, TRIDENT_LOCAL_CONFIG_PATH};
use trident_api::config::{HostConfigurationSource, LocalConfigFile};

use setsail::KsTranslator;

#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[clap(global = true, short, long, default_value = TRIDENT_LOCAL_CONFIG_PATH)]
    config: PathBuf,
    #[clap(subcommand)]
    subcmd: SubCommand,
}

#[derive(Subcommand, Debug)]
enum SubCommand {
    Run,
    #[clap(name = "start-network")]
    StartNetwork,
    // TODO(5910): Remove this in the future
    ParseKickstart {
        file: String,
    },
}

fn main() -> Result<(), Error> {
    // Initialize the loggers

    // Create the logstream
    let logstream = trident::Logstream::create();

    // Set up the multilogger
    trident::MultiLogger::new()
        .with_logger(Box::new(
            env_logger::builder().format_timestamp(None).build(),
        ))
        .with_logger(logstream.make_logger())
        .init()
        .expect("Logger already registered");

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

    // Load the config file
    info!("Loading config from '{}'", args.config.display());
    let config_contents = fs::read_to_string(&args.config)
        .map_err(|e| warn!("Failed to read config file: {e}"))
        .unwrap_or_default();

    // Parse the config file
    let mut config: LocalConfigFile = match serde_yaml::from_str(&config_contents)
        .context("Failed to parse Trident configuration")
    {
        Ok(config) => config,
        Err(e) => {
            warn!("{e:?}");

            // If parsing the config file failed, maybe we can still understand enough of it to
            // extract the phonehome URL.
            if let Some(url) = config_contents
                .lines()
                .find(|l| l.starts_with("phonehome:"))
                .map(|l| l[10..].trim())
                .filter(|l| reqwest::Url::parse(l).is_ok())
            {
                if let Some(o) = OrchestratorConnection::new(url.to_string()) {
                    o.report_error(format!("{e:?}"))
                }
            }
            return Err(e);
        }
    };

    // Set up logstream if configured
    if let Some(url) = config.trident_config.logstream.as_ref() {
        logstream
            .set_server(url.to_string())
            .context("Failed to set logstream URL")?;
    }

    debug!(
        "Trident config:\n{}",
        serde_yaml::to_string(&config).unwrap_or("Failed to serialize host config".into())
    );

    // If we have kickstart it means we don't have networking config readily available.
    // We _could_ try parsing now, but we are in an early stage of boot and we want to parse on
    // a later stage so %pre scripts can run and do their thing.
    // It would also mean parsing twice, unless we updated the config file in place.
    // That sounds like a can of worms and we still have the issue about being too early.
    if matches!(args.subcmd, SubCommand::StartNetwork)
        && matches!(
            config.host_config_source,
            HostConfigurationSource::Kickstart(_) | HostConfigurationSource::KickstartEmbedded(_)
        )
    {
        warn!("Cannot set up network early when using kickstart");
        return Ok(());
    }

    let host_config = match &mut config.host_config_source {
        HostConfigurationSource::File(path) => {
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
        HostConfigurationSource::Embedded(contents) => Some(mem::take(contents)),
        HostConfigurationSource::GrpcCommand { .. } => None,
        HostConfigurationSource::KickstartEmbedded(contents) => {
            match KsTranslator::new()
                .run_pre_scripts(true)
                .translate(setsail::load_kickstart_string(contents))
            {
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
        HostConfigurationSource::Kickstart(file) => {
            match KsTranslator::new()
                .run_pre_scripts(true)
                .translate(setsail::load_kickstart_file(
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

    debug!(
        "Host config:\n{}",
        serde_yaml::to_string(&host_config).unwrap_or("Failed to serialize host config".into())
    );

    match args.subcmd {
        SubCommand::Run => {
            let orchestrator = config
                .trident_config
                .phonehome
                .as_ref()
                .and_then(|url| OrchestratorConnection::new(url.clone()));

            match config.host_config_source {
                HostConfigurationSource::File(_)
                | HostConfigurationSource::Embedded(_)
                | HostConfigurationSource::Kickstart(_)
                | HostConfigurationSource::KickstartEmbedded(_) => {
                    info!("Running");
                    match trident::run(*host_config.unwrap(), &config.trident_config) {
                        Ok(()) => {
                            if let Some(orchestrator) = orchestrator {
                                orchestrator.report_success()
                            }
                        }
                        Err(e) => {
                            error!("{e:?}");
                            if let Some(orchestrator) = orchestrator {
                                orchestrator.report_error(format!("{e:?}"));
                            }
                        }
                    }
                }
                HostConfigurationSource::GrpcCommand { listen_port } => {
                    info!("Listening");
                    if let Some(orchestrator) = orchestrator {
                        orchestrator.report_success()
                    }
                    trident::serve("0.0.0.0".parse().unwrap(), listen_port.unwrap_or(50051))?;
                }
            }
        }

        SubCommand::StartNetwork => {
            info!("Starting network");
            trident::start_provisioning_network(
                config.trident_config.network_override,
                host_config.as_deref(),
            )
            .context("Failed to start provisioning network")?;
        }

        // TODO(5910): Remove this in the future
        SubCommand::ParseKickstart { .. } => unreachable!(),
    }

    Ok(())
}
