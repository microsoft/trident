use std::time::Duration;

use anyhow::Error;
use clap::ValueEnum;
use tokio::{sync::mpsc::Sender, time};

use trident_proto::v1::{
    servicing_response::Response as ResponseBody, Completed, Log, LogLevel, RebootStatus,
    ServicingKind, ServicingResponse, Started, StatusCode, TridentError, TridentErrorKind,
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
    response(&tx, ResponseBody::Started(Started {})).await?;
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
            ResponseBody::Log(Log {
                level: level.into(),
                message: message.into(),
                ..Default::default()
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
        ResponseBody::Completed(Completed {
            status: if failed {
                StatusCode::Failure
            } else {
                StatusCode::Success
            }
            .into(),
            error: failed.then(|| TridentError {
                kind: TridentErrorKind::ServicingError.into(),
                message:
                    "DEMO: Installation failed\nFailed to stream the OS image\nConnection timed out"
                        .into(),
                subkind: "demo-network-failure".into(),
                error_message: "Connection timed out".into(),
                ..Default::default()
            }),
            reboot_status: RebootStatus::RebootRequired.into(),
            servicing_kind: Some(ServicingKind::CleanInstall.into()),
            ..Default::default()
        }),
    )
    .await
}

async fn response(tx: &Sender<Event>, response: ResponseBody) -> Result<(), Error> {
    client::send(
        tx,
        Event::Response(ServicingResponse {
            response: Some(response),
            ..Default::default()
        }),
    )
    .await
}
