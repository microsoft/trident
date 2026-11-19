use anyhow::{anyhow, Context, Error};
use log::info;
use tokio::task;

use trident_acl_agent::{
    annotations::k8s::NodeClient,
    check_nebraska_reachable,
    core::{
        config::AgentConfig,
        nebraska,
        trident::{self, TridentClient},
    },
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
            // Single, fail-fast attempt - appropriate for a one-shot
            // diagnostic (unlike the long-running orchestrator, which
            // retries a transient kubelet-cert failure with backoff; see
            // KubernetesConfig::resolve_node_name_once's doc comment).
            let node_name = config
                .kubernetes
                .resolve_node_name_once()
                .context("failed to resolve node_name")?;
            // Report the actually-resolved server (kubeconfig's own server,
            // unless overridden by kubernetes.api_server), not a value
            // guessed from config - the two only match when an override is
            // set.
            let cluster_url = client.cluster_url();
            client
                .get_node(&node_name)
                .await
                .with_context(|| {
                    format!(
                        "failed to reach Kubernetes API server at {cluster_url} (get Node {node_name:?})"
                    )
                })?;
            info!("kubernetes: reached API server at {cluster_url} and fetched Node {node_name:?}");
        }
        ConnectionTarget::Tridentd => {
            let socket = trident::redacted_socket(&config.trident.socket);
            TridentClient::connect(&config.trident.socket)
                .await
                .with_context(|| format!("failed to reach tridentd at {socket}"))?;
            info!("tridentd: connected to {socket}");
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
            //
            // Probes share affinity without starting a new update interval.
            let endpoint_for_task = endpoint.clone();
            let track = config.nebraska.track.clone();
            let cookie_store = nebraska::CookieStore::new(config.orchestration.cookie_path.clone());
            let cookie_enabled = config.orchestration.cookie_enabled;
            let cookie_lifetime = config.orchestration.cookie_lifetime;
            task::spawn_blocking(move || {
                check_nebraska_reachable(
                    &endpoint_for_task,
                    &app_id,
                    &track,
                    IdSource::MachineIdHashed,
                    &cookie_store,
                    cookie_enabled,
                    cookie_lifetime,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tridentd_connection_error_redacts_socket_in_context_and_source() {
        let mut config = AgentConfig::default();
        config.trident.socket = "invalid-socket-secret".to_string();
        let error = validate_connection(ConnectionTarget::Tridentd, &config)
            .await
            .expect_err("invalid socket should fail validation");
        let message = format!("{error:#}");
        assert!(message.contains("failed to reach tridentd at <invalid URI>"));
        assert!(message.contains("failed to connect to trident socket <invalid URI>"));
        assert!(!message.contains("invalid-socket-secret"));
        assert_eq!(config.trident.socket, "invalid-socket-secret");
    }
}
