use std::{cell::Cell, process::ExitCode};

use anyhow::{bail, Context, Error};
use log::error;
use tokio::fs;
use tokio::runtime::Builder;
use tonic::Code;

use trident_api::error::{InternalError, TridentError};

use crate::{
    cli::{ClientArgs, ClientCommands, TridentExitCodes},
    run_command_if, ExitKind, OperationSource, TRIDENT_VERSION,
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

    // `run_client` (the actual RPC) runs *inside* this closure, not before
    // it, so `command_start` (fired by `run_command_if` the moment this
    // closure is entered) actually brackets the RPC instead of always
    // following it -- otherwise every client-side event the RPC itself
    // fires, and the timestamp of `command_start` itself, would be
    // reported after the call had already finished. `is_transport_failure`
    // is computed from the raw `anyhow::Error` chain here, inside the
    // closure, and stashed via `transport_failure` for `run_command_if`'s
    // `should_report` predicate below, which only ever sees the already-
    // wrapped `TridentError` and has no way to inspect that chain itself.
    let transport_failure = Cell::new(false);
    let result = run_command_if(
        &command,
        OperationSource::GrpcClient,
        || {
            let client_result = runtime.block_on(run_client(args));
            transport_failure.set(is_transport_failure(&client_result));
            client_result.map_err(|e| {
                TridentError::with_source(InternalError::Internal("grpc-client command failed"), e)
            })
        },
        |_error| transport_failure.get(),
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

/// The daemon fires its own, correctly-classified `command_error` for any
/// request it actually received and acted on -- including one it rejected
/// outright (see e.g. `services::reject_invalid_argument`), and every
/// `Status` the daemon itself ever deliberately constructs comes from
/// `trident_error_to_status`, which never produces `Code::Unavailable`.
/// So a genuine transport-level failure -- the daemon never received or
/// finished answering this request at all -- has no other reporter, and
/// is what this checks for:
/// - `ConnectionError`: the initial connection attempt itself failed
///   (socket not found, connection refused).
/// - `RequestError`/`ResponseError` whose wrapped `Status` is
///   `Code::Unavailable`: tonic's own code for a connection that broke
///   mid-call (e.g. the daemon process died or the socket was closed
///   while a request/response was in flight), as opposed to a `Status`
///   the daemon constructed and returned deliberately, which always
///   carries a different code and has already been reported server-side.
fn is_transport_failure(client_result: &Result<ExitKind, Error>) -> bool {
    client_result.as_ref().err().is_some_and(|e| {
        e.chain()
            .any(|cause| match cause.downcast_ref::<TridentClientError>() {
                Some(TridentClientError::ConnectionError(..)) => true,
                Some(TridentClientError::RequestError(_, status))
                | Some(TridentClientError::ResponseError(_, status)) => {
                    status.code() == Code::Unavailable
                }
                _ => false,
            })
    })
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
