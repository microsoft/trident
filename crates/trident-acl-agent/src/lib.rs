//! # Harpoon
//!
//! Harpoon is Trident's ACL update sidecar. Historically it was a one-shot
//! Omaha client that called Trident's combined `Update()` RPC once and exited.
//! This crate now defaults to the AKS annotation protocol described in the
//! local design doc (`aks-rp ↔ trident-acl-agent`, especially §3–§6 and
//! §12–§13), while preserving the original `omaha-only` mode as an explicit
//! opt-out (see `config::GoalSource`).
//!
//! All Omaha/Nebraska protocol traffic (both `omaha-only` and annotation mode)
//! goes through the [`nebraska`] client module, a self-contained, reusable
//! implementation of the Nebraska/Omaha update protocol. It is usable both by
//! this crate's agent binary and by a future Trident ACL Agent that
//! orchestrates updates differently.

use anyhow::Context;
use semver::Version;

pub mod annotations;
pub mod config;
pub mod error;
pub mod id;
pub mod k8s;
pub mod nebraska;
pub mod orchestrator;
pub mod state;
pub mod trident;

/// Only built for `cargo test` (relies on trident-proto's `server` feature,
/// which is only enabled via trident-acl-agent's dev-dependencies - see
/// mock_tridentd.rs's module docs).
#[cfg(test)]
pub mod mock_tridentd;

use error::HarpoonError;
use nebraska::{CheckOutcome, Client, MachineId, NebraskaError};
use trident::TridentClient;

pub use id::IdSource;

// Deliberately invalid sentinels, mirroring DEFAULT_NEBRASKA_ENDPOINT's
// `.invalid` domain trick: a deployment that forgets to configure (or
// override via the update-request annotation's `appId`/`track` fields) a
// real app_id/track fails loudly against Nebraska instead of silently
// querying a real-looking but wrong app/group.
pub const DEFAULT_NEBRASKA_APP_ID: &str = "00000000-0000-0000-0000-000000000000";
pub const DEFAULT_NEBRASKA_TRACK: &str = "unspecified";

/// Builds a validated [`MachineId`] from an [`IdSource`], translating the
/// crate's own machine-id/hostname read errors into a single [`HarpoonError`].
fn build_machine_id(source: IdSource) -> Result<MachineId, HarpoonError> {
    MachineId::new(source.produce_id()?).map_err(|err| HarpoonError::Nebraska(err.to_string()))
}

/// Historical one-shot flow: query the Nebraska/Omaha server at
/// `config.nebraska.endpoint` once, and if an update is offered, call
/// tridentd's combined `Update()` RPC once and exit. No Kubernetes/annotation
/// involvement.
pub async fn run_omaha_only(config: &config::AgentConfig) -> Result<(), anyhow::Error> {
    let endpoint = config.nebraska.endpoint.clone().ok_or_else(|| {
        anyhow::anyhow!(
            "no Nebraska endpoint configured: pass <url> on the CLI or set TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT"
        )
    })?;

    // Client::check_for_update() is a blocking call (reqwest::blocking under
    // the hood, see nebraska::transport) - calling it directly from this
    // async fn can panic ("Cannot drop a runtime in a context where blocking
    // is not allowed") because reqwest::blocking spins up its own inner
    // Tokio runtime per call, which isn't safe to tear down from inside an
    // already-running async task. Run it on a dedicated blocking thread.
    let app_id = config.nebraska.app_id.clone();
    let track = config.nebraska.track.clone();
    let machine_id = build_machine_id(IdSource::MachineIdHashed)?;
    let outcome = tokio::task::spawn_blocking(move || {
        let client = Client::new(endpoint, app_id, track, machine_id);
        client.check_for_update(&Version::new(0, 0, 0))
    })
    .await
    .context("Nebraska query task panicked")?
    .map_err(|err| anyhow::anyhow!("Nebraska query failed: {err}"))?;

    match outcome {
        CheckOutcome::UpToDate | CheckOutcome::UpdateInProgress => {
            log::debug!("No update available from Nebraska");
            Ok(())
        }
        CheckOutcome::UpdateAvailable(offer) => {
            log::info!("Triggering one-shot Omaha update to {}", offer.version);
            let mut client = TridentClient::connect(&config.trident.socket).await?;
            let combined_timeout =
                config.orchestration.stage_timeout + config.orchestration.finalize_timeout;
            // Integrity of the downloaded image is verified by Trident itself
            // via the image's own COSI metadata, so the Nebraska-reported hash
            // (offer.primary.hash) is not passed here.
            client
                .update(&offer.primary.url, None, combined_timeout)
                .await?;
            Ok(())
        }
    }
}

/// Checks that the Omaha/Nebraska server at `url` is reachable and speaking
/// the Omaha protocol, without treating any app-level result (including a
/// non-OK app/update-check status) as a failure. Unlike
/// [`Client::check_for_update`], this only fails on network/transport
/// problems or a response that isn't well-formed Omaha XML -- it's meant for
/// a pure "can we talk to this server at all" check (e.g.
/// `--validate-connection nebraska`), not for deciding whether an update is
/// available.
pub fn check_nebraska_reachable(
    url: &url::Url,
    app_id: &str,
    track: &str,
    machine_id_source: IdSource,
) -> Result<(), HarpoonError> {
    let machine_id = build_machine_id(machine_id_source)?;
    let client = Client::new(url.clone(), app_id, track, machine_id);
    match client.check_for_update(&Version::new(0, 0, 0)) {
        Ok(_) => Ok(()),
        // A well-formed response reporting a non-OK app/update-check status
        // still proves the server is reachable and speaking Omaha; only a
        // transport/parse-level failure means it is not.
        Err(NebraskaError::ServerError(_)) => Ok(()),
        Err(err) => Err(HarpoonError::Nebraska(err.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_nebraska_reachable_succeeds_on_error_app_status() {
        // check_nebraska_reachable() is meant to be a pure "can we reach this
        // server and does it speak Omaha" check, unlike check_for_update()
        // which also validates app-level semantics. A well-formed response
        // with a non-OK app status should still count as "reachable" here,
        // even though check_for_update() would reject the same response as a
        // NebraskaError::ServerError.
        let mut server = mockito::Server::new();

        let omaha_mock = server
            .mock("POST", "/")
            .with_status(200)
            .with_body(indoc::indoc! {r#"
                <?xml version="1.0" encoding="UTF-8"?>
                <response protocol="3.0" server="mock">
                    <daystart elapsed_seconds="0"/>
                    <app appid="test" status="error-unknownApplication">
                        <updatecheck status="error-internal"></updatecheck>
                    </app>
                </response>"#})
            .expect(1)
            .create();

        check_nebraska_reachable(
            &url::Url::parse(&server.url()).unwrap(),
            "test",
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap();

        omaha_mock.assert();
    }

    #[test]
    fn test_check_nebraska_reachable_fails_on_transport_error() {
        let err = check_nebraska_reachable(
            // Port 0 never accepts a connection.
            &url::Url::parse("http://127.0.0.1:0/").unwrap(),
            "test",
            "track",
            IdSource::MachineIdHashed,
        )
        .unwrap_err();
        assert!(matches!(err, HarpoonError::Nebraska(_)));
    }
}
