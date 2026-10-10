//! Thin Kubernetes client wrapper for trident-acl-agent's node self-patching
//! protocol.
//!
//! Implements the Node get/watch/patch access.
//!
//! The design calls for get/patch access to exactly one Node object.
//! Node changes are observed via the Kubernetes watch API (`kube::runtime::watcher`)
//! rather than polling, so annotation updates are delivered promptly and without
//! placing repeated load on the API server. `watch_poll_interval` only
//! bounds the watch request's `timeoutSeconds` (see
//! [`NodeClient::watch_node`]) - it floors how long a healthy watch
//! connection is held open before a routine reconnect, and how often the
//! fake test API server needs to support being polled if it does not
//! support real watches. It does not influence reconnect/backoff timing
//! after a dropped or failed watch; that is governed entirely by
//! `kube::runtime::watcher`'s built-in `default_backoff()`.

use std::{collections::BTreeMap, time::Duration};

use anyhow::{Context, Error};
use futures::{stream::BoxStream, StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Node;
use kube::{
    api::{Patch, PatchParams},
    config::{KubeConfigOptions, Kubeconfig},
    error::ErrorResponse,
    runtime::{
        watcher::{self, Error as WatchError},
        WatchStreamExt,
    },
    Api, Client, Config, Error as KubeError,
};
use log::warn;
use reqwest::StatusCode;
use serde_json::json;
use thiserror::Error;

use osutils::dmi;
use sysdefs::osuuid::OsUuid;

use crate::core::config::KubernetesConfig;

/// Floor for the Kubernetes watch request's `timeoutSeconds`, decoupled from
/// `poll_interval` (see [`NodeClient::watch_node`]). Sits comfortably under
/// typical apiserver request-timeout defaults (~300s) while avoiding
/// reconnect churn on an otherwise-healthy watch.
const WATCH_TIMEOUT_SECS: u32 = 290;

#[derive(Debug, Error)]
pub enum K8sClientError {
    #[error("failed to build Kubernetes client config: {0}")]
    Config(#[from] Error),
    #[error("node object no longer exists")]
    NodeGone,
    #[error("failed Kubernetes API call: {0}")]
    Api(#[source] KubeError),
    #[error("Kubernetes watch stream failed: {0}")]
    Watch(#[from] WatchError),
}

#[derive(Clone)]
pub struct NodeClient {
    api: Api<Node>,
    poll_interval: Duration,
    cluster_url: String,
    /// Mirrors [`KubernetesConfig::validate_node_uuid`]: enables
    /// [`verify_node_identity`]'s `systemUUID`-vs-local-hardware check in
    /// both [`NodeClient::get_node`] and [`NodeClient::watch_node`].
    validate_identity: bool,
}

impl NodeClient {
    pub async fn new(config: &KubernetesConfig) -> Result<Self, K8sClientError> {
        let client_config = load_client_config(config).await?;
        let cluster_url = client_config.cluster_url.to_string();
        let client = Client::try_from(client_config).map_err(Error::new)?;
        Ok(Self {
            api: Api::all(client),
            poll_interval: config.watch_poll_interval,
            cluster_url,
            validate_identity: config.validate_node_uuid,
        })
    }

    pub fn cluster_url(&self) -> &str {
        &self.cluster_url
    }

    pub async fn get_node(&self, name: &str) -> Result<Node, K8sClientError> {
        let node = self.api.get(name).await.map_err(map_kube_error)?;
        if self.validate_identity {
            verify_node_identity(&node, name, dmi::read_product_uuid())?;
        }
        Ok(node)
    }

    pub async fn patch_node_labels(
        &self,
        name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<Node, K8sClientError> {
        let patch = json!({ "metadata": { "labels": labels } });
        self.api
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
            .map_err(map_kube_error)
    }

    pub async fn patch_node_annotations(
        &self,
        name: &str,
        annotations: BTreeMap<String, String>,
    ) -> Result<Node, K8sClientError> {
        let patch = json!({ "metadata": { "annotations": annotations } });
        self.api
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
            .map_err(map_kube_error)
    }

    pub async fn patch_node_metadata(
        &self,
        name: &str,
        labels: BTreeMap<String, Option<String>>,
        annotations: BTreeMap<String, Option<String>>,
    ) -> Result<Node, K8sClientError> {
        let patch = json!({
            "metadata": {
                "labels": labels,
                "annotations": annotations,
            }
        });
        self.api
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await
            .map_err(map_kube_error)
    }

    pub fn watch_node(&self, name: String) -> BoxStream<'static, Result<Node, K8sClientError>> {
        // The watch request's timeoutSeconds bounds how long the API server
        // holds the connection open before closing it, at which point
        // `kube::runtime::watcher` reconnects. `poll_interval` defaults to a
        // couple of seconds (fine for a fallback-polling cadence), so using
        // it directly here would force a reconnect every couple of seconds
        // even on a perfectly healthy watch. Floor the request timeout at
        // WATCH_TIMEOUT_SECS instead, while still honoring a larger
        // configured `poll_interval` if one is ever set. Note this only
        // affects the cadence of routine reconnects on a healthy watch -
        // `default_backoff()` below (not `poll_interval`) governs
        // retry/backoff timing after a dropped or failed watch.
        let timeout_secs = self
            .poll_interval
            .as_secs()
            .max(u64::from(WATCH_TIMEOUT_SECS)) as u32;
        let watcher_config = watcher::Config::default()
            .fields(&format!("metadata.name={name}"))
            .timeout(timeout_secs);

        // Applied per watch event below, mirroring get_node: a Node emitted
        // by the watch whose systemUUID doesn't match this machine is just
        // as stale/wrong as one returned by a direct get, and must be routed
        // into the same NodeGone handling rather than silently reconciling
        // against the wrong node.
        let validate_identity = self.validate_identity;

        watcher::watcher(self.api.clone(), watcher_config)
            .default_backoff()
            .touched_objects()
            .map_err(map_watch_error)
            .and_then(move |node| {
                let name = name.clone();
                async move {
                    if validate_identity {
                        verify_node_identity(&node, &name, dmi::read_product_uuid())?;
                    }
                    Ok(node)
                }
            })
            .boxed()
    }
}

/// True if `resp` represents a Kubernetes API 404 (Not Found).
fn is_not_found_response(resp: &ErrorResponse) -> bool {
    resp.code == StatusCode::NOT_FOUND.as_u16()
}

/// True if `err` is a Kubernetes API error wrapping a 404 (Not Found).
fn is_not_found(err: &KubeError) -> bool {
    matches!(err, KubeError::Api(resp) if is_not_found_response(resp))
}

/// Confirms `node`'s reported `status.nodeInfo.systemUUID` matches this
/// machine's own hardware product UUID from [`dmi::read_product_uuid`].
/// A mismatch means the Node object we
/// fetched by name does not describe this machine - e.g. the Node name was
/// recycled onto different hardware - so it's treated identically to the
/// Node not existing ([`K8sClientError::NodeGone`]).
///
/// If the local product UUID can't be read, the check is skipped (logged at
/// warn) rather than failing closed, so a host without DMI data (e.g. some
/// VM/container test environments) doesn't lose all Node access.
///
/// Likewise, if the Node's reported `systemUUID` is empty, the check is
/// skipped. kubelet populates this field via cadvisor reading the same
/// `product_uuid` file; if that read ever fails on the kubelet's side,
/// cadvisor logs an error but still lets node registration proceed with an
/// empty `systemUUID` rather than surfacing the error. An empty value is
/// therefore evidence kubelet couldn't determine the UUID - not evidence the
/// node is a different machine - so treating it as a mismatch would risk a
/// false positive that locks us out of an otherwise-healthy node forever.
fn verify_node_identity(
    node: &Node,
    name: &str,
    local_uuid: Result<OsUuid, Error>,
) -> Result<(), K8sClientError> {
    let local_uuid = match local_uuid {
        Ok(uuid) => uuid,
        Err(err) => {
            warn!(
                "failed to read local product uuid, skipping Node {name:?} identity verification: {err:#}"
            );
            return Ok(());
        }
    };

    let local_uuid = local_uuid.to_string();
    if local_uuid.trim().is_empty() {
        warn!("Local product uuid is empty, skipping Node {name:?} identity verification");
        return Ok(());
    }

    let node_uuid = node
        .status
        .as_ref()
        .and_then(|status| status.node_info.as_ref())
        .map(|node_info| node_info.system_uuid.as_str())
        .unwrap_or_default();

    if node_uuid.is_empty() {
        warn!("Node {name:?} status.nodeInfo.systemUUID is empty, skipping identity verification");
        return Ok(());
    }

    if !OsUuid::from(node_uuid)
        .to_string()
        .eq_ignore_ascii_case(&local_uuid)
    {
        warn!(
            "Node {name:?} status.nodeInfo.systemUUID {node_uuid:?} does not match local product uuid {local_uuid:?}; treating node as not found"
        );
        return Err(K8sClientError::NodeGone);
    }

    Ok(())
}

fn map_kube_error(err: KubeError) -> K8sClientError {
    if is_not_found(&err) {
        K8sClientError::NodeGone
    } else {
        K8sClientError::Api(err)
    }
}

/// The watch path can fail three different ways (initial list, a watch-event
/// error body, or a dropped watch connection), each wrapping either a bare
/// ErrorResponse or a full kube_client::Error. A Node 404 can surface through
/// any of them (e.g. the node is deleted mid-watch, or the initial LIST used
/// to seed the watch 404s). Treat all three the same way map_kube_error
/// treats a direct API 404: classify as NodeGone so is_node_gone_error() can
/// route it into await_node_recreation() instead of retrying it indefinitely
/// via default_backoff().
fn map_watch_error(err: WatchError) -> K8sClientError {
    let is_404 = match &err {
        WatchError::WatchError(resp) => is_not_found_response(resp),
        WatchError::WatchFailed(e)
        | WatchError::InitialListFailed(e)
        | WatchError::WatchStartFailed(e) => is_not_found(e),
        WatchError::NoResourceVersion => false,
    };
    if is_404 {
        K8sClientError::NodeGone
    } else {
        K8sClientError::Watch(err)
    }
}

async fn load_client_config(config: &KubernetesConfig) -> Result<Config, Error> {
    let path = config.kubeconfig.as_path();
    let kubeconfig = Kubeconfig::read_from(path)
        .with_context(|| format!("failed to read kubeconfig {}", path.display()))?;
    let mut client_config =
        Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default()).await?;
    if let Some(api_server) = &config.api_server {
        client_config.cluster_url = api_server.as_str().parse()?;
    }
    Ok(client_config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error_response(code: u16) -> ErrorResponse {
        ErrorResponse {
            status: "Failure".to_string(),
            message: if code == 404 {
                "nodes \"n\" not found".to_string()
            } else {
                "forbidden".to_string()
            },
            reason: if code == 404 {
                "NotFound".to_string()
            } else {
                "Forbidden".to_string()
            },
            code,
        }
    }

    #[test]
    fn maps_404_to_node_gone() {
        let err = KubeError::Api(error_response(404));

        assert!(matches!(map_kube_error(err), K8sClientError::NodeGone));
    }

    #[test]
    fn leaves_other_api_errors_as_api() {
        let err = KubeError::Api(error_response(403));

        assert!(matches!(map_kube_error(err), K8sClientError::Api(_)));
    }

    #[test]
    fn maps_watch_error_404_to_node_gone() {
        let err = WatchError::WatchError(error_response(404));

        assert!(matches!(map_watch_error(err), K8sClientError::NodeGone));
    }

    #[test]
    fn maps_watch_failed_404_to_node_gone() {
        let err = WatchError::WatchFailed(KubeError::Api(error_response(404)));

        assert!(matches!(map_watch_error(err), K8sClientError::NodeGone));
    }

    #[test]
    fn maps_initial_list_failed_404_to_node_gone() {
        let err = WatchError::InitialListFailed(KubeError::Api(error_response(404)));

        assert!(matches!(map_watch_error(err), K8sClientError::NodeGone));
    }

    #[test]
    fn leaves_other_watch_errors_as_watch() {
        let err = WatchError::WatchError(error_response(403));

        assert!(matches!(map_watch_error(err), K8sClientError::Watch(_)));
    }

    #[test]
    fn leaves_other_watch_failed_as_watch() {
        let err = WatchError::WatchFailed(KubeError::Api(error_response(403)));

        assert!(matches!(map_watch_error(err), K8sClientError::Watch(_)));
    }

    fn node_with_system_uuid(uuid: &str) -> Node {
        use k8s_openapi::api::core::v1::{NodeStatus, NodeSystemInfo};

        Node {
            status: Some(NodeStatus {
                node_info: Some(NodeSystemInfo {
                    system_uuid: uuid.to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn matching_system_uuid_is_ok() {
        let node = node_with_system_uuid("1234-ABCD");
        verify_node_identity(&node, "n", Ok(OsUuid::from("1234-ABCD"))).unwrap();
    }

    #[test]
    fn matching_system_uuid_is_case_insensitive() {
        for (local, remote) in [
            ("1234-abcd", "1234-ABCD"),
            (
                "6BA7B810-9DAD-11D1-80B4-00C04FD430C8",
                "6ba7b810-9dad-11d1-80b4-00c04fd430c8",
            ),
        ] {
            let node = node_with_system_uuid(remote);
            verify_node_identity(&node, "n", Ok(OsUuid::from(local))).unwrap();
        }
    }

    #[test]
    fn mismatched_system_uuid_is_node_gone() {
        let node = node_with_system_uuid("5678-EFGH");
        let error = verify_node_identity(&node, "n", Ok(OsUuid::from("1234-ABCD"))).unwrap_err();
        assert!(matches!(error, K8sClientError::NodeGone), "got {error:?}");
    }

    #[test]
    fn missing_node_system_info_skips_verification() {
        verify_node_identity(&Node::default(), "n", Ok(OsUuid::from("1234-ABCD"))).unwrap();
    }

    #[test]
    fn empty_node_system_uuid_skips_verification() {
        let node = node_with_system_uuid("");
        verify_node_identity(&node, "n", Ok(OsUuid::from("1234-ABCD"))).unwrap();
    }

    #[test]
    fn unreadable_local_uuid_skips_verification() {
        let node = node_with_system_uuid("5678-EFGH");
        verify_node_identity(&node, "n", Err(Error::msg("UUID file unavailable"))).unwrap();
    }

    #[test]
    fn empty_local_uuid_skips_verification() {
        let node = node_with_system_uuid("1234-ABCD");
        for local in ["", "  \n"] {
            verify_node_identity(&node, "n", Ok(OsUuid::from(local))).unwrap();
        }
    }
}
