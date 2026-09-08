use std::process::ExitCode;

use anyhow::{bail, Context, Error};
use log::error;
use tokio::fs;
use tokio::runtime::Builder;

use trident_api::error::{InternalError, TridentError};

use crate::{
    cli::{ClientArgs, ClientCommands, TridentExitCodes},
    run_command_if, ExitKind, TRIDENT_VERSION,
};

use crate::cli;

mod error;
mod tridentclient;

use error::TridentClientError;
use tridentclient::{RebootHandling, TridentClient};

pub fn client_main(args: &ClientArgs) -> ExitCode {
    // Start the Tokio runtime
    let Ok(runtime) = Builder::new_multi_thread().enable_all().build() else {
        error!("Failed to create Tokio runtime");
        return TridentExitCodes::SetupFailed.into();
    };

    // `setup_tracing()` (see `main.rs`) treats grpc-client the same as any
    // other command -- it's a first-class telemetry participant, not just
    // a transport-only escape hatch, so it fires `command_start` like
    // every other command. `command_error` is more selective (see
    // `is_transport_failure` below). `run_client`'s errors are plain
    // `anyhow::Error` (not `TridentError`), so they're wrapped in a
    // generic `InternalError::Internal` here purely to get them into
    // `run_command_if`'s `Result<_, TridentError>` shape -- the original
    // anyhow context chain is preserved as the error's source and still
    // printed in full below.
    let command = args.command.name().replace('-', "_");
    let client_result = runtime.block_on(run_client(args));

    // The daemon fires its own, correctly-classified `command_error` for
    // any request it actually received and acted on -- including one it
    // rejected outright (see e.g. `services::reject_invalid_argument`).
    // Only a genuine transport-level failure (the daemon never received
    // or answered this request at all -- socket not found, connection
    // refused, connection dropped mid-call) has no other reporter, so
    // that's the only case where firing a client-side `command_error`
    // adds signal instead of just duplicating the daemon's own event
    // under a generic, less-informative classification.
    let is_transport_failure = client_result.as_ref().err().is_some_and(|e| {
        e.chain().any(|cause| {
            matches!(
                cause.downcast_ref::<TridentClientError>(),
                Some(TridentClientError::ConnectionError(..))
            )
        })
    });

    let result = run_command_if(
        &command,
        || {
            client_result.map_err(|e| {
                TridentError::with_source(InternalError::Internal("grpc-client command failed"), e)
            })
        },
        |_error| is_transport_failure,
    );

    match result {
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

        ClientCommands::StreamDisk { image, hash } => {
            return client
                .stream_disk(image, hash.as_ref(), RebootHandling::Trident)
                .await
                .context("Trident failed to stream image");
        }

        ClientCommands::Update {
            config,
            allowed_operations,
            ..
        } => {
            let config_yaml = fs::read_to_string(config).await.with_context(|| {
                format!("Failed to read configuration file: {}", config.display())
            })?;

            let operations = cli::to_operations(allowed_operations);

            if operations.has_finalize() && operations.has_stage() {
                return client
                    .update(config_yaml, RebootHandling::Trident)
                    .await
                    .context("Trident failed to perform update");
            } else if operations.has_stage() {
                return client
                    .update_stage(config_yaml)
                    .await
                    .context("Trident failed to perform update_stage");
            } else if operations.has_finalize() {
                return client
                    .update_finalize(RebootHandling::Trident)
                    .await
                    .context("Trident failed to perform update_finalize");
            } else {
                bail!("At least one allowed operation must be specified");
            }
        }

        #[cfg(feature = "grpc-preview")]
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
