//! RP-side scenario runner for the Harpoon label protocol.

use std::{collections::BTreeMap, time::Instant};

use anyhow::Context;
use reqwest::Client;
use serde_json::json;
use tokio::time::{sleep, Duration, Instant as TokioInstant};
use url::Url;

use crate::scenario::{labels_from_patch_step, Scenario, ScenarioReport, ScenarioStep, StepReport};

const STATE_LABEL: &str = "kubernetes.azure.com/trident-abupdate-state";
const OBSERVED_REQUEST_ID_LABEL: &str = "kubernetes.azure.com/trident-abupdate-observed-request-id";
const FAILURE_REASON_LABEL: &str = "kubernetes.azure.com/trident-abupdate-failure-reason";

pub async fn run_scenario(
    client: &Client,
    apiserver_url: &Url,
    node_name: &str,
    scenario: &Scenario,
) -> anyhow::Result<ScenarioReport> {
    let mut steps = Vec::with_capacity(scenario.steps.len());
    let mut passed = true;

    for (index, step) in scenario.steps.iter().enumerate() {
        let start = Instant::now();
        let report = match step {
            ScenarioStep::Patch { patch } => {
                let labels = labels_from_patch_step(patch);
                patch_node_labels(client, apiserver_url, node_name, labels).await?;
                StepReport {
                    index,
                    kind: "patch".to_string(),
                    passed: true,
                    elapsed_ms: start.elapsed().as_millis(),
                    message: "patched fake Node labels".to_string(),
                    expected: None,
                    actual: None,
                }
            }
            ScenarioStep::Expect { expect } => {
                let deadline = TokioInstant::now() + expect.timeout.0;
                let expected = json!({
                    "state": expect.state,
                    "observedRequestId": expect.observed_request_id,
                    "expectTimeout": expect.expect_timeout,
                });
                let mut last_seen = json!({});
                let mut matched = false;

                while TokioInstant::now() < deadline {
                    let node = get_node(client, apiserver_url, node_name).await?;
                    let labels = label_map(&node);
                    last_seen = json!({
                        "state": labels.get(STATE_LABEL),
                        "observedRequestId": labels.get(OBSERVED_REQUEST_ID_LABEL),
                        "failureReason": labels.get(FAILURE_REASON_LABEL),
                    });
                    if labels.get(STATE_LABEL).map(String::as_str) == Some(expect.state.as_str())
                        && expect
                            .observed_request_id
                            .as_deref()
                            .map(|expected| {
                                labels.get(OBSERVED_REQUEST_ID_LABEL).map(String::as_str)
                                    == Some(expected)
                            })
                            .unwrap_or(true)
                    {
                        matched = true;
                        break;
                    }
                    sleep(Duration::from_millis(500)).await;
                }

                let passed_step = if expect.expect_timeout {
                    !matched
                } else {
                    matched
                };
                StepReport {
                    index,
                    kind: "expect".to_string(),
                    passed: passed_step,
                    elapsed_ms: start.elapsed().as_millis(),
                    message: if passed_step {
                        if expect.expect_timeout {
                            "timed out as expected".to_string()
                        } else {
                            "observed expected state".to_string()
                        }
                    } else {
                        "state expectation failed".to_string()
                    },
                    expected: Some(expected),
                    actual: Some(last_seen),
                }
            }
            ScenarioStep::AssertFailureReason {
                assert_failure_reason,
            } => {
                let node = get_node(client, apiserver_url, node_name).await?;
                let labels = label_map(&node);
                let actual = labels.get(FAILURE_REASON_LABEL).map(|v| v.to_string());
                let passed_step = actual.as_deref() == Some(assert_failure_reason.as_str());
                StepReport {
                    index,
                    kind: "assert-failure-reason".to_string(),
                    passed: passed_step,
                    elapsed_ms: start.elapsed().as_millis(),
                    message: if passed_step {
                        "failure reason matched".to_string()
                    } else {
                        "failure reason mismatch".to_string()
                    },
                    expected: Some(json!(assert_failure_reason)),
                    actual: Some(json!(actual)),
                }
            }
        };

        passed &= report.passed;
        steps.push(report);
    }

    Ok(ScenarioReport { passed, steps })
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
            &json!({ "metadata": { "labels": labels } }),
        )?)
        .send()
        .await?
        .error_for_status()
        .context("fake apiserver patch failed")?;
    Ok(())
}

async fn get_node(
    client: &Client,
    apiserver_url: &Url,
    node_name: &str,
) -> anyhow::Result<serde_json::Value> {
    let url = apiserver_url.join(&format!("/api/v1/nodes/{node_name}"))?;
    let body = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    Ok(serde_json::from_str(&body)?)
}

fn label_map(node: &serde_json::Value) -> BTreeMap<String, String> {
    node.get("metadata")
        .and_then(|metadata| metadata.get("labels"))
        .and_then(serde_json::Value::as_object)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}
