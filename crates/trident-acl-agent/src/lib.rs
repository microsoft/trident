//! # trident-acl-agent
//!
//! One-shot Omaha/Nebraska update client. Queries the configured Nebraska
//! server once and, if an update is offered, calls tridentd's combined
//! `Update()` RPC once and exits. No Kubernetes/annotation involvement -
//! this is the "legacy" flow that predates the AKS annotation-driven
//! orchestrator (see `trident-aks-agent`, which now owns that flow and its
//! own systemd unit). Ships with no unit file; configured entirely via
//! `TRIDENT_ACL_AGENT_*` environment variables (see [`config`]).
//!
//! All Omaha/Nebraska protocol traffic goes through `trident-agent-core`'s
//! [`trident_agent_core::nebraska`] client, shared with `trident-aks-agent`.

use anyhow::Context;
use semver::Version;

use trident_agent_core::{
    build_machine_id,
    nebraska::{CheckOutcome, Client},
    trident::TridentClient,
    IdSource,
};

pub mod config;

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
