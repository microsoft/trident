//! Thin Kubernetes client wrapper for Harpoon's node self-patching protocol.
//!
//! Implements the Node get/watch/patch access described in
//! `docs/update-trigger-design.md`:
//! https://msazure.visualstudio.com/One/_git/Compute-ACL-Update-Service?version=GC67946fff8f296e10217b70e063c896e6028ea843&path=/docs/update-trigger-design.md
//!
//! The design calls for get/patch access to exactly one Node object (§3–§5).
//! Node changes are observed via the Kubernetes watch API (`kube::runtime::watcher`)
//! rather than polling, so label updates are delivered promptly and without
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
    runtime::{watcher, WatchStreamExt},
    Api, Client, Config,
};
use serde_json::json;

use crate::config::KubernetesConfig;

#[derive(Debug, thiserror::Error)]
pub enum K8sClientError {
    #[error("failed to build Kubernetes client config: {0}")]
    Config(#[from] anyhow::Error),
    #[error("failed Kubernetes API call: {0}")]
    Api(#[from] kube::Error),
    #[error("Kubernetes watch stream failed: {0}")]
    Watch(#[from] kube::runtime::watcher::Error),
}

#[derive(Clone)]
pub struct NodeClient {
    api: Api<Node>,
    poll_interval: std::time::Duration,
}

impl NodeClient {
    pub async fn new(config: &KubernetesConfig) -> Result<Self, K8sClientError> {
        let client_config = load_client_config(config).await?;
        let client = Client::try_from(client_config).map_err(anyhow::Error::new)?;
        Ok(Self {
            api: Api::all(client),
            poll_interval: config.watch_poll_interval,
        })
    }

    pub async fn get_node(&self, name: &str) -> Result<Node, K8sClientError> {
        Ok(self.api.get(name).await?)
    }

    pub async fn patch_node_labels(
        &self,
        name: &str,
        labels: BTreeMap<String, String>,
    ) -> Result<Node, K8sClientError> {
        let patch = json!({ "metadata": { "labels": labels } });
        Ok(self
            .api
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await?)
    }

    pub async fn patch_node_annotations(
        &self,
        name: &str,
        annotations: BTreeMap<String, String>,
    ) -> Result<Node, K8sClientError> {
        let patch = json!({ "metadata": { "annotations": annotations } });
        Ok(self
            .api
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await?)
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
        Ok(self
            .api
            .patch(name, &PatchParams::default(), &Patch::Merge(&patch))
            .await?)
    }

    /// Streams Node updates for `name` using the Kubernetes watch API instead
    /// of polling. `watcher::Config::default().fields(...)` scopes the watch
    /// server-side to the single node we care about, and `.default_backoff()`
    /// governs reconnect timing (capped near `poll_interval`) if the watch
    /// connection drops.
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

async fn load_client_config(config: &KubernetesConfig) -> Result<Config, anyhow::Error> {
    let path = Path::new(&config.kubeconfig);
    let kubeconfig = Kubeconfig::read_from(path)
        .with_context(|| format!("failed to read kubeconfig {}", path.display()))?;
    let mut client_config =
        Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default()).await?;
    // Only override the server URL the kubeconfig already resolved to when a
    // deployment explicitly configures one. A node's own kubeconfig (e.g.
    // /var/lib/kubelet/kubeconfig) already points at the correct cluster API
    // server FQDN, so overriding it unconditionally with a fixed default
    // (like the in-cluster-only `https://kubernetes.default.svc`) would
    // break any deployment running outside a pod's network namespace, where
    // that in-cluster DNS name doesn't resolve.
    if let Some(api_server) = &config.api_server {
        client_config.cluster_url = api_server.as_str().parse()?;
    }
    Ok(client_config)
}
