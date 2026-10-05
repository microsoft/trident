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
use reqwest::StatusCode;
use serde_json::json;
use thiserror::Error;

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
        })
    }

    pub fn cluster_url(&self) -> &str {
        &self.cluster_url
    }

    pub async fn get_node(&self, name: &str) -> Result<Node, K8sClientError> {
        self.api.get(name).await.map_err(map_kube_error)
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

        watcher::watcher(self.api.clone(), watcher_config)
            .default_backoff()
            .touched_objects()
            .map_err(map_watch_error)
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

fn map_kube_error(err: KubeError) -> K8sClientError {
    if is_not_found(&err) {
        K8sClientError::NodeGone
    } else {
        K8sClientError::Api(err)
    }
}

// The watch path can fail three different ways (initial list, a watch-event
// error body, or a dropped watch connection), each wrapping either a bare
// ErrorResponse or a full kube_client::Error. A Node 404 can surface through
// any of them (e.g. the node is deleted mid-watch, or the initial LIST used
// to seed the watch 404s). Treat all three the same way map_kube_error
// treats a direct API 404: classify as NodeGone so is_node_gone_error() can
// route it into await_node_recreation() instead of retrying it indefinitely
// via default_backoff().
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
}
