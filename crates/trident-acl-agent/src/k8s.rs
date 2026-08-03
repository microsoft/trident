//! Thin Kubernetes client wrapper for Harpoon's node self-patching protocol.
//!
//! The design calls for get/patch access to exactly one Node object (§3–§5).
//! For v1 we use a simple poll loop instead of a streaming watch. This keeps
//! the implementation predictable for both production and the tester's fake API
//! server while still reacting within a couple of seconds.

use std::{collections::BTreeMap, path::Path};

use anyhow::Context;
use futures::{stream::BoxStream, StreamExt};
use k8s_openapi::api::core::v1::Node;
use kube::{
    api::{Patch, PatchParams},
    config::{KubeConfigOptions, Kubeconfig},
    Api, Client, Config,
};
use serde_json::json;
use tokio_stream::wrappers::IntervalStream;

use crate::config::KubernetesConfig;

#[derive(Debug, thiserror::Error)]
pub enum K8sClientError {
    #[error("failed to build Kubernetes client config: {0}")]
    Config(#[from] anyhow::Error),
    #[error("failed Kubernetes API call: {0}")]
    Api(#[from] kube::Error),
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

    pub fn watch_node(&self, name: String) -> BoxStream<'static, Result<Node, K8sClientError>> {
        let client = self.clone();
        IntervalStream::new(tokio::time::interval(self.poll_interval))
            .then(move |_| {
                let client = client.clone();
                let name = name.clone();
                async move { client.get_node(&name).await }
            })
            .boxed()
    }
}

async fn load_client_config(config: &KubernetesConfig) -> Result<Config, anyhow::Error> {
    let mut client_config = if let Some(path) = config.kubeconfig.as_deref() {
        let path = Path::new(path);
        if path.exists() {
            let kubeconfig = Kubeconfig::read_from(path)
                .with_context(|| format!("failed to read kubeconfig {}", path.display()))?;
            Config::from_custom_kubeconfig(kubeconfig, &KubeConfigOptions::default()).await?
        } else {
            // Assumption: production deployments use a dedicated ServiceAccount
            // identity. Falling back to in-cluster/default inference keeps that
            // path working instead of assuming kubelet credentials are present.
            Config::infer().await?
        }
    } else {
        Config::infer().await?
    };

    client_config.cluster_url = config.api_server.as_str().parse()?;
    Ok(client_config)
}
