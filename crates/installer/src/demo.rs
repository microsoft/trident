use std::time::Duration;

use anyhow::Error;
use clap::ValueEnum;
use tokio::{sync::mpsc::Sender, time};

use trident_api::status::ServicingType;
use trident_client::{
    Completion, LogLevel, LogRecord, OperationOutcome, RebootDisposition, RemoteError,
    RemoteErrorCategory, ServicingEvent,
};

use crate::{
    client::{self, Event},
    config::SerialVerbosity,
};

const DEMO_STEP: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(super) enum Scenario {
    Success,
    Failure,
    NoImages,
    AlreadyPresent,
    Disconnect,
}

/// Emits a simulated servicing sequence without connecting to Trident or touching disks.
pub(super) async fn run(scenario: Scenario, tx: Sender<Event>) -> Result<(), Error> {
    client::send(
        &tx,
        Event::Prepared {
            description: "DEMO: StreamDisk from ISO image".into(),
            reboot: false,
            serial_verbosity: SerialVerbosity::Debug,
            stream_image: None,
        },
    )
    .await?;
    match scenario {
        Scenario::NoImages => return client::send(&tx, Event::Error {
            details: "DEMO: No Host Configuration and no COSI images in cosi/.\nTry the simulated shell or a remote-source recovery action.\nNo network requests or machine operations will be performed.".into(),
            uncertain: false,
        }).await,
        Scenario::AlreadyPresent => return client::send(&tx, Event::AlreadyPresent(
            "DEMO: Matching COSI filesystem UUIDs found. No writes performed.".into()
        )).await,
        _ => {}
    }
    response(&tx, ServicingEvent::Started { timestamp: None }).await?;
    for (level, message) in [
        (LogLevel::Info, "Loading image metadata"),
        (LogLevel::Debug, "Inspecting available block devices"),
        (LogLevel::Info, "Preparing storage"),
        (LogLevel::Trace, "Example detailed diagnostic record"),
        (LogLevel::Info, "Streaming the OS image"),
    ] {
        time::sleep(DEMO_STEP).await;
        response(
            &tx,
            ServicingEvent::Log(LogRecord {
                timestamp: None,
                level,
                message: message.into(),
                module: String::new(),
                target: String::new(),
                location: None,
            }),
        )
        .await?;
    }
    time::sleep(DEMO_STEP).await;
    if matches!(scenario, Scenario::Disconnect) {
        return client::send(&tx, Event::Error {
            details: "DEMO: Connection lost before Completed. Outcome unknown; another write must wait until Trident is confirmed idle.".into(),
            uncertain: true,
        }).await;
    }
    let failed = matches!(scenario, Scenario::Failure);
    response(
        &tx,
        ServicingEvent::Completed(if failed {
            OperationOutcome::Failed(RemoteError {
                category: RemoteErrorCategory::Servicing,
                message:
                    "DEMO: Installation failed\nFailed to stream the OS image\nConnection timed out"
                        .into(),
                subkind: "demo-network-failure".into(),
                error_message: "Connection timed out".into(),
                location: None,
            })
        } else {
            OperationOutcome::Succeeded(Completion {
                reboot: RebootDisposition::Required,
                servicing_type: Some(ServicingType::CleanInstall),
                image_hash: None,
            })
        }),
    )
    .await
}

/// Wraps a demo servicing event in the same shape used by a real install.
async fn response(tx: &Sender<Event>, response: ServicingEvent) -> Result<(), Error> {
    client::send(tx, Event::Response(response)).await
}
