use anyhow::{anyhow, Context, Error};
use log::info;
use tokio::task;

use trident_acl_agent::{
    annotations::k8s::NodeClient,
    check_nebraska_reachable,
    core::{config::AgentConfig, nebraska, trident::TridentClient},
    IdSource,
};

use crate::cli::ConnectionTarget;

/// Checks connectivity to exactly one of `target`'s dependencies and returns
/// `Ok(())` on success. The caller (`main`) surfaces any `Err` the normal way
/// (`anyhow`'s `Termination` impl prints the error and exits non-zero), so
/// this function only needs to produce a descriptive error on failure - no
/// explicit `process::exit` is required.
pub async fn validate_connection(
    target: ConnectionTarget,
    config: &AgentConfig,
) -> Result<(), Error> {
    match target {
        ConnectionTarget::Kubernetes => {
            let client = NodeClient::new(&config.kubernetes)
                .await
                .context("failed to build Kubernetes client")?;
            // Report the actually-resolved server (kubeconfig's own server,
            // unless overridden by kubernetes.api_server), not a value
            // guessed from config - the two only match when an override is
            // set.
            let cluster_url = client.cluster_url();
            client
                .get_node(&config.kubernetes.node_name)
                .await
                .with_context(|| {
                    format!(
                        "failed to reach Kubernetes API server at {cluster_url} (get Node {:?})",
                        config.kubernetes.node_name
                    )
                })?;
            info!(
                "kubernetes: reached API server at {cluster_url} and fetched Node {:?}",
                config.kubernetes.node_name
            );
        }
        ConnectionTarget::Tridentd => {
            TridentClient::connect(&config.trident.socket)
                .await
                .with_context(|| {
                    format!("failed to reach tridentd at {}", config.trident.socket)
                })?;
            info!("tridentd: connected to {}", config.trident.socket);
        }
        ConnectionTarget::Nebraska => {
            let endpoint = config.nebraska.endpoint.clone().ok_or_else(|| {
                anyhow!(
                    "nebraska.endpoint is not configured (set TRIDENT_ACL_AGENT_NEBRASKA_ENDPOINT)"
                )
            })?;
            let app_id = config.nebraska.app_id.clone();
            // check_nebraska_reachable() is a blocking call (reqwest::blocking
            // under the hood, see nebraska::transport) - calling it directly from this
            // async fn can panic ("Cannot drop a runtime in a context where
            // blocking is not allowed") because reqwest::blocking spins up
            // its own inner Tokio runtime per call, which isn't safe to tear
            // down from inside an already-running async task. Run it on a
            // dedicated blocking thread instead.
            //
            // Deliberately uses check_nebraska_reachable() rather than
            // query_for_update(): the latter also validates app-level
            // semantics (app ID match, non-error app/update-check status),
            // which would make this a "can we get a valid update check" test
            // rather than the pure reachability check documented on
            // ConnectionTarget::Nebraska above.
            let endpoint_for_task = endpoint.clone();
            let track = config.nebraska.track.clone();
            task::spawn_blocking(move || {
                check_nebraska_reachable(
                    &endpoint_for_task,
                    &app_id,
                    &track,
                    IdSource::MachineIdHashed,
                )
            })
            .await
            .context("Nebraska connectivity check task panicked")?
            .with_context(|| {
                format!(
                    "failed to reach Nebraska server at {}",
                    nebraska::redacted(&endpoint)
                )
            })?;
            info!(
                "nebraska: reached server at {}",
                nebraska::redacted(&endpoint)
            );
        }
    }
    Ok(())
}
