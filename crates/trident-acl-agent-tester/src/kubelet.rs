//! Fake kubelet helper for tester runs.
//!
//! The marker-file default is repo-local instead of `/tmp` because this runtime
//! forbids temporary-directory writes. The behavior is the same: the reboot shim
//! writes the marker, the proxy flips a Ready/NotReady-equivalent annotation,
//! waits, then flips back.

use std::{collections::BTreeMap, path::PathBuf, time::Duration};

use anyhow::Context;
use reqwest::Client;
use tokio::time::sleep;
use url::Url;

const REBOOT_STATE_ANNOTATION: &str = "trident-acl-agent-tester/reboot-state";

pub async fn run(
    client: &Client,
    apiserver_url: &Url,
    node_name: &str,
    bootstrap_labels: BTreeMap<String, String>,
    marker_file: PathBuf,
    reboot_duration: Duration,
) -> anyhow::Result<()> {
    if !bootstrap_labels.is_empty() {
        patch_node_labels(client, apiserver_url, node_name, bootstrap_labels).await?;
    }

    loop {
        if marker_file.exists() {
            patch_node_annotations(
                client,
                apiserver_url,
                node_name,
                BTreeMap::from([(REBOOT_STATE_ANNOTATION.to_string(), "not-ready".to_string())]),
            )
            .await?;
            sleep(reboot_duration).await;
            patch_node_annotations(
                client,
                apiserver_url,
                node_name,
                BTreeMap::from([(REBOOT_STATE_ANNOTATION.to_string(), "ready".to_string())]),
            )
            .await?;
            std::fs::remove_file(&marker_file).with_context(|| {
                format!("failed to remove reboot marker {}", marker_file.display())
            })?;
        }

        sleep(Duration::from_secs(1)).await;
    }
}

pub fn write_reboot_marker(marker_file: &PathBuf) -> anyhow::Result<()> {
    if let Some(parent) = marker_file.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create reboot marker directory {}",
                parent.display()
            )
        })?;
    }
    std::fs::write(marker_file, b"reboot-requested\n")
        .with_context(|| format!("failed to write reboot marker {}", marker_file.display()))?;
    Ok(())
}

async fn patch_node_labels(
    client: &Client,
    apiserver_url: &Url,
    node_name: &str,
    labels: BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let url = apiserver_url.join(&format!("/api/v1/nodes/{node_name}"))?;
    client
        .patch(url)
        .header("content-type", "application/merge-patch+json")
        .body(serde_json::to_vec(
            &serde_json::json!({ "metadata": { "labels": labels } }),
        )?)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}

async fn patch_node_annotations(
    client: &Client,
    apiserver_url: &Url,
    node_name: &str,
    annotations: BTreeMap<String, String>,
) -> anyhow::Result<()> {
    let url = apiserver_url.join(&format!("/api/v1/nodes/{node_name}"))?;
    client
        .patch(url)
        .header("content-type", "application/merge-patch+json")
        .body(serde_json::to_vec(
            &serde_json::json!({ "metadata": { "annotations": annotations } }),
        )?)
        .send()
        .await?
        .error_for_status()?;
    Ok(())
}
