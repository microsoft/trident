use std::process::ExitCode;

use anyhow::{bail, Context, Error};
use log::error;
use tokio::runtime::Builder;

use crate::{
    cli::{ClientArgs, ClientCommands},
    TRIDENT_VERSION,
};

mod client;
mod error;

use client::TridentClient;

pub fn client_main(args: &ClientArgs) -> ExitCode {
    // Start the Tokio runtime
    let Ok(runtime) = Builder::new_multi_thread().enable_all().build() else {
        error!("Failed to create Tokio runtime");
        return ExitCode::from(1);
    };

    if let Err(e) = runtime.block_on(run_client(args)) {
        error!("Client failed: {:?}", e);
        return ExitCode::from(1);
    }

    ExitCode::from(0)
}

async fn run_client(args: &ClientArgs) -> Result<(), Error> {
    let mut client = TridentClient::connect(&args.server)
        .await
        .context("Failed to connect to Trident server")?;

    match &args.command {
        ClientCommands::Version => {
            println!("client: {TRIDENT_VERSION}");
            let version = client
                .version()
                .await
                .context("Failed to get Trident server version")?;
            println!("server: {}", version);
        }
        _ => {
            bail!("Unimplemented client command");
        }
    }

    Ok(())
}
