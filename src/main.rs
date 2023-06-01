use std::net::SocketAddr;

use clap::{Parser, Subcommand};
use tonic::transport::Server;
use trident::{config::ConfigFile, GreeterImpl, GreeterServer};

#[derive(Parser, Debug)]
#[command(version)]
struct Args {
    #[clap(short, long)]
    config: Option<String>,
    #[clap(short, long)]
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
    let config: ConfigFile = toml::from_str(&config).expect("Failed to parse config file");

    match args.subcmd {
        SubCommand::Validate => {}
        SubCommand::Run => {
            println!("Running");

            Server::builder()
                .add_service(GreeterServer::new(GreeterImpl::default()))
                .serve(SocketAddr::new(
                    "::1".parse().unwrap(),
                    config.listen_port.unwrap_or(50051),
                ))
                .await?;
        }
    }

    Ok(())
}
