//! Thin Kubernetes client wrapper for Harpoon's node self-patching protocol.
//!
//! Implements the Node get/watch/patch access described in the current
//! accepted design (`accepted-design-v2.md`).
//!
//! The design calls for get/patch access to exactly one Node object (§2.2–§2.6).
//! Node changes are observed via the Kubernetes watch API (`kube::runtime::watcher`)
//! rather than polling, so annotation updates are delivered promptly and without
//! placing repeated load on the API server. `watch_poll_interval` still bounds
//! how quickly the watcher notices a dropped/re-established connection (used
//! as the watcher's backoff ceiling) and how often the fake test API server
//! needs to support being polled if it does not support real watches.

use std::{collections::BTreeMap, path::Path};

use anyhow::Context;
use futures::{stream::BoxStream, StreamExt, TryStreamExt};
use k8s_openapi::api::core::v1::Node;
use kube::{
    api::{Patch, PatchParams},
    config::{KubeConfigOptions, Kubeconfig},
    error::ErrorResponse,
    runtime::{watcher, WatchStreamExt},
    Api, Client, Config,
};
use serde_json::json;

use crate::config::KubernetesConfig;

#[derive(Debug, thiserror::Error)]
pub enum K8sClientError {
    #[error("failed to build Kubernetes client config: {0}")]
    Config(#[from] anyhow::Error),
    #[error("node object no longer exists")]
    NodeGone,
    #[error("failed Kubernetes API call: {0}")]
    Api(#[source] kube::Error),
    #[error("Kubernetes watch stream failed: {0}")]
    Watch(#[from] kube::runtime::watcher::Error),
}

#[derive(Clone)]
pub struct NodeClient {
    api: Api<Node>,
    poll_interval: std::time::Duration,
    cluster_url: String,
}

impl NodeClient {
    pub async fn new(config: &KubernetesConfig) -> Result<Self, K8sClientError> {
        let client_config = load_client_config(config).await?;
        let cluster_url = client_config.cluster_url.to_string();
        let client = Client::try_from(client_config).map_err(anyhow::Error::new)?;
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
        let watcher_config = watcher::Config::default()
            .fields(&format!("metadata.name={name}"))
            .timeout(self.poll_interval.as_secs().max(1) as u32);

        watcher(self.api.clone(), watcher_config)
            .default_backoff()
            .touched_objects()
            .map_err(K8sClientError::from)
            .boxed()
    }
}

fn map_kube_error(err: kube::Error) -> K8sClientError {
    if matches!(&err, kube::Error::Api(ErrorResponse { code: 404, .. })) {
        K8sClientError::NodeGone
    } else {
        K8sClientError::Api(err)
    }
}

async fn load_client_config(config: &KubernetesConfig) -> Result<Config, anyhow::Error> {
    let path = Path::new(&config.kubeconfig);
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
    use kube::error::ErrorResponse;

    use super::*;

    #[test]
    fn maps_404_to_node_gone() {
        let err = kube::Error::Api(ErrorResponse {
            status: "Failure".to_string(),
            message: "nodes \"n\" not found".to_string(),
            reason: "NotFound".to_string(),
            code: 404,
        });

        assert!(matches!(map_kube_error(err), K8sClientError::NodeGone));
    }

    #[test]
    fn leaves_other_api_errors_as_api() {
        let err = kube::Error::Api(ErrorResponse {
            status: "Failure".to_string(),
            message: "forbidden".to_string(),
            reason: "Forbidden".to_string(),
            code: 403,
        });

        assert!(matches!(map_kube_error(err), K8sClientError::Api(_)));
    }
}
