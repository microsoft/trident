use std::process::ExitCode;

use anyhow::{bail, Context, Error};
use log::error;
use tokio::{fs, runtime::Builder};

use crate::{
    cli::{self, ClientArgs, ClientCommands, TridentExitCodes},
    ExitKind, TRIDENT_VERSION,
};

mod error;
mod tridentclient;

use tridentclient::{RebootHandling, TridentClient};

pub fn client_main(args: &ClientArgs) -> ExitCode {
    // Start the Tokio runtime
    let Ok(runtime) = Builder::new_multi_thread().enable_all().build() else {
        error!("Failed to create Tokio runtime");
        return TridentExitCodes::SetupFailed.into();
    };

    match runtime.block_on(run_client(args)) {
        Err(e) => {
            error!("Client failed: {:?}", e);
            return TridentExitCodes::Failed.into();
        }
        Ok(ExitKind::Done) => {}
        Ok(ExitKind::NeedsReboot) => {
            if let Err(e) = crate::request_reboot_with_wait() {
                error!("Failed to reboot: {e:?}");
                return TridentExitCodes::RebootUnsuccessful.into();
            }
        }
    }

    TridentExitCodes::Success.into()
}

async fn run_client(args: &ClientArgs) -> Result<ExitKind, Error> {
    let mut client = TridentClient::connect(&args.server)
        .await
        .context("Failed to connect to Trident server")?;

    match &args.command {
        ClientCommands::Version => {
            println!("client: {TRIDENT_VERSION}");
            let version = client
                .version()
                .await
                .context("Failed to get Trident daemon version")?;
            println!("daemon: {}", version);
        }

        ClientCommands::Install {
            config,
            allowed_operations,
            multiboot,
        } => {
            let config_yaml = fs::read_to_string(config).await.with_context(|| {
                format!("Failed to read configuration file: {}", config.display())
            })?;

            if *multiboot {
                bail!("Multiboot installs are not implemented via gRPC client yet");
            }

            let operations = cli::to_operations(allowed_operations);

            if operations.has_finalize() && operations.has_stage() {
                return client
                    .install(config_yaml, RebootHandling::Trident)
                    .await
                    .context("Trident failed to perform install");
            } else if operations.has_stage() {
                bail!("Staging-only installs are not implemented via gRPC client yet");
            } else if operations.has_finalize() {
                bail!("Finalizing-only installs are not implemented via gRPC client yet");
            } else {
                bail!("At least one allowed operation must be specified");
            }
        }

        ClientCommands::StreamImage { image, hash } => {
            return client
                .stream_image(image, hash, RebootHandling::Trident)
                .await
                .context("Trident failed to stream image");
        }

        ClientCommands::Commit => {
            return client
                .commit()
                .await
                .context("Trident failed to perform commit");
        }

        cmd => {
            bail!("Unimplemented command: '{}'", cmd.name());
        }
    }

    Ok(ExitKind::Done)
}
