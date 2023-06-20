use clap::{Parser, Subcommand};
use trident::config::ConfigFile;

#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[clap(global = true, short, long)]
    config: Option<String>,
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
    let args = Args::parse();

    let config = args
        .config
        .map(|filename| std::fs::read_to_string(filename).expect("Failed to read config file"))
        .unwrap_or_default();
    let config: ConfigFile = serde_yaml::from_str(&config).expect("Failed to parse config file");

    println!("Starting network!");
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
            println!("Running");
            trident::serve(
                "0.0.0.0".parse().unwrap(),
                config.core.listen_port.unwrap_or(50051),
            )
            .await?;
        }
    }

    Ok(())
}
