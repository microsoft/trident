use std::process::ExitCode;

use anyhow::{anyhow, bail, Context, Error};
use log::{debug, error, info, log, Level};
use tokio::fs;
use tokio::runtime::Builder;

use trident_api::{config::ImageSha384, primitives::hash::Sha384Hash};
use trident_client::{
    Client as TridentClient, ClientError, Completion, LogLevel, RebootDisposition, RebootPolicy,
    ServicingEvent, ServicingOperation,
};

use crate::{
    cli::{ClientArgs, ClientCommands, TridentExitCodes},
    ExitKind, TRIDENT_VERSION,
};

use crate::cli;

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
    let client = TridentClient::connect(&args.server)
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
            let image_hash = parse_image_hash(hash.as_deref())?;
            return finish(
                client
                    .stream_disk(image, image_hash.as_ref(), RebootPolicy::Trident)
                    .await,
                "Trident failed to stream image",
            )
            .await;
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
                return finish(
                    client.update_yaml(config_yaml, RebootPolicy::Trident).await,
                    "Trident failed to perform update",
                )
                .await;
            } else if operations.has_stage() {
                return finish(
                    client.update_stage_yaml(config_yaml).await,
                    "Trident failed to perform update_stage",
                )
                .await;
            } else if operations.has_finalize() {
                return finish(
                    client.update_finalize(RebootPolicy::Trident).await,
                    "Trident failed to perform update_finalize",
                )
                .await;
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
                return finish(
                    client
                        .install_yaml(config_yaml, RebootPolicy::Trident)
                        .await,
                    "Trident failed to perform install",
                )
                .await;
            } else if operations.has_stage() {
                bail!("Staging-only installs are not implemented via gRPC client yet");
            } else if operations.has_finalize() {
                bail!("Finalizing-only installs are not implemented via gRPC client yet");
            } else {
                bail!("At least one allowed operation must be specified");
            }
        }

        ClientCommands::Commit => {
            return finish(
                client.commit(RebootPolicy::Trident).await,
                "Trident failed to perform commit",
            )
            .await;
        }

        cmd => {
            bail!("Unimplemented command: '{}'", cmd.name());
        }
    }

    Ok(ExitKind::Done)
}

async fn finish(
    operation: Result<ServicingOperation, ClientError>,
    context: &'static str,
) -> Result<ExitKind, Error> {
    let completion = operation
        .context(context)?
        .wait_with(log_servicing_event)
        .await
        .context(context)?;

    info!("Servicing completed successfully");
    exit_kind(completion)
}

fn log_servicing_event(event: &ServicingEvent) {
    match event {
        ServicingEvent::Started { .. } => info!("Servicing started"),
        ServicingEvent::Log(record) => {
            let target = format!("DAEMON::{}", record.target);
            log!(target: &target, log_level(record.level), "{}", record.message);
        }
        ServicingEvent::Completed(_) => {}
        _ => {}
    }
}

fn log_level(level: LogLevel) -> Level {
    match level {
        LogLevel::Error => Level::Error,
        LogLevel::Warn => Level::Warn,
        LogLevel::Info => Level::Info,
        LogLevel::Debug => Level::Debug,
        LogLevel::Trace => Level::Trace,
        _ => Level::Warn,
    }
}

fn exit_kind(completion: Completion) -> Result<ExitKind, Error> {
    match completion.reboot {
        RebootDisposition::Started => {
            info!("A reboot has been started by Trident");
            // E2E A/B update tests depend on this exact message.
            debug!("Requesting reboot");
            Ok(ExitKind::Done)
        }
        RebootDisposition::Required => {
            info!("A reboot is required to complete the operation");
            Ok(ExitKind::NeedsReboot)
        }
        RebootDisposition::NotRequired => {
            info!("No reboot is required to complete the operation");
            Ok(ExitKind::Done)
        }
        disposition => bail!("Unsupported reboot disposition: {disposition:?}"),
    }
}

fn parse_image_hash(hash: Option<&str>) -> Result<Option<Sha384Hash>, Error> {
    let hash = hash
        .map(ImageSha384::new)
        .transpose()
        .map_err(|error| anyhow!("{error:?}"))?;
    match hash {
        None | Some(ImageSha384::Ignored) => Ok(None),
        Some(ImageSha384::Checksum(hash)) => Ok(Some(hash)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA384: &str =
        "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    #[test]
    fn completion_maps_reboot_disposition() {
        let completion = |reboot| Completion {
            reboot,
            servicing_kind: None,
            image_hash: None,
        };

        assert_eq!(
            exit_kind(completion(RebootDisposition::NotRequired)).unwrap(),
            ExitKind::Done
        );
        assert_eq!(
            exit_kind(completion(RebootDisposition::Started)).unwrap(),
            ExitKind::Done
        );
        assert_eq!(
            exit_kind(completion(RebootDisposition::Required)).unwrap(),
            ExitKind::NeedsReboot
        );
    }

    #[test]
    fn image_hash_preserves_cli_forms() {
        assert_eq!(parse_image_hash(None).unwrap(), None);
        assert_eq!(parse_image_hash(Some("ignored")).unwrap(), None);
        assert_eq!(
            parse_image_hash(Some(&format!("sha384:{SHA384}")))
                .unwrap()
                .unwrap()
                .as_str(),
            SHA384
        );
        parse_image_hash(Some(SHA384)).unwrap_err();
    }
}
