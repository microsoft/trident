//! The legacy one-shot Omaha flow: query the Nebraska/Omaha server once,
//! and if an update is offered, call tridentd's combined `Update()` RPC
//! once and exit. No Kubernetes/annotation involvement.

use anyhow::{anyhow, Context, Error};
use log::{debug, info, warn};
use semver::Version;
use tokio::task;

use crate::{
    core::{
        config::AgentConfig,
        nebraska::{CheckOutcome, Client},
        trident::TridentClient,
        version::{self, FALLBACK_ALWAYS_VERSION},
    },
    IdSource,
};

/// Historical one-shot flow: query the Nebraska/Omaha server at
/// `config.nebraska.endpoint` once, and if an update is offered, call
/// tridentd's combined `Update()` RPC once and exit. No Kubernetes/annotation
/// involvement.
pub async fn run_omaha_only(config: &AgentConfig) -> Result<(), Error> {
    let endpoint = config.nebraska.endpoint.clone().ok_or_else(|| {
        anyhow!("no Nebraska endpoint configured: set TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT")
    })?;

    // Client::check_for_update() is a blocking call (reqwest::blocking under
    // the hood, see nebraska::transport) - calling it directly from this
    // async fn can panic ("Cannot drop a runtime in a context where blocking
    // is not allowed") because reqwest::blocking spins up its own inner
    // Tokio runtime per call, which isn't safe to tear down from inside an
    // already-running async task. Run it on a dedicated blocking thread.
    let app_id = config.nebraska.app_id.clone();
    let track = config.nebraska.track.clone();
    let machine_id = crate::build_machine_id(IdSource::MachineIdHashed)?;
    let current_version_raw = version::current_active_version()?;
    let current_version = Version::parse(&current_version_raw).unwrap_or_else(|err| {
        warn!(
            "current version {current_version_raw:?} is not valid semver ({err}); reporting 0.0.0 to Nebraska"
        );
        Version::parse(FALLBACK_ALWAYS_VERSION).expect("invariant: FALLBACK_ALWAYS_VERSION is valid semver")
    });
    let outcome = task::spawn_blocking(move || {
        let client = Client::new(endpoint, app_id, track, machine_id);
        client.check_for_update(&current_version)
    })
    .await
    .context("Nebraska query task panicked")?
    .context("Nebraska query failed")?;

    match outcome {
        CheckOutcome::UpToDate | CheckOutcome::UpdateInProgress => {
            debug!("No update available from Nebraska");
            Ok(())
        }
        CheckOutcome::UpdateAvailable(offer) => {
            info!("Triggering one-shot Omaha update to {}", offer.version);
            // Nebraska's reported hash is required, not optional: it is
            // Trident's only signal to verify the downloaded image's identity
            // before installing it, so a missing hash is a hard failure here
            // rather than a silently-skipped check. See
            // `PackageHash::to_cosi_sha384` for why this needs converting
            // (our Nebraska deployment stores a base64 SHA-384 in the field
            // Omaha calls `sha1`) rather than being forwarded as-is.
            let hash = offer
                .primary
                .hash
                .as_ref()
                .ok_or_else(|| {
                    anyhow!(
                        "Nebraska offered {} with no package hash for '{}'; refusing to update without an integrity check",
                        offer.version,
                        offer.primary.name
                    )
                })?
                .to_cosi_sha384()
                .context("Nebraska-reported hash is not a usable SHA-384")?;
            let mut client = TridentClient::connect(&config.trident.socket).await?;
            let combined_timeout =
                config.orchestration.stage_timeout + config.orchestration.finalize_timeout;
            client
                .update(&offer.primary.url, Some(&hash), combined_timeout)
                .await?;
            Ok(())
        }
    }
}
